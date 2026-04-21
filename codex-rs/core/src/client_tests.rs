use super::AuthRequestTelemetryContext;
use super::ModelClient;
use super::PendingUnauthorizedRetry;
use super::UnauthorizedRecoveryExecution;
use super::X_CODEX_INSTALLATION_ID_HEADER;
use super::X_CODEX_PARENT_THREAD_ID_HEADER;
use super::X_CODEX_TURN_METADATA_HEADER;
use super::X_CODEX_WINDOW_ID_HEADER;
use super::X_OPENAI_SUBAGENT_HEADER;
use crate::ResponseEvent;
use crate::ResponseStream;
use crate::client_common::Prompt;
use crate::lease_auth::SessionLeaseAuth;
use crate::runtime_lease::CollaborationTreeBindingHandle;
use crate::runtime_lease::CollaborationTreeId;
use crate::runtime_lease::RemoteContextResetRecord;
use crate::runtime_lease::RequestBoundaryKind;
use crate::runtime_lease::RuntimeLeaseAuthority;
use crate::runtime_lease::RuntimeLeaseHost;
use crate::runtime_lease::RuntimeLeaseHostId;
use crate::runtime_lease::SessionLeaseView;
use anyhow::bail;
use base64::Engine;
use codex_api::CoreAuthProvider;
use codex_api::RawMemory;
use codex_api::RawMemoryMetadata;
use codex_api::RealtimeEventParser;
use codex_api::RealtimeOutputModality;
use codex_api::RealtimeSessionConfig;
use codex_api::RealtimeSessionMode;
use codex_app_server_protocol::AuthMode;
use codex_config::types::AccountPoolDefinitionToml;
use codex_config::types::AccountsConfigToml;
use codex_login::AuthCredentialsStoreMode;
use codex_login::AuthDotJson;
use codex_login::CodexAuth;
use codex_login::TokenData;
use codex_login::auth::LeaseAuthBinding;
use codex_login::auth::LeaseScopedAuthSession;
use codex_login::auth::LeasedTurnAuth;
use codex_login::save_auth;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_model_provider_info::create_oss_provider_with_base_url;
use codex_otel::SessionTelemetry;
use codex_protocol::ThreadId;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::protocol::RealtimeVoice;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_state::AccountRegistryEntryUpdate;
use codex_state::AccountStartupSelectionUpdate;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn test_model_client(session_source: SessionSource) -> ModelClient {
    test_model_client_with_lease_auth(session_source, /*lease_auth*/ None)
}

fn test_model_client_with_lease_auth(
    session_source: SessionSource,
    lease_auth: Option<Arc<SessionLeaseAuth>>,
) -> ModelClient {
    let provider = create_oss_provider_with_base_url("https://example.com/v1", WireApi::Responses);
    ModelClient::new(
        /*auth_manager*/ None,
        lease_auth,
        ThreadId::new(),
        /*installation_id*/ "11111111-1111-4111-8111-111111111111".to_string(),
        provider,
        session_source,
        /*model_verbosity*/ None,
        /*enable_request_compression*/ false,
        /*include_timing_metrics*/ false,
        /*beta_features_header*/ None,
    )
}

fn test_model_client_with_runtime_lease_view(_allow_context_reuse: bool) -> ModelClient {
    let provider = create_oss_provider_with_base_url("https://example.com/v1", WireApi::Responses);
    let conversation_id = ThreadId::new();
    let session_id = conversation_id.to_string();
    ModelClient::new_with_runtime_lease(
        /*auth_manager*/ None,
        /*lease_auth*/ None,
        Some(RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new(
            "runtime-lease-test".to_string(),
        ))),
        Some(Arc::new(tokio::sync::Mutex::new(SessionLeaseView::new()))),
        session_id.clone(),
        Arc::new(CollaborationTreeBindingHandle::new(
            CollaborationTreeId::root_for_session(&session_id),
        )),
        conversation_id,
        /*installation_id*/ "11111111-1111-4111-8111-111111111111".to_string(),
        provider,
        SessionSource::Cli,
        /*model_verbosity*/ None,
        /*enable_request_compression*/ false,
        /*include_timing_metrics*/ false,
        /*beta_features_header*/ None,
    )
}

fn test_model_client_with_runtime_authority(
    authority: RuntimeLeaseAuthority,
    base_url: &str,
) -> ModelClient {
    test_model_client_with_runtime_host(
        RuntimeLeaseHost::pooled_with_authority_for_test(
            RuntimeLeaseHostId::new("runtime-lease-test".to_string()),
            authority,
        ),
        base_url,
    )
}

fn test_model_client_with_runtime_host(host: RuntimeLeaseHost, base_url: &str) -> ModelClient {
    let provider = create_oss_provider_with_base_url(base_url, WireApi::Responses);
    let conversation_id = ThreadId::new();
    let session_id = conversation_id.to_string();
    ModelClient::new_with_runtime_lease(
        /*auth_manager*/ None,
        /*lease_auth*/ None,
        Some(host),
        Some(Arc::new(tokio::sync::Mutex::new(SessionLeaseView::new()))),
        session_id.clone(),
        Arc::new(CollaborationTreeBindingHandle::new(
            CollaborationTreeId::root_for_session(&session_id),
        )),
        conversation_id,
        /*installation_id*/ "11111111-1111-4111-8111-111111111111".to_string(),
        provider,
        SessionSource::Cli,
        /*model_verbosity*/ None,
        /*enable_request_compression*/ false,
        /*include_timing_metrics*/ false,
        /*beta_features_header*/ None,
    )
}

