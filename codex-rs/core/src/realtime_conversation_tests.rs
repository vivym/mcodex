use super::PreparedRealtimeConversationStart;
use super::RealtimeHandoffState;
use super::RealtimeSessionKind;
use super::RealtimeStart;
use super::RealtimeWsVersion;
use super::build_realtime_session_config;
use super::handle_start_inner;
use super::realtime_request_headers;
use super::realtime_text_from_handoff_request;
use crate::codex::make_session_and_context;
use crate::codex::make_session_and_context_with_rx;
use async_channel::bounded;
use codex_api::Provider as ApiProvider;
use codex_api::RealtimeEventParser;
use codex_api::RealtimeOutputModality as ApiRealtimeOutputModality;
use codex_api::RealtimeSessionConfig;
use codex_api::RealtimeSessionMode;
use codex_api::RetryConfig;
use codex_model_provider_info::WireApi;
use codex_model_provider_info::create_oss_provider_with_base_url;
use codex_protocol::protocol::ConversationStartTransport;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::RealtimeHandoffRequested;
use codex_protocol::protocol::RealtimeTranscriptEntry;
use codex_protocol::protocol::RealtimeVoice;
use futures::StreamExt;
use http::HeaderMap;
use pretty_assertions::assert_eq;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::Request;
use tokio_tungstenite::tungstenite::handshake::server::Response;
use tokio_util::sync::CancellationToken;

struct DropNotify(Option<oneshot::Sender<()>>);

impl Drop for DropNotify {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

#[test]
fn extracts_text_from_handoff_request_active_transcript() {
    let handoff = RealtimeHandoffRequested {
        handoff_id: "handoff_1".to_string(),
        item_id: "item_1".to_string(),
        input_transcript: "ignored".to_string(),
        active_transcript: vec![
            RealtimeTranscriptEntry {
                role: "user".to_string(),
                text: "hello".to_string(),
            },
            RealtimeTranscriptEntry {
                role: "assistant".to_string(),
                text: "hi there".to_string(),
            },
        ],
    };
    assert_eq!(
        realtime_text_from_handoff_request(&handoff),
        Some("user: hello\nassistant: hi there".to_string())
    );
}

#[test]
fn extracts_text_from_handoff_request_input_transcript_if_messages_missing() {
    let handoff = RealtimeHandoffRequested {
        handoff_id: "handoff_1".to_string(),
        item_id: "item_1".to_string(),
        input_transcript: "ignored".to_string(),
        active_transcript: vec![],
    };
    assert_eq!(
        realtime_text_from_handoff_request(&handoff),
        Some("ignored".to_string())
    );
}

#[test]
fn ignores_empty_handoff_request_input_transcript() {
    let handoff = RealtimeHandoffRequested {
        handoff_id: "handoff_1".to_string(),
        item_id: "item_1".to_string(),
        input_transcript: String::new(),
        active_transcript: vec![],
    };
    assert_eq!(realtime_text_from_handoff_request(&handoff), None);
}

#[tokio::test]
async fn clears_active_handoff_explicitly() {
    let (tx, _rx) = bounded(1);
    let state = RealtimeHandoffState::new(tx, RealtimeSessionKind::V1);

    *state.active_handoff.lock().await = Some("handoff_1".to_string());
    assert_eq!(
        state.active_handoff.lock().await.clone(),
        Some("handoff_1".to_string())
    );

    *state.active_handoff.lock().await = None;
    assert_eq!(state.active_handoff.lock().await.clone(), None);
}

#[tokio::test]
async fn build_realtime_session_config_uses_remote_session_id_after_reset() {
    let (session, _turn_context) = make_session_and_context().await;
    let session = Arc::new(session);
    let thread_id = session.conversation_id.to_string();

    let mut model_client_session = session.services.model_client.new_session();
    model_client_session.reset_remote_session_identity();
    let remote_session_id = model_client_session.remote_session_id().to_string();
    assert_ne!(remote_session_id, thread_id);

    let config = build_realtime_session_config(
        &session,
        None,
        None,
        codex_protocol::protocol::RealtimeOutputModality::Text,
        None,
    )
    .await
    .expect("build realtime config");

    assert_eq!(
        config.session_id.as_deref(),
        Some(remote_session_id.as_str())
    );
}

#[tokio::test]
async fn realtime_websocket_start_acquires_and_holds_runtime_admission() {
    let (mut session, _turn_context) = make_session_and_context().await;
    let session = Arc::new({
        let authority =
            crate::runtime_lease::RuntimeLeaseAuthority::for_test_accepting("pooled-account", 7);
        install_runtime_lease_authority(&mut session, authority.clone()).await;
        session
    });
    let authority = session
        .services
        .runtime_lease_host
        .as_ref()
        .and_then(crate::runtime_lease::RuntimeLeaseHost::pooled_authority)
        .expect("runtime authority");
    let (base_url, captured_headers, release_server, server_task) =
        spawn_realtime_websocket_server().await;

    let start = RealtimeStart {
        api_provider: test_realtime_provider(base_url),
        extra_headers: realtime_request_headers(Some("session-1"), /*api_key*/ None)
            .expect("headers"),
        session_id_is_implicit: false,
        session_config: RealtimeSessionConfig {
            instructions: "backend prompt".to_string(),
            model: Some("gpt-realtime-test".to_string()),
            session_id: Some("session-1".to_string()),
            event_parser: RealtimeEventParser::RealtimeV2,
            session_mode: RealtimeSessionMode::Conversational,
            output_modality: ApiRealtimeOutputModality::Text,
            voice: RealtimeVoice::Marin,
        },
        model_client: session.services.model_client.clone(),
        session_telemetry: session.services.session_telemetry.clone(),
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        sdp: None,
    };

    let output = session
        .conversation
        .start(start)
        .await
        .expect("realtime websocket should start");

    assert_eq!(
        authority.recorded_boundaries_for_test(),
        vec![crate::runtime_lease::RequestBoundaryKind::Realtime]
    );
    assert_eq!(authority.admitted_count_for_test(), 1);
    {
        let headers = captured_headers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            headers.get("x-session-id").and_then(|v| v.to_str().ok()),
            Some("session-1")
        );
        assert_eq!(
            headers
                .get("chatgpt-account-id")
                .and_then(|v| v.to_str().ok()),
            Some("pooled-account")
        );
    }

