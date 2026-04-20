use super::AuthRequestTelemetryContext;
use super::ModelClient;
use super::PendingUnauthorizedRetry;
use super::UnauthorizedRecoveryExecution;
use super::X_CODEX_INSTALLATION_ID_HEADER;
use super::X_CODEX_PARENT_THREAD_ID_HEADER;
use super::X_CODEX_TURN_METADATA_HEADER;
use super::X_CODEX_WINDOW_ID_HEADER;
use super::X_OPENAI_SUBAGENT_HEADER;
use crate::lease_auth::SessionLeaseAuth;
use crate::runtime_lease::CollaborationTreeBindingHandle;
use crate::runtime_lease::CollaborationTreeId;
use crate::runtime_lease::RemoteContextResetRecord;
use crate::runtime_lease::RuntimeLeaseHost;
use crate::runtime_lease::RuntimeLeaseHostId;
use crate::runtime_lease::SessionLeaseView;
use anyhow::bail;
use codex_api::CoreAuthProvider;
use codex_app_server_protocol::AuthMode;
use codex_login::CodexAuth;
use codex_login::auth::LeaseAuthBinding;
use codex_login::auth::LeaseScopedAuthSession;
use codex_login::auth::LeasedTurnAuth;
use codex_model_provider_info::WireApi;
use codex_model_provider_info::create_oss_provider_with_base_url;
use codex_otel::SessionTelemetry;
use codex_protocol::ThreadId;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

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