fn websocket_provider(base_url: &str) -> ModelProviderInfo {
    ModelProviderInfo {
        name: "mock-ws".into(),
        base_url: Some(format!("{base_url}/v1")),
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(0),
        stream_max_retries: Some(0),
        stream_idle_timeout_ms: Some(5_000),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: true,
    }
}

fn test_websocket_model_client_with_runtime_host(
    host: RuntimeLeaseHost,
    base_url: &str,
) -> ModelClient {
    let provider = websocket_provider(base_url);
    let conversation_id = ThreadId::new();
    let session_id = conversation_id.to_string();
    ModelClient::new_with_runtime_lease(
        /*auth_manager*/ None,
        /*lease_auth*/ None,
        Some(host),
        Some(Arc::new(tokio::sync::Mutex::new(SessionLeaseView::new()))),
        session_id.clone(),
        Arc::new(CollaborationTreeBindingHandle::new(
            CollaborationTreeId::root_for_session(&session_id),
        )),
        conversation_id,
        /*installation_id*/ "11111111-1111-4111-8111-111111111111".to_string(),
        provider,
        SessionSource::Cli,
        /*model_verbosity*/ None,
        /*enable_request_compression*/ false,
        /*include_timing_metrics*/ false,
        /*beta_features_header*/ None,
    )
}

fn test_websocket_model_client_with_runtime_authority(
    authority: RuntimeLeaseAuthority,
    base_url: &str,
) -> ModelClient {
    test_websocket_model_client_with_runtime_host(
        RuntimeLeaseHost::pooled_with_authority_for_test(
            RuntimeLeaseHostId::new("runtime-lease-test".to_string()),
            authority,
        ),
        base_url,
    )
}

async fn test_pooled_runtime_host_with_manager_owner(
    account_id: &str,
) -> anyhow::Result<(RuntimeLeaseHost, tempfile::TempDir)> {
    let codex_home = tempfile::tempdir()?;
    let state_db =
        codex_state::StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into())
            .await?;
    state_db
        .upsert_account_registry_entry(AccountRegistryEntryUpdate {
            account_id: account_id.to_string(),
            pool_id: "pool-main".to_string(),
            position: 0,
            account_kind: "chatgpt".to_string(),
            backend_family: "local".to_string(),
            workspace_id: Some("workspace-main".to_string()),
            enabled: true,
            healthy: true,
        })
        .await?;
    state_db
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("pool-main".to_string()),
            preferred_account_id: Some(account_id.to_string()),
            suppressed: false,
        })
        .await?;
    save_auth(
        &pooled_auth_home(codex_home.path(), account_id),
        &auth_dot_json_for_account(account_id),
        AuthCredentialsStoreMode::File,
    )?;
    let manager = crate::state::SessionServices::build_account_pool_manager(
        Some(state_db),
        Some(test_accounts_config()),
        codex_home.path().to_path_buf(),
        format!("holder-client-test-{account_id}"),
    )
    .await?
    .expect("test manager should build");
    let host = RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new(format!(
        "runtime-lease-{account_id}"
    )));
    host.install_manager_owner(Arc::clone(&manager))?;
    Ok((host, codex_home))
}

fn test_accounts_config() -> AccountsConfigToml {
    let mut pools = HashMap::new();
    pools.insert(
        "pool-main".to_string(),
        AccountPoolDefinitionToml {
            allow_context_reuse: Some(false),
            account_kinds: None,
        },
    );

    AccountsConfigToml {
        backend: None,
        default_pool: None,
        proactive_switch_threshold_percent: None,
        lease_ttl_secs: None,
        heartbeat_interval_secs: None,
        min_switch_interval_secs: None,
        allocation_mode: None,
        pools: Some(pools),
    }
}

fn pooled_auth_home(codex_home: &Path, account_id: &str) -> std::path::PathBuf {
    codex_home
        .join(".pooled-auth/backends/local/accounts")
        .join(account_id)
}

fn auth_dot_json_for_account(account_id: &str) -> AuthDotJson {
    let access_token = fake_access_token(account_id);
    AuthDotJson {
        auth_mode: Some(AuthMode::Chatgpt),
        openai_api_key: None,
        tokens: Some(TokenData {
            id_token: codex_login::token_data::parse_chatgpt_jwt_claims(&access_token)
                .expect("fake access token should parse"),
            access_token,
            refresh_token: "refresh-token".to_string(),
            account_id: Some(account_id.to_string()),
        }),
        last_refresh: Some(chrono::Utc::now()),
    }
}

fn fake_access_token(chatgpt_account_id: &str) -> String {
    let header = serde_json::json!({
        "alg": "none",
        "typ": "JWT",
    });
    let payload = serde_json::json!({
        "email": "user@example.com",
        "email_verified": true,
        "https://api.openai.com/auth": {
            "chatgpt_plan_type": "pro",
            "chatgpt_user_id": "user-12345",
            "chatgpt_account_id": chatgpt_account_id,
        },
    });
    let b64 = |value: serde_json::Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&value).expect("serialize fake JWT part"))
    };
    format!("{}.{}.sig", b64(header), b64(payload))
}