    session
        .conversation
        .finish_if_active(&output.realtime_active)
        .await;
    assert_eq!(authority.admitted_count_for_test(), 0);
    let _ = release_server.send(());
    server_task.await.expect("server task should finish");
}

#[tokio::test]
async fn realtime_websocket_start_rebuilds_implicit_session_id_after_remote_context_reset() {
    let (mut session, _turn_context) = make_session_and_context().await;
    let session = Arc::new({
        let authority =
            crate::runtime_lease::RuntimeLeaseAuthority::for_test_accepting("pooled-account", 7);
        install_runtime_lease_authority(&mut session, authority.clone()).await;
        session
    });
    let initial_session_id = session
        .services
        .model_client
        .remote_session_id()
        .to_string();
    let session_config = build_realtime_session_config(
        &session,
        None,
        None,
        codex_protocol::protocol::RealtimeOutputModality::Text,
        None,
    )
    .await
    .expect("build realtime config");
    assert_eq!(
        session_config.session_id.as_deref(),
        Some(initial_session_id.as_str())
    );
    session
        .services
        .model_client
        .reset_remote_session_identity();
    let reset_session_id = session
        .services
        .model_client
        .remote_session_id()
        .to_string();
    assert_ne!(reset_session_id, initial_session_id);
    let (base_url, captured_headers, release_server, server_task) =
        spawn_realtime_websocket_server().await;

    let output = session
        .conversation
        .start(RealtimeStart {
            api_provider: test_realtime_provider(base_url),
            extra_headers: realtime_request_headers(
                session_config.session_id.as_deref(),
                /*api_key*/ None,
            )
            .expect("headers"),
            session_id_is_implicit: true,
            session_config,
            model_client: session.services.model_client.clone(),
            session_telemetry: session.services.session_telemetry.clone(),
            cancellation_token: tokio_util::sync::CancellationToken::new(),
            sdp: None,
        })
        .await
        .expect("realtime websocket should start after remote-context reset");

    {
        let headers = captured_headers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            headers.get("x-session-id").and_then(|v| v.to_str().ok()),
            Some(reset_session_id.as_str())
        );
    }

    session
        .conversation
        .finish_if_active(&output.realtime_active)
        .await;
    let _ = release_server.send(());
    server_task.await.expect("server task should finish");
}

