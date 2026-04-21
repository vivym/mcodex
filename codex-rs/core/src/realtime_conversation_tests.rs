use super::RealtimeHandoffState;
use super::RealtimeSessionKind;
use super::RealtimeStart;
use super::build_realtime_session_config;
use super::realtime_request_headers;
use super::realtime_text_from_handoff_request;
use crate::codex::make_session_and_context;
use async_channel::bounded;
use codex_api::Provider as ApiProvider;
use codex_api::RealtimeEventParser;
use codex_api::RealtimeOutputModality as ApiRealtimeOutputModality;
use codex_api::RealtimeSessionConfig;
use codex_api::RealtimeSessionMode;
use codex_api::RetryConfig;
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
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::Request;
use tokio_tungstenite::tungstenite::handshake::server::Response;

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

async fn install_runtime_lease_authority(
    session: &mut crate::codex::Session,
    authority: crate::runtime_lease::RuntimeLeaseAuthority,
) {
    let runtime_lease_host = crate::runtime_lease::RuntimeLeaseHost::pooled_with_authority_for_test(
        crate::runtime_lease::RuntimeLeaseHostId::new("runtime-lease-realtime-test".to_string()),
        authority,
    );
    let session_id = session.conversation_id.to_string();
    let provider = session.provider().await;
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