struct SnapshotOnlyLeaseScopedAuthSession {
    binding: LeaseAuthBinding,
    leased_calls: AtomicUsize,
    refresh_calls: AtomicUsize,
}

impl SnapshotOnlyLeaseScopedAuthSession {
    fn new(account_id: &str) -> Self {
        Self {
            binding: LeaseAuthBinding {
                account_id: account_id.to_string(),
                backend_account_handle: format!("handle-{account_id}"),
                lease_epoch: 1,
            },
            leased_calls: AtomicUsize::new(0),
            refresh_calls: AtomicUsize::new(0),
        }
    }
}

impl LeaseScopedAuthSession for SnapshotOnlyLeaseScopedAuthSession {
    fn leased_turn_auth(&self) -> anyhow::Result<LeasedTurnAuth> {
        self.leased_calls.fetch_add(1, Ordering::SeqCst);
        Ok(LeasedTurnAuth::new(
            CodexAuth::create_dummy_chatgpt_auth_for_testing(),
        ))
    }

    fn refresh_leased_turn_auth(&self) -> anyhow::Result<LeasedTurnAuth> {
        self.refresh_calls.fetch_add(1, Ordering::SeqCst);
        bail!("request path must use leased auth snapshots without refresh")
    }

    fn binding(&self) -> &LeaseAuthBinding {
        &self.binding
    }

    fn ensure_current(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn test_model_info() -> ModelInfo {
    serde_json::from_value(json!({
        "slug": "gpt-test",
        "display_name": "gpt-test",
        "description": "desc",
        "default_reasoning_level": "medium",
        "supported_reasoning_levels": [
            {"effort": "medium", "description": "medium"}
        ],
        "shell_type": "shell_command",
        "visibility": "list",
        "supported_in_api": true,
        "priority": 1,
        "upgrade": null,
        "base_instructions": "base instructions",
        "model_messages": null,
        "supports_reasoning_summaries": false,
        "support_verbosity": false,
        "default_verbosity": null,
        "apply_patch_tool_type": null,
        "truncation_policy": {"mode": "bytes", "limit": 10000},
        "supports_parallel_tool_calls": false,
        "supports_image_detail_original": false,
        "context_window": 272000,
        "auto_compact_token_limit": null,
        "experimental_supported_tools": []
    }))
    .expect("deserialize test model info")
}

fn test_session_telemetry() -> SessionTelemetry {
    SessionTelemetry::new(
        ThreadId::new(),
        "gpt-test",
        "gpt-test",
        /*account_id*/ None,
        /*account_email*/ None,
        /*auth_mode*/ None,
        "test-originator".to_string(),
        /*log_user_prompts*/ false,
        "test-terminal".to_string(),
        SessionSource::Cli,
    )
}

fn prompt_with_input(text: &str) -> Prompt {
    Prompt {
        input: vec![ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: text.to_string(),
            }],
            end_turn: None,
            phase: None,
        }],
        tools: Vec::new(),
        parallel_tool_calls: false,
        base_instructions: BaseInstructions {
            text: "base".to_string(),
        },
        personality: None,
        output_schema: None,
    }
}