#[tokio::test]
async fn realtime_websocket_start_retries_unauthorized_with_fresh_admission() {
    let (mut session, _turn_context) = make_session_and_context().await;
    let session = Arc::new({
        let authority =
            crate::runtime_lease::RuntimeLeaseAuthority::for_test_accepting("pooled-account", 7);
        install_runtime_lease_authority(&mut session, authority.clone()).await;
        session
    });
    let authority = session
        .services
        .runtime_lease_host
        .as_ref()
        .and_then(crate::runtime_lease::RuntimeLeaseHost::pooled_authority)
        .expect("runtime authority");
    let (base_url, captured_headers, release_server, server_task) =
        spawn_realtime_websocket_server_with_initial_401().await;

    let start = RealtimeStart {
        api_provider: test_realtime_provider(base_url),
        extra_headers: realtime_request_headers(Some("session-1"), /*api_key*/ None)
            .expect("headers"),
        session_id_is_implicit: false,
        session_config: RealtimeSessionConfig {
            instructions: "backend prompt".to_string(),
            model: Some("gpt-realtime-test".to_string()),
            session_id: Some("session-1".to_string()),
            event_parser: RealtimeEventParser::RealtimeV2,
            session_mode: RealtimeSessionMode::Conversational,
            output_modality: ApiRealtimeOutputModality::Text,
            voice: RealtimeVoice::Marin,
        },
        model_client: session.services.model_client.clone(),
        session_telemetry: session.services.session_telemetry.clone(),
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        sdp: None,
    };

    let output = session
        .conversation
        .start(start)
        .await
        .expect("realtime websocket should recover from 401");

    assert_eq!(
        authority.recorded_boundaries_for_test(),
        vec![
            crate::runtime_lease::RequestBoundaryKind::Realtime,
            crate::runtime_lease::RequestBoundaryKind::Realtime,
        ]
    );
    assert_eq!(authority.admitted_count_for_test(), 1);
    {
        let headers = captured_headers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            headers.get("x-session-id").and_then(|v| v.to_str().ok()),
            Some("session-1")
        );
        assert_eq!(
            headers
                .get("chatgpt-account-id")
                .and_then(|v| v.to_str().ok()),
            Some("pooled-account")
        );
    }

    session
        .conversation
        .finish_if_active(&output.realtime_active)
        .await;
    assert_eq!(authority.admitted_count_for_test(), 0);
    let _ = release_server.send(());
    server_task.await.expect("server task should finish");
}

#[tokio::test]
async fn realtime_webrtc_start_retries_sideband_unauthorized_without_recreating_call() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    let (mut session, _turn_context) = make_session_and_context().await;
    let authority =
        crate::runtime_lease::RuntimeLeaseAuthority::for_test_accepting("pooled-account", 7);
    let call_server = MockServer::start().await;
    install_runtime_lease_authority_with_base_url(&mut session, authority, &call_server.uri())
        .await;
    let session = Arc::new(session);
    Mock::given(method("POST"))
        .and(path("/realtime/calls"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("location", "/v1/realtime/calls/rtc_test")
                .set_body_string("v=answer\r\n"),
        )
        .expect(1)
        .mount(&call_server)
        .await;
    let (base_url, _captured_headers, release_server, server_task) =
        spawn_realtime_websocket_server_with_initial_401s(2).await;

    let output = session
        .conversation
        .start(RealtimeStart {
            api_provider: test_realtime_provider(base_url),
            extra_headers: realtime_request_headers(Some("session-1"), /*api_key*/ None)
                .expect("headers"),
            session_id_is_implicit: false,
            session_config: RealtimeSessionConfig {
                instructions: "backend prompt".to_string(),
                model: Some("gpt-realtime-test".to_string()),
                session_id: Some("session-1".to_string()),
                event_parser: RealtimeEventParser::RealtimeV2,
                session_mode: RealtimeSessionMode::Conversational,
                output_modality: ApiRealtimeOutputModality::Text,
                voice: RealtimeVoice::Marin,
            },
            model_client: session.services.model_client.clone(),
            session_telemetry: session.services.session_telemetry.clone(),
            cancellation_token: tokio_util::sync::CancellationToken::new(),
            sdp: Some("v=offer\r\n".to_string()),
        })
        .await
        .expect("webrtc sideband 401 should recover without recreating the call");

    let requests = call_server
        .received_requests()
        .await
        .expect("captured realtime call requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/realtime/calls");

    session
        .conversation
        .finish_if_active(&output.realtime_active)
        .await;
    let _ = release_server.send(());
    server_task.await.expect("server task should finish");
}

#[tokio::test]
async fn realtime_webrtc_start_holds_call_admission_until_sideband_connects() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    let (mut session, _turn_context) = make_session_and_context().await;
    let authority =
        crate::runtime_lease::RuntimeLeaseAuthority::for_test_accepting("pooled-account", 7);
    let call_server = MockServer::start().await;
    install_runtime_lease_authority_with_base_url(
        &mut session,
        authority.clone(),
        &call_server.uri(),
    )
    .await;
    let tree_id = session
        .services
        .model_client
        .current_collaboration_tree_id();
    let session = Arc::new(session);
    Mock::given(method("POST"))
        .and(path("/realtime/calls"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("location", "/v1/realtime/calls/rtc_test")
                .set_body_string("v=answer\r\n"),
        )
        .expect(1)
        .mount(&call_server)
        .await;
    let (base_url, _captured_headers, release_server, server_task) =
        spawn_realtime_websocket_server_with_accept_delay(Duration::from_millis(500)).await;

    let mut start_task = tokio::spawn({
        let session = Arc::clone(&session);
        async move {
            session
                .conversation
                .start(RealtimeStart {
                    api_provider: test_realtime_provider(base_url),
                    extra_headers: realtime_request_headers(
                        Some("session-1"),
                        /*api_key*/ None,
                    )
                    .expect("headers"),
                    session_id_is_implicit: false,
                    session_config: RealtimeSessionConfig {
                        instructions: "backend prompt".to_string(),
                        model: Some("gpt-realtime-test".to_string()),
                        session_id: Some("session-1".to_string()),
                        event_parser: RealtimeEventParser::RealtimeV2,
                        session_mode: RealtimeSessionMode::Conversational,
                        output_modality: ApiRealtimeOutputModality::Text,
                        voice: RealtimeVoice::Marin,
                    },
                    model_client: session.services.model_client.clone(),
                    session_telemetry: session.services.session_telemetry.clone(),
                    cancellation_token: tokio_util::sync::CancellationToken::new(),
                    sdp: Some("v=offer\r\n".to_string()),
                })
                .await
        }
    });

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let requests = call_server
                .received_requests()
                .await
                .expect("captured realtime call requests");
            if requests.len() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("webrtc call should be created");
    wait_for_admitted_count(&authority, 2).await;

    let sibling_admission = authority
        .acquire_request_lease_for_test(crate::runtime_lease::LeaseRequestContext::new(
            crate::runtime_lease::RequestBoundaryKind::Realtime,
            "realtime-sibling".to_string(),
            tree_id,
            Some("sibling".to_string()),
            tokio_util::sync::CancellationToken::new(),
        ))
        .await
        .expect("sibling admission should succeed");
    authority
        .report_terminal_unauthorized(&sibling_admission.snapshot)
        .await
        .expect("terminal unauthorized should report");
    drop(sibling_admission.guard);

    let join_result = tokio::time::timeout(Duration::from_secs(2), &mut start_task)
        .await
        .unwrap_or_else(|_| {
            start_task.abort();
            panic!("webrtc start should stop after sibling 401");
        });
    let start_result = join_result.expect("start task should join");
    assert!(
        start_result.is_err(),
        "webrtc start should fail once sibling 401 cancels the tree"
    );
    wait_for_admitted_count(&authority, 0).await;

    let _ = release_server.send(());
    server_task.await.expect("server task should finish");
}

#[tokio::test]
async fn realtime_websocket_start_cancels_while_connect_is_in_flight_after_sibling_terminal_unauthorized()
 {
    let (mut session, _turn_context) = make_session_and_context().await;
    let runtime_lease_host = crate::runtime_lease::RuntimeLeaseHost::pooled_with_authority_for_test(
        crate::runtime_lease::RuntimeLeaseHostId::new(
            "runtime-lease-realtime-start-test".to_string(),
        ),
        crate::runtime_lease::RuntimeLeaseAuthority::for_test_accepting("pooled-account", 7),
    );
    install_runtime_lease_authority(
        &mut session,
        runtime_lease_host
            .pooled_authority()
            .expect("runtime authority should exist"),
    )
    .await;
    let authority = session
        .services
        .runtime_lease_host
        .as_ref()
        .and_then(crate::runtime_lease::RuntimeLeaseHost::pooled_authority)
        .expect("runtime authority");
    let runtime_lease_host = session
        .services
        .runtime_lease_host
        .as_ref()
        .cloned()
        .expect("runtime host");
    let tree_id = crate::runtime_lease::CollaborationTreeId::for_test("realtime-start-tree");
    let member_cancel = tokio_util::sync::CancellationToken::new();
    let membership = runtime_lease_host.register_collaboration_member(
        tree_id.clone(),
        "realtime-member".to_string(),
        member_cancel.clone(),
    );
    let _binding = session
        .services
        .model_client
        .bind_collaboration_tree(membership);
    let session = Arc::new(session);
    let (base_url, _captured_headers, release_server, server_task) =
        spawn_realtime_websocket_server_with_accept_delay(Duration::from_millis(500)).await;

    let start = RealtimeStart {
        api_provider: test_realtime_provider(base_url),
        extra_headers: realtime_request_headers(Some("session-1"), /*api_key*/ None)
            .expect("headers"),
        session_id_is_implicit: false,
        session_config: RealtimeSessionConfig {
            instructions: "backend prompt".to_string(),
            model: Some("gpt-realtime-test".to_string()),
            session_id: Some("session-1".to_string()),
            event_parser: RealtimeEventParser::RealtimeV2,
            session_mode: RealtimeSessionMode::Conversational,
            output_modality: ApiRealtimeOutputModality::Text,
            voice: RealtimeVoice::Marin,
        },
        model_client: session.services.model_client.clone(),
        session_telemetry: session.services.session_telemetry.clone(),
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        sdp: None,
    };

    let mut start_task = tokio::spawn(async move { session.conversation.start(start).await });

    wait_for_admitted_count(&authority, 1).await;
    let sibling_admission = authority
        .acquire_request_lease_for_test(crate::runtime_lease::LeaseRequestContext::new(
            crate::runtime_lease::RequestBoundaryKind::Realtime,
            "realtime-sibling".to_string(),
            tree_id,
            Some("sibling".to_string()),
            tokio_util::sync::CancellationToken::new(),
        ))
        .await
        .expect("sibling admission should succeed");
    authority
        .report_terminal_unauthorized(&sibling_admission.snapshot)
        .await
        .expect("terminal unauthorized should report");
    drop(sibling_admission.guard);

    let join_result = tokio::time::timeout(Duration::from_secs(2), &mut start_task)
        .await
        .unwrap_or_else(|_| {
            start_task.abort();
            panic!("realtime websocket start should stop after sibling 401");
        });
    let start_result = join_result.expect("start task should join");
    assert!(
        start_result.is_err(),
        "realtime websocket start should fail once sibling 401 cancels the tree"
    );
    assert!(member_cancel.is_cancelled());
    wait_for_admitted_count(&authority, 0).await;

    let _ = release_server.send(());
    server_task.await.expect("server task should finish");
}