async fn wait_for_admitted_count(authority: &RuntimeLeaseAuthority, expected: usize) {
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

async fn stream_until_complete(mut stream: ResponseStream) {
    while let Some(event) = stream.next().await {
        match event {
            Ok(ResponseEvent::Completed { .. }) => return,
            Ok(_) => {}
            Err(err) => panic!("stream should complete without error: {err:#}"),
        }
    }
    panic!("stream ended before completion");
}

#[test]
fn build_subagent_headers_sets_other_subagent_label() {
    let client = test_model_client(SessionSource::SubAgent(SubAgentSource::Other(
        "memory_consolidation".to_string(),
    )));
    let headers = client.build_subagent_headers();
    let value = headers
        .get(X_OPENAI_SUBAGENT_HEADER)
        .and_then(|value| value.to_str().ok());
    assert_eq!(value, Some("memory_consolidation"));
}

#[test]
fn build_ws_client_metadata_includes_window_lineage_and_turn_metadata() {
    let parent_thread_id = ThreadId::new();
    let client = test_model_client(SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
        parent_thread_id,
        depth: 2,
        agent_path: None,
        agent_nickname: None,
        agent_role: None,
    }));

    client.advance_window_generation();

    let client_metadata = client.build_ws_client_metadata(Some(r#"{"turn_id":"turn-123"}"#));
    let conversation_id = client.state.conversation_id;
    assert_eq!(
        client_metadata,
        std::collections::HashMap::from([
            (
                X_CODEX_INSTALLATION_ID_HEADER.to_string(),
                "11111111-1111-4111-8111-111111111111".to_string(),
            ),
            (
                X_CODEX_WINDOW_ID_HEADER.to_string(),
                format!("{conversation_id}:1"),
            ),
            (
                X_OPENAI_SUBAGENT_HEADER.to_string(),
                "collab_spawn".to_string(),
            ),
            (
                X_CODEX_PARENT_THREAD_ID_HEADER.to_string(),
                parent_thread_id.to_string(),
            ),
            (
                X_CODEX_TURN_METADATA_HEADER.to_string(),
                r#"{"turn_id":"turn-123"}"#.to_string(),
            ),
        ])
    );
}

#[tokio::test]
async fn summarize_memories_returns_empty_for_empty_input() {
    let client = test_model_client(SessionSource::Cli);
    let model_info = test_model_info();
    let session_telemetry = test_session_telemetry();

    let output = client
        .summarize_memories(
            Vec::new(),
            &model_info,
            /*effort*/ None,
            &session_telemetry,
        )
        .await
        .expect("empty summarize request should succeed");
    assert_eq!(output.len(), 0);
}

#[tokio::test]
async fn direct_request_setup_uses_leased_auth_snapshot_without_refresh() {
    let lease_auth = Arc::new(SessionLeaseAuth::default());
    let lease_session = Arc::new(SnapshotOnlyLeaseScopedAuthSession::new("account_id"));
    lease_auth.replace_current(Some(lease_session.clone()));
    let client = test_model_client_with_lease_auth(SessionSource::Cli, Some(lease_auth));

    let client_setup = client
        .current_client_setup()
        .await
        .expect("direct request setup should use the leased auth snapshot");

    assert_eq!(
        client_setup.api_auth.account_id.as_deref(),
        Some("account_id")
    );
    assert_eq!(lease_session.leased_calls.load(Ordering::SeqCst), 1);
    assert_eq!(lease_session.refresh_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn responses_http_setup_acquires_admission_for_pooled_runtime_host() {
    let authority = RuntimeLeaseAuthority::for_test_accepting("account_id", 7);
    let client =
        test_model_client_with_runtime_authority(authority.clone(), "https://example.com/v1");

    let setup = client
        .admitted_client_setup(
            RequestBoundaryKind::ResponsesHttp,
            Some("turn-1"),
            "request-1",
            CancellationToken::new(),
        )
        .await
        .expect("pooled runtime request should acquire admission");

    assert_eq!(
        setup.setup.api_auth.account_id.as_deref(),
        Some("account_id")
    );
    assert_eq!(
        authority.recorded_boundaries_for_test(),
        vec![RequestBoundaryKind::ResponsesHttp]
    );
    assert_eq!(authority.admitted_count_for_test(), 1);

    drop(setup);

    assert_eq!(authority.admitted_count_for_test(), 0);
}

#[tokio::test]
async fn responses_http_stream_acquires_admission_per_provider_round_trip() {
    core_test_support::skip_if_no_network!();

    let server = wiremock::MockServer::start().await;
    let response_mock = core_test_support::responses::mount_sse_sequence(
        &server,
        vec![
            core_test_support::responses::sse(vec![
                core_test_support::responses::ev_response_created("resp-1"),
                core_test_support::responses::ev_completed("resp-1"),
            ]),
            core_test_support::responses::sse(vec![
                core_test_support::responses::ev_response_created("resp-2"),
                core_test_support::responses::ev_completed("resp-2"),
            ]),
        ],
    )
    .await;
    let authority = RuntimeLeaseAuthority::for_test_accepting("account_id", 7);
    let client = test_model_client_with_runtime_authority(authority.clone(), &server.uri());
    let prompt = prompt_with_input("hello");

    for expected_round_trips in 1..=2 {
        let mut session = client.new_session();
        let stream = session
            .stream(
                &prompt,
                &test_model_info(),
                &test_session_telemetry(),
                /*effort*/ None,
                codex_protocol::config_types::ReasoningSummary::None,
                /*service_tier*/ None,
                /*turn_metadata_header*/ None,
            )
            .await
            .expect("HTTP stream should start");
        assert_eq!(authority.admitted_count_for_test(), 1);

        stream_until_complete(stream).await;
        wait_for_admitted_count(&authority, 0).await;
        assert_eq!(
            authority.recorded_boundaries_for_test().len(),
            expected_round_trips
        );
    }

    assert_eq!(
        authority.recorded_boundaries_for_test(),
        vec![
            RequestBoundaryKind::ResponsesHttp,
            RequestBoundaryKind::ResponsesHttp,
        ]
    );
    assert_eq!(response_mock.requests().len(), 2);
}

#[tokio::test]
async fn responses_http_streaming_admission_is_held_until_completion_then_released_once() {
    core_test_support::skip_if_no_network!();

    let server = wiremock::MockServer::start().await;
    core_test_support::responses::mount_sse_once(
        &server,
        core_test_support::responses::sse(vec![
            core_test_support::responses::ev_response_created("resp-1"),
            core_test_support::responses::ev_completed("resp-1"),
        ]),
    )
    .await;
    let authority = RuntimeLeaseAuthority::for_test_accepting("account_id", 7);
    let client = test_model_client_with_runtime_authority(authority.clone(), &server.uri());
    let prompt = prompt_with_input("hello");
    let mut session = client.new_session();

    let stream = session
        .stream(
            &prompt,
            &test_model_info(),
            &test_session_telemetry(),
            /*effort*/ None,
            codex_protocol::config_types::ReasoningSummary::None,
            /*service_tier*/ None,
            /*turn_metadata_header*/ None,
        )
        .await
        .expect("HTTP stream should start");

    assert_eq!(authority.admitted_count_for_test(), 1);

    stream_until_complete(stream).await;
    wait_for_admitted_count(&authority, 0).await;
    assert_eq!(
        authority.recorded_boundaries_for_test(),
        vec![RequestBoundaryKind::ResponsesHttp]
    );
}

#[tokio::test]
async fn responses_http_streaming_admission_releases_once_on_drop() {
    core_test_support::skip_if_no_network!();

    let (tx_complete, rx_complete) = tokio::sync::oneshot::channel();
    let (server, _completions) =
        core_test_support::streaming_sse::start_streaming_sse_server(vec![vec![
            core_test_support::streaming_sse::StreamingSseChunk {
                gate: None,
                body: core_test_support::responses::sse(vec![
                    core_test_support::responses::ev_response_created("resp-1"),
                ]),
            },
            core_test_support::streaming_sse::StreamingSseChunk {
                gate: Some(rx_complete),
                body: core_test_support::responses::sse(vec![
                    core_test_support::responses::ev_completed("resp-1"),
                ]),
            },
        ]])
        .await;
    let authority = RuntimeLeaseAuthority::for_test_accepting("account_id", 7);
    let base_url = format!("{}/v1", server.uri());
    let client = test_model_client_with_runtime_authority(authority.clone(), &base_url);
    let prompt = prompt_with_input("hello");
    let mut session = client.new_session();
    let mut stream = session
        .stream(
            &prompt,
            &test_model_info(),
            &test_session_telemetry(),
            /*effort*/ None,
            codex_protocol::config_types::ReasoningSummary::None,
            /*service_tier*/ None,
            /*turn_metadata_header*/ None,
        )
        .await
        .expect("HTTP stream should start");

    assert_eq!(authority.admitted_count_for_test(), 1);
    match stream.next().await {
        Some(Ok(_)) => {}
        other => panic!("expected first streamed event before drop, got {other:?}"),
    }

    drop(stream);
    wait_for_admitted_count(&authority, 0).await;

    let _ = tx_complete.send(());
    server.shutdown().await;
}

#[tokio::test]
async fn admitted_client_setup_requires_pooled_authority_when_runtime_host_is_pooled() {
    let client = test_model_client_with_runtime_lease_view(/*allow_context_reuse*/ false);

    let err = match client
        .admitted_client_setup(
            RequestBoundaryKind::ResponsesCompact,
            /*turn_id*/ None,
            "responses-compact",
            CancellationToken::new(),
        )
        .await
    {
        Ok(_) => panic!("pooled runtime host without authority must fail closed"),
        Err(err) => err,
    };

    assert!(
        err.to_string().contains(
            "pooled runtime lease host runtime-lease-test is missing published authority"
        )
    );
}

#[tokio::test]
async fn compact_conversation_history_uses_responses_compact_admission() {
    let server = wiremock::MockServer::start().await;
    let compact_mock = core_test_support::responses::mount_compact_json_once(
        &server,
        json!({
            "output": []
        }),
    )
    .await;
    let authority = RuntimeLeaseAuthority::for_test_accepting("account_id", 7);
    let client = test_model_client_with_runtime_authority(authority.clone(), &server.uri());
    let prompt = Prompt {
        input: vec![ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "compact me".to_string(),
            }],
            end_turn: None,
            phase: None,
        }],
        tools: Vec::new(),
        parallel_tool_calls: false,
        base_instructions: BaseInstructions {
            text: "base".to_string(),
        },
        personality: None,
        output_schema: None,
    };

    let output = client
        .compact_conversation_history(
            &prompt,
            &test_model_info(),
            /*effort*/ None,
            codex_protocol::config_types::ReasoningSummary::None,
            &test_session_telemetry(),
            /*account_id_override*/ None,
        )
        .await
        .expect("compact request should succeed");

    assert_eq!(output, Vec::<ResponseItem>::new());
    assert_eq!(
        authority.recorded_boundaries_for_test(),
        vec![RequestBoundaryKind::ResponsesCompact]
    );
    assert_eq!(
        compact_mock
            .single_request()
            .header("chatgpt-account-id")
            .as_deref(),
        Some("account_id")
    );
}