#[tokio::test]
async fn realtime_websocket_running_session_stops_when_sibling_member_reports_terminal_unauthorized()
 {
    let (mut session, _turn_context) = make_session_and_context().await;
    let runtime_lease_host = crate::runtime_lease::RuntimeLeaseHost::pooled_with_authority_for_test(
        crate::runtime_lease::RuntimeLeaseHostId::new("runtime-lease-realtime-test".to_string()),
        crate::runtime_lease::RuntimeLeaseAuthority::for_test_accepting("pooled-account", 7),
    );
    install_runtime_lease_authority(
        &mut session,
        runtime_lease_host
            .pooled_authority()
            .expect("runtime authority should exist"),
    )
    .await;
    let authority = session
        .services
        .runtime_lease_host
        .as_ref()
        .and_then(crate::runtime_lease::RuntimeLeaseHost::pooled_authority)
        .expect("runtime authority");
    let runtime_lease_host = session
        .services
        .runtime_lease_host
        .as_ref()
        .cloned()
        .expect("runtime host");
    let tree_id = crate::runtime_lease::CollaborationTreeId::for_test("realtime-tree");
    let member_cancel = tokio_util::sync::CancellationToken::new();
    let membership = runtime_lease_host.register_collaboration_member(
        tree_id.clone(),
        "realtime-member".to_string(),
        member_cancel.clone(),
    );
    let _binding = session
        .services
        .model_client
        .bind_collaboration_tree(membership);
    let session = Arc::new(session);
    let (base_url, _captured_headers, release_server, server_task) =
        spawn_realtime_websocket_server().await;

    let start = RealtimeStart {
        api_provider: test_realtime_provider(base_url),
        extra_headers: realtime_request_headers(Some("session-1"), /*api_key*/ None)
            .expect("headers"),
        session_id_is_implicit: false,
        session_config: RealtimeSessionConfig {
            instructions: "backend prompt".to_string(),
            model: Some("gpt-realtime-test".to_string()),
            session_id: Some("session-1".to_string()),
            event_parser: RealtimeEventParser::RealtimeV2,
            session_mode: RealtimeSessionMode::Conversational,
            output_modality: ApiRealtimeOutputModality::Text,
            voice: RealtimeVoice::Marin,
        },
        model_client: session.services.model_client.clone(),
        session_telemetry: session.services.session_telemetry.clone(),
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        sdp: None,
    };

    let output = session
        .conversation
        .start(start)
        .await
        .expect("realtime websocket should start");

    let sibling_cancel = tokio_util::sync::CancellationToken::new();
    let sibling_request = crate::runtime_lease::LeaseRequestContext::new(
        crate::runtime_lease::RequestBoundaryKind::Realtime,
        "session-sibling".to_string(),
        tree_id,
        Some("sibling".to_string()),
        sibling_cancel.clone(),
    );
    let sibling_admission = authority
        .acquire_request_lease_for_test(sibling_request)
        .await
        .expect("sibling admission should succeed");

    authority
        .report_terminal_unauthorized(&sibling_admission.snapshot)
        .await
        .expect("terminal unauthorized should report");
    drop(sibling_admission.guard);

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !output
                .realtime_active
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("running realtime session should stop after sibling 401");

    assert!(member_cancel.is_cancelled());
    assert!(authority.admitted_count_for_test() <= 1);
    let _ = release_server.send(());
    server_task.await.expect("server task should finish");
}

#[tokio::test]
async fn realtime_websocket_lease_cancellation_emits_closed_event() {
    let (session, _turn_context, rx) = make_session_and_context_with_rx().await;
    let mut session = Arc::into_inner(session).expect("unique session");
    let runtime_lease_host = crate::runtime_lease::RuntimeLeaseHost::pooled_with_authority_for_test(
        crate::runtime_lease::RuntimeLeaseHostId::new(
            "runtime-lease-realtime-closed-test".to_string(),
        ),
        crate::runtime_lease::RuntimeLeaseAuthority::for_test_accepting("pooled-account", 7),
    );
    install_runtime_lease_authority(
        &mut session,
        runtime_lease_host
            .pooled_authority()
            .expect("runtime authority should exist"),
    )
    .await;
    let authority = session
        .services
        .runtime_lease_host
        .as_ref()
        .and_then(crate::runtime_lease::RuntimeLeaseHost::pooled_authority)
        .expect("runtime authority");
    let runtime_lease_host = session
        .services
        .runtime_lease_host
        .as_ref()
        .cloned()
        .expect("runtime host");
    let tree_id = crate::runtime_lease::CollaborationTreeId::for_test("realtime-closed-tree");
    let member_cancel = tokio_util::sync::CancellationToken::new();
    let membership = runtime_lease_host.register_collaboration_member(
        tree_id.clone(),
        "realtime-member".to_string(),
        member_cancel.clone(),
    );
    let _binding = session
        .services
        .model_client
        .bind_collaboration_tree(membership);
    let session = Arc::new(session);
    let (base_url, _captured_headers, release_server, server_task) =
        spawn_realtime_websocket_server().await;

    handle_start_inner(
        &session,
        "sub-realtime-closed",
        PreparedRealtimeConversationStart {
            api_provider: test_realtime_provider(base_url),
            extra_headers: realtime_request_headers(Some("session-1"), /*api_key*/ None)
                .expect("headers"),
            requested_session_id: Some("session-1".to_string()),
            session_id_is_implicit: false,
            version: RealtimeWsVersion::V2,
            session_config: RealtimeSessionConfig {
                instructions: "backend prompt".to_string(),
                model: Some("gpt-realtime-test".to_string()),
                session_id: Some("session-1".to_string()),
                event_parser: RealtimeEventParser::RealtimeV2,
                session_mode: RealtimeSessionMode::Conversational,
                output_modality: ApiRealtimeOutputModality::Text,
                voice: RealtimeVoice::Marin,
            },
            transport: ConversationStartTransport::Websocket,
        },
    )
    .await
    .expect("realtime websocket should start");

    let sibling_cancel = tokio_util::sync::CancellationToken::new();
    let sibling_request = crate::runtime_lease::LeaseRequestContext::new(
        crate::runtime_lease::RequestBoundaryKind::Realtime,
        "session-sibling".to_string(),
        tree_id,
        Some("sibling".to_string()),
        sibling_cancel,
    );
    let sibling_admission = authority
        .acquire_request_lease_for_test(sibling_request)
        .await
        .expect("sibling admission should succeed");
    authority
        .report_terminal_unauthorized(&sibling_admission.snapshot)
        .await
        .expect("terminal unauthorized should report");
    drop(sibling_admission.guard);

    let closed_reason = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let event = rx.recv().await.expect("event");
            if let EventMsg::RealtimeConversationClosed(closed) = event.msg {
                return closed.reason;
            }
        }
    })
    .await
    .expect("lease cancellation should emit a closed event");

    assert_eq!(closed_reason.as_deref(), Some("transport_closed"));
    assert!(member_cancel.is_cancelled());
    let _ = release_server.send(());
    server_task.await.expect("server task should finish");
}