#[tokio::test]
async fn compact_conversation_history_ignores_mismatched_account_override_for_pooled_admission()
-> anyhow::Result<()> {
    let server = wiremock::MockServer::start().await;
    let compact_mock = core_test_support::responses::mount_compact_json_once(
        &server,
        json!({
            "output": []
        }),
    )
    .await;
    let (runtime_host, _codex_home) =
        test_pooled_runtime_host_with_manager_owner("acct-compact-a").await?;
    let client = test_model_client_with_runtime_host(runtime_host, &server.uri());
    let prompt = Prompt {
        input: vec![ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "compact me".to_string(),
            }],
            end_turn: None,
            phase: None,
        }],
        tools: Vec::new(),
        parallel_tool_calls: false,
        base_instructions: BaseInstructions {
            text: "base".to_string(),
        },
        personality: None,
        output_schema: None,
    };

    let output = client
        .compact_conversation_history(
            &prompt,
            &test_model_info(),
            /*effort*/ None,
            codex_protocol::config_types::ReasoningSummary::None,
            &test_session_telemetry(),
            Some("acct-turn-override".to_string()),
        )
        .await?;

    assert_eq!(output, Vec::<ResponseItem>::new());
    assert_eq!(
        compact_mock
            .single_request()
            .header("chatgpt-account-id")
            .as_deref(),
        Some("acct-compact-a")
    );
    Ok(())
}

#[tokio::test]
async fn summarize_memories_uses_memory_summary_admission() {
    use wiremock::Mock;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path_regex;

    let server = wiremock::MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(".*/memories/trace_summarize$"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
                    "output": []
                })),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let authority = RuntimeLeaseAuthority::for_test_accepting("account_id", 7);
    let client = test_model_client_with_runtime_authority(authority.clone(), &server.uri());

    let output = client
        .summarize_memories(
            vec![RawMemory {
                id: "trace-1".to_string(),
                metadata: RawMemoryMetadata {
                    source_path: "/tmp/trace.json".to_string(),
                },
                items: vec![json!({"type": "message", "role": "user", "content": []})],
            }],
            &test_model_info(),
            /*effort*/ None,
            &test_session_telemetry(),
        )
        .await
        .expect("memory summary request should succeed");

    assert_eq!(output, Vec::new());
    assert_eq!(
        authority.recorded_boundaries_for_test(),
        vec![RequestBoundaryKind::MemorySummary]
    );
}

#[tokio::test]
async fn create_realtime_call_uses_realtime_admission() {
    use wiremock::Mock;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path_regex;

    let server = wiremock::MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(".*/realtime/calls$"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("location", "/v1/realtime/calls/rtc_test")
                .set_body_string("sdp-answer"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let authority = RuntimeLeaseAuthority::for_test_accepting("account_id", 7);
    let client = test_model_client_with_runtime_authority(authority.clone(), &server.uri());

    let call = client
        .create_realtime_call_with_headers(
            "sdp-offer".to_string(),
            RealtimeSessionConfig {
                instructions: String::new(),
                model: Some("gpt-realtime-test".to_string()),
                session_id: Some("session-1".to_string()),
                event_parser: RealtimeEventParser::RealtimeV2,
                session_mode: RealtimeSessionMode::Conversational,
                output_modality: RealtimeOutputModality::Text,
                voice: RealtimeVoice::Alloy,
            },
            http::HeaderMap::new(),
        )
        .await
        .expect("realtime call should succeed");

    assert_eq!(call.call_id, "rtc_test");
    assert_eq!(
        authority.recorded_boundaries_for_test(),
        vec![RequestBoundaryKind::Realtime]
    );
}

#[tokio::test]
async fn lease_view_reset_uses_existing_model_client_reset_boundary() {
    let client = test_model_client_with_runtime_lease_view(/*allow_context_reuse*/ false);
    let before = client.remote_session_id();

    client
        .apply_test_lease_snapshot("acct-a", 1, Some("turn-1"), "turn-1")
        .await;
    client
        .apply_test_lease_snapshot("acct-b", 2, Some("turn-2"), "turn-2")
        .await;

    assert_ne!(client.remote_session_id(), before);
    assert_eq!(client.cached_websocket_session_for_test().connection, None);
    assert_eq!(
        client.latest_remote_context_reset_for_test(),
        Some(RemoteContextResetRecord {
            session_id: client.session_id_for_test(),
            turn_id: Some("turn-2".to_string()),
            request_id: "turn-2".to_string(),
            lease_generation: 2,
            transport_reset_generation: client.current_window_generation(),
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_prewarm_releases_handshake_admission_when_idle_websocket_is_cached() {
    core_test_support::skip_if_no_network!();

    let server = core_test_support::responses::start_websocket_server(vec![vec![vec![
        core_test_support::responses::ev_response_created("warm-1"),
        core_test_support::responses::ev_completed("warm-1"),
    ]]])
    .await;
    let authority = RuntimeLeaseAuthority::for_test_accepting("account_id", 7);
    let client =
        test_websocket_model_client_with_runtime_authority(authority.clone(), server.uri());
    let prompt = prompt_with_input("hello");

    {
        let mut session = client.new_session();
        session
            .prewarm_websocket(
                &prompt,
                &test_model_info(),
                &test_session_telemetry(),
                /*effort*/ None,
                codex_protocol::config_types::ReasoningSummary::None,
                /*service_tier*/ None,
                /*turn_metadata_header*/ None,
            )
            .await
            .expect("websocket prewarm should succeed");
        wait_for_admitted_count(&authority, 0).await;
    }

    assert_eq!(
        authority.recorded_boundaries_for_test(),
        vec![RequestBoundaryKind::ResponsesWebSocketPrewarm]
    );
    assert_eq!(
        client.cached_websocket_session_for_test().connection,
        Some(())
    );

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_preconnect_releases_handshake_admission_when_idle_websocket_is_cached() {
    core_test_support::skip_if_no_network!();

    let server = core_test_support::responses::start_websocket_server(vec![vec![vec![]]]).await;
    let authority = RuntimeLeaseAuthority::for_test_accepting("account_id", 7);
    let client =
        test_websocket_model_client_with_runtime_authority(authority.clone(), server.uri());

    {
        let mut session = client.new_session();
        session
            .preconnect_websocket(&test_session_telemetry(), &test_model_info())
            .await
            .expect("websocket preconnect should succeed");
        wait_for_admitted_count(&authority, 0).await;
    }

    assert_eq!(
        authority.recorded_boundaries_for_test(),
        vec![RequestBoundaryKind::ResponsesWebSocketPrewarm]
    );
    assert_eq!(
        client.cached_websocket_session_for_test().connection,
        Some(())
    );

    drop(client);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cached_websocket_is_discarded_when_admitted_generation_changes() {
    core_test_support::skip_if_no_network!();

    let server =
        core_test_support::responses::start_websocket_server(vec![vec![vec![]], vec![vec![]]])
            .await;
    let authority = RuntimeLeaseAuthority::for_test_accepting("account_id", 7);
    let client =
        test_websocket_model_client_with_runtime_authority(authority.clone(), server.uri());

    {
        let mut session = client.new_session();
        session
            .preconnect_websocket(&test_session_telemetry(), &test_model_info())
            .await
            .expect("first preconnect should succeed");
    }
    assert_eq!(
        client.cached_websocket_session_for_test().connection,
        Some(())
    );
    authority
        .install_replacement_for_test("account_id", 8)
        .await;

    {
        let mut session = client.new_session();
        session
            .preconnect_websocket(&test_session_telemetry(), &test_model_info())
            .await
            .expect("replacement preconnect should succeed");
    }

    assert_eq!(server.handshakes().len(), 2);
    assert_eq!(
        authority.recorded_boundaries_for_test(),
        vec![
            RequestBoundaryKind::ResponsesWebSocketPrewarm,
            RequestBoundaryKind::ResponsesWebSocketPrewarm,
        ]
    );

    drop(client);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_session_is_discarded_when_transport_reset_generation_changes() {
    core_test_support::skip_if_no_network!();

    let server =
        core_test_support::responses::start_websocket_server(vec![vec![vec![]], vec![vec![]]])
            .await;
    let authority = RuntimeLeaseAuthority::for_test_accepting("account_id", 7);
    let client = test_websocket_model_client_with_runtime_authority(authority, server.uri());
    let mut session = client.new_session();

    session
        .preconnect_websocket(&test_session_telemetry(), &test_model_info())
        .await
        .expect("first preconnect should succeed");
    assert_eq!(session.websocket_session.lease_generation, Some(7));
    assert_eq!(
        session.websocket_session.transport_reset_generation,
        Some(client.current_window_generation())
    );

    client.advance_window_generation();

    session
        .preconnect_websocket(&test_session_telemetry(), &test_model_info())
        .await
        .expect("replacement preconnect should succeed");

    assert_eq!(server.handshakes().len(), 2);
    assert_eq!(
        session.websocket_session.transport_reset_generation,
        Some(client.current_window_generation())
    );

    drop(session);
    drop(client);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_streaming_admission_is_held_until_completion_then_released_once() {
    core_test_support::skip_if_no_network!();

    let server = core_test_support::responses::start_websocket_server(vec![vec![vec![
        core_test_support::responses::ev_response_created("resp-1"),
        core_test_support::responses::ev_completed("resp-1"),
    ]]])
    .await;
    let authority = RuntimeLeaseAuthority::for_test_accepting("account_id", 7);
    let client =
        test_websocket_model_client_with_runtime_authority(authority.clone(), server.uri());
    let prompt = prompt_with_input("hello");
    let mut session = client.new_session();

    let stream = session
        .stream(
            &prompt,
            &test_model_info(),
            &test_session_telemetry(),
            /*effort*/ None,
            codex_protocol::config_types::ReasoningSummary::None,
            /*service_tier*/ None,
            /*turn_metadata_header*/ None,
        )
        .await
        .expect("websocket stream should start");

    assert_eq!(authority.admitted_count_for_test(), 1);

    stream_until_complete(stream).await;
    wait_for_admitted_count(&authority, 0).await;
    assert_eq!(
        authority.recorded_boundaries_for_test(),
        vec![RequestBoundaryKind::ResponsesWebSocket]
    );

    drop(client);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_streaming_admission_releases_once_on_drop() {
    core_test_support::skip_if_no_network!();

    let server = core_test_support::responses::start_websocket_server_with_headers(vec![
        core_test_support::responses::WebSocketConnectionConfig {
            requests: vec![vec![core_test_support::responses::ev_response_created(
                "resp-1",
            )]],
            response_headers: Vec::new(),
            accept_delay: None,
            close_after_requests: false,
        },
    ])
    .await;
    let authority = RuntimeLeaseAuthority::for_test_accepting("account_id", 7);
    let client =
        test_websocket_model_client_with_runtime_authority(authority.clone(), server.uri());
    let prompt = prompt_with_input("hello");
    let mut session = client.new_session();
    let mut stream = session
        .stream(
            &prompt,
            &test_model_info(),
            &test_session_telemetry(),
            /*effort*/ None,
            codex_protocol::config_types::ReasoningSummary::None,
            /*service_tier*/ None,
            /*turn_metadata_header*/ None,
        )
        .await
        .expect("websocket stream should start");

    assert_eq!(authority.admitted_count_for_test(), 1);
    assert!(matches!(
        stream.next().await,
        Some(Ok(ResponseEvent::Created))
    ));

    drop(stream);
    wait_for_admitted_count(&authority, 0).await;

    drop(client);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_websocket_stream_forces_fresh_handshake_before_reuse() {
    core_test_support::skip_if_no_network!();

    let server = core_test_support::responses::start_websocket_server_with_headers(vec![
        core_test_support::responses::WebSocketConnectionConfig {
            requests: vec![vec![core_test_support::responses::ev_response_created(
                "resp-1",
            )]],
            response_headers: Vec::new(),
            accept_delay: None,
            close_after_requests: false,
        },
        core_test_support::responses::WebSocketConnectionConfig {
            requests: vec![vec![
                core_test_support::responses::ev_response_created("resp-2"),
                core_test_support::responses::ev_completed("resp-2"),
            ]],
            response_headers: Vec::new(),
            accept_delay: None,
            close_after_requests: true,
        },
    ])
    .await;
    let authority = RuntimeLeaseAuthority::for_test_accepting("account_id", 7);
    let client =
        test_websocket_model_client_with_runtime_authority(authority.clone(), server.uri());
    let prompt = prompt_with_input("hello");
    let mut session = client.new_session();
    let mut first_stream = session
        .stream(
            &prompt,
            &test_model_info(),
            &test_session_telemetry(),
            /*effort*/ None,
            codex_protocol::config_types::ReasoningSummary::None,
            /*service_tier*/ None,
            /*turn_metadata_header*/ None,
        )
        .await
        .expect("first websocket stream should start");

    assert!(matches!(
        first_stream.next().await,
        Some(Ok(ResponseEvent::Created))
    ));
    drop(first_stream);
    wait_for_admitted_count(&authority, 0).await;

    let second_stream = session
        .stream(
            &prompt,
            &test_model_info(),
            &test_session_telemetry(),
            /*effort*/ None,
            codex_protocol::config_types::ReasoningSummary::None,
            /*service_tier*/ None,
            /*turn_metadata_header*/ None,
        )
        .await
        .expect("replacement websocket stream should start");

    stream_until_complete(second_stream).await;
    assert!(
        server
            .wait_for_handshakes(/*expected*/ 2, Duration::from_secs(2))
            .await
    );
    assert_eq!(server.handshakes().len(), 2);
    assert_eq!(
        authority.recorded_boundaries_for_test(),
        vec![
            RequestBoundaryKind::ResponsesWebSocket,
            RequestBoundaryKind::ResponsesWebSocket,
        ]
    );

    drop(session);
    drop(client);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_streaming_admission_releases_once_on_transport_failure() {
    core_test_support::skip_if_no_network!();

    let server = core_test_support::responses::start_websocket_server(vec![vec![vec![
        core_test_support::responses::ev_response_created("resp-1"),
    ]]])
    .await;
    let authority = RuntimeLeaseAuthority::for_test_accepting("account_id", 7);
    let client =
        test_websocket_model_client_with_runtime_authority(authority.clone(), server.uri());
    let prompt = prompt_with_input("hello");
    let mut session = client.new_session();
    let mut stream = session
        .stream(
            &prompt,
            &test_model_info(),
            &test_session_telemetry(),
            /*effort*/ None,
            codex_protocol::config_types::ReasoningSummary::None,
            /*service_tier*/ None,
            /*turn_metadata_header*/ None,
        )
        .await
        .expect("websocket stream should start");

    assert!(matches!(
        stream.next().await,
        Some(Ok(ResponseEvent::Created))
    ));
    assert!(matches!(stream.next().await, Some(Err(_))));
    wait_for_admitted_count(&authority, 0).await;

    drop(client);
    server.shutdown().await;
}

#[tokio::test]
async fn auth_recovery_retry_reacquires_fresh_admission_and_reporter() {
    core_test_support::skip_if_no_network!();

    let server = wiremock::MockServer::start().await;
    let response_mock = core_test_support::responses::mount_response_sequence(
        &server,
        vec![
            wiremock::ResponseTemplate::new(401)
                .insert_header("content-type", "application/json")
                .set_body_string("unauthorized"),
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(core_test_support::responses::sse(vec![
                    core_test_support::responses::ev_response_created("resp-1"),
                    core_test_support::responses::ev_completed("resp-1"),
                ])),
        ],
    )
    .await;
    let authority = RuntimeLeaseAuthority::for_test_accepting("account_id", 7);
    let client = test_model_client_with_runtime_authority(authority.clone(), &server.uri());
    let prompt = prompt_with_input("hello");
    let mut session = client.new_session();

    let stream = session
        .stream(
            &prompt,
            &test_model_info(),
            &test_session_telemetry(),
            /*effort*/ None,
            codex_protocol::config_types::ReasoningSummary::None,
            /*service_tier*/ None,
            /*turn_metadata_header*/ None,
        )
        .await
        .expect("HTTP retry stream should start");

    stream_until_complete(stream).await;
    wait_for_admitted_count(&authority, 0).await;

    assert_eq!(response_mock.requests().len(), 2);
    assert_eq!(
        authority.recorded_boundaries_for_test(),
        vec![
            RequestBoundaryKind::ResponsesHttp,
            RequestBoundaryKind::ResponsesHttp,
        ]
    );
}

#[test]
fn auth_request_telemetry_context_tracks_attached_auth_and_retry_phase() {
    let auth_context = AuthRequestTelemetryContext::new(
        Some(AuthMode::Chatgpt),
        &CoreAuthProvider::for_test(Some("access-token"), Some("workspace-123")),
        PendingUnauthorizedRetry::from_recovery(UnauthorizedRecoveryExecution {
            mode: "managed",
            phase: "refresh_token",
        }),
    );

    assert_eq!(auth_context.auth_mode, Some("Chatgpt"));
    assert!(auth_context.auth_header_attached);
    assert_eq!(auth_context.auth_header_name, Some("authorization"));
    assert!(auth_context.retry_after_unauthorized);
    assert_eq!(auth_context.recovery_mode, Some("managed"));
    assert_eq!(auth_context.recovery_phase, Some("refresh_token"));
}