#[tokio::test]
async fn lease_cancellation_task_aborts_input_task_without_self_aborting_cleanup() {
    let (mut session, _turn_context) = make_session_and_context().await;
    let session = Arc::new({
        let authority =
            crate::runtime_lease::RuntimeLeaseAuthority::for_test_accepting("pooled-account", 7);
        install_runtime_lease_authority(&mut session, authority).await;
        session
    });
    let (base_url, _captured_headers, release_server, server_task) =
        spawn_realtime_websocket_server().await;

    let start = RealtimeStart {
        api_provider: test_realtime_provider(base_url),
        extra_headers: realtime_request_headers(Some("session-1"), /*api_key*/ None)
            .expect("headers"),
        session_id_is_implicit: false,
        session_config: RealtimeSessionConfig {
            instructions: "backend prompt".to_string(),
            model: Some("gpt-realtime-test".to_string()),
            session_id: Some("session-1".to_string()),
            event_parser: RealtimeEventParser::RealtimeV2,
            session_mode: RealtimeSessionMode::Conversational,
            output_modality: ApiRealtimeOutputModality::Text,
            voice: RealtimeVoice::Marin,
        },
        model_client: session.services.model_client.clone(),
        session_telemetry: session.services.session_telemetry.clone(),
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        sdp: None,
    };

    let output = session
        .conversation
        .start(start)
        .await
        .expect("realtime websocket should start");
    let (input_task_dropped_tx, input_task_dropped_rx) = oneshot::channel();
    let replacement_input_task = tokio::spawn(async move {
        let _notify = DropNotify(Some(input_task_dropped_tx));
        std::future::pending::<()>().await;
    });
    let replacement_lease_cancellation_token = CancellationToken::new();
    let mut state = {
        let mut guard = session.conversation.state.lock().await;
        guard.take().expect("running conversation state")
    };
    state.input_task.abort();
    let _ = state.input_task.await;
    if let Some(lease_cancellation_task) = state.lease_cancellation_task.take() {
        lease_cancellation_task.abort();
        let _ = lease_cancellation_task.await;
    }
    let replacement_realtime_active = Arc::clone(&state.realtime_active);
    state.input_task = replacement_input_task;
    state.lease_cancellation_task = Some(tokio::spawn({
        let cancellation_token = replacement_lease_cancellation_token.clone();
        let manager = session.conversation.clone();
        let realtime_active = Arc::clone(&replacement_realtime_active);
        async move {
            cancellation_token.cancelled().await;
            manager
                .finish_if_active_from_lease_cancellation(&realtime_active)
                .await;
        }
    }));
    {
        let mut guard = session.conversation.state.lock().await;
        *guard = Some(state);
    }

    replacement_lease_cancellation_token.cancel();

    tokio::time::timeout(Duration::from_secs(1), input_task_dropped_rx)
        .await
        .expect("lease cancellation cleanup should abort input task")
        .expect("input task drop notification should arrive");
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if session.conversation.running_state().await.is_none() {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("lease cancellation cleanup should clear running state");

    assert!(
        !output
            .realtime_active
            .load(std::sync::atomic::Ordering::Relaxed),
        "lease cancellation should deactivate the running conversation"
    );
    let _ = release_server.send(());
    server_task.await.expect("server task should finish");
}

async fn install_runtime_lease_authority(
    session: &mut crate::codex::Session,
    authority: crate::runtime_lease::RuntimeLeaseAuthority,
) {
    let runtime_lease_host = crate::runtime_lease::RuntimeLeaseHost::pooled_with_authority_for_test(
        crate::runtime_lease::RuntimeLeaseHostId::new("runtime-lease-realtime-test".to_string()),
        authority,
    );
    let provider = session.provider().await;
    install_runtime_lease_host_and_provider(session, runtime_lease_host, provider);
}

async fn install_runtime_lease_authority_with_base_url(
    session: &mut crate::codex::Session,
    authority: crate::runtime_lease::RuntimeLeaseAuthority,
    base_url: &str,
) {
    let runtime_lease_host = crate::runtime_lease::RuntimeLeaseHost::pooled_with_authority_for_test(
        crate::runtime_lease::RuntimeLeaseHostId::new("runtime-lease-realtime-test".to_string()),
        authority,
    );
    let provider = create_oss_provider_with_base_url(base_url, WireApi::Responses);
    install_runtime_lease_host_and_provider(session, runtime_lease_host, provider);
}

fn install_runtime_lease_host_and_provider(
    session: &mut crate::codex::Session,
    runtime_lease_host: crate::runtime_lease::RuntimeLeaseHost,
    provider: codex_model_provider_info::ModelProviderInfo,
) {
    let session_id = session.conversation_id.to_string();
    session.services.runtime_lease_host = Some(runtime_lease_host.clone());
    session.services.model_client = crate::client::ModelClient::new_with_runtime_lease(
        /*auth_manager*/ None,
        /*lease_auth*/ None,
        Some(runtime_lease_host),
        Some(Arc::new(tokio::sync::Mutex::new(
            crate::runtime_lease::SessionLeaseView::new(),
        ))),
        session_id.clone(),
        Arc::new(crate::runtime_lease::CollaborationTreeBindingHandle::new(
            crate::runtime_lease::CollaborationTreeId::root_for_session(&session_id),
        )),
        session.conversation_id,
        "11111111-1111-4111-8111-111111111111".to_string(),
        provider,
        codex_protocol::protocol::SessionSource::Exec,
        /*model_verbosity*/ None,
        /*enable_request_compression*/ false,
        /*include_timing_metrics*/ false,
        /*beta_features_header*/ None,
    );
}

fn test_realtime_provider(base_url: String) -> ApiProvider {
    ApiProvider {
        name: "test".to_string(),
        base_url,
        query_params: Some(HashMap::new()),
        headers: HeaderMap::new(),
        retry: RetryConfig {
            max_attempts: 1,
            base_delay: Duration::from_millis(1),
            retry_429: false,
            retry_5xx: false,
            retry_transport: false,
        },
        stream_idle_timeout: Duration::from_secs(5),
    }
}

async fn wait_for_admitted_count(
    authority: &crate::runtime_lease::RuntimeLeaseAuthority,
    expected: usize,
) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if authority.admitted_count_for_test() == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("timed out waiting for admitted count");
}

async fn spawn_realtime_websocket_server() -> (
    String,
    Arc<std::sync::Mutex<HeaderMap>>,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind realtime websocket listener");
    let addr = listener.local_addr().expect("listener address");
    let captured_headers = Arc::new(std::sync::Mutex::new(HeaderMap::new()));
    let (release_tx, release_rx) = oneshot::channel();
    let headers_for_callback = Arc::clone(&captured_headers);
    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept websocket");
        let callback = move |request: &Request, response: Response| {
            let mut headers = headers_for_callback
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *headers = request.headers().clone();
            Ok(response)
        };
        let mut ws = accept_hdr_async(stream, callback)
            .await
            .expect("websocket handshake");
        let first = ws
            .next()
            .await
            .expect("session update message")
            .expect("session update websocket result")
            .into_text()
            .expect("session update text");
        let first_json: Value = serde_json::from_str(&first).expect("session update json");
        assert_eq!(first_json["type"], "session.update");
        let _ = release_rx.await;
        let _ = ws.close(None).await;
    });

    (
        format!("http://{addr}"),
        captured_headers,
        release_tx,
        server_task,
    )
}

async fn spawn_realtime_websocket_server_with_initial_401() -> (
    String,
    Arc<std::sync::Mutex<HeaderMap>>,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    spawn_realtime_websocket_server_with_initial_401s(1).await
}

async fn spawn_realtime_websocket_server_with_initial_401s(
    initial_401s: usize,
) -> (
    String,
    Arc<std::sync::Mutex<HeaderMap>>,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind realtime websocket listener");
    let addr = listener.local_addr().expect("listener address");
    let captured_headers = Arc::new(std::sync::Mutex::new(HeaderMap::new()));
    let (release_tx, release_rx) = oneshot::channel();
    let headers_for_callback = Arc::clone(&captured_headers);
    let server_task = tokio::spawn(async move {
        for _ in 0..initial_401s {
            let (mut unauthorized_stream, _) =
                listener.accept().await.expect("accept 401 websocket");
            let mut request_buf = [0_u8; 1024];
            let _ = unauthorized_stream.read(&mut request_buf).await;
            unauthorized_stream
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .expect("write 401 response");
        }

        let (stream, _) = listener.accept().await.expect("accept websocket");
        let callback = move |request: &Request, response: Response| {
            let mut headers = headers_for_callback
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *headers = request.headers().clone();
            Ok(response)
        };
        let mut ws = accept_hdr_async(stream, callback)
            .await
            .expect("websocket handshake");
        let first = ws
            .next()
            .await
            .expect("session update message")
            .expect("session update websocket result")
            .into_text()
            .expect("session update text");
        let first_json: Value = serde_json::from_str(&first).expect("session update json");
        assert_eq!(first_json["type"], "session.update");
        let _ = release_rx.await;
        let _ = ws.close(None).await;
    });

    (
        format!("http://{addr}"),
        captured_headers,
        release_tx,
        server_task,
    )
}

async fn spawn_realtime_websocket_server_with_accept_delay(
    accept_delay: Duration,
) -> (
    String,
    Arc<std::sync::Mutex<HeaderMap>>,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind realtime websocket listener");
    let addr = listener.local_addr().expect("listener address");
    let captured_headers = Arc::new(std::sync::Mutex::new(HeaderMap::new()));
    let (release_tx, release_rx) = oneshot::channel();
    let headers_for_callback = Arc::clone(&captured_headers);
    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept websocket");
        tokio::time::sleep(accept_delay).await;
        let callback = move |request: &Request, response: Response| {
            let mut headers = headers_for_callback
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *headers = request.headers().clone();
            Ok(response)
        };
        let Ok(mut ws) = accept_hdr_async(stream, callback).await else {
            return;
        };
        let first = ws
            .next()
            .await
            .expect("session update message")
            .expect("session update websocket result")
            .into_text()
            .expect("session update text");
        let first_json: Value = serde_json::from_str(&first).expect("session update json");
        assert_eq!(first_json["type"], "session.update");
        let _ = release_rx.await;
        let _ = ws.close(None).await;
    });

    (
        format!("http://{addr}"),
        captured_headers,
        release_tx,
        server_task,
    )
}
