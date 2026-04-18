use super::connection_handling_websocket::DEFAULT_READ_TIMEOUT;
use anyhow::Result;
use app_test_support::ChatGptAuthFixture;
use app_test_support::McpProcess;
use app_test_support::create_mock_responses_server_sequence_unchecked;
use app_test_support::to_response;
use app_test_support::write_chatgpt_auth;
use chrono::DateTime;
use chrono::Utc;
use codex_app_server::INVALID_PARAMS_ERROR_CODE;
use codex_app_server_protocol::AccountOperationalState;
use codex_app_server_protocol::AccountPoolAccountsListParams;
use codex_app_server_protocol::AccountPoolAccountsListResponse;
use codex_app_server_protocol::AccountPoolDiagnosticsReadParams;
use codex_app_server_protocol::AccountPoolDiagnosticsReadResponse;
use codex_app_server_protocol::AccountPoolDiagnosticsStatus;
use codex_app_server_protocol::AccountPoolEventType;
use codex_app_server_protocol::AccountPoolEventsListParams;
use codex_app_server_protocol::AccountPoolEventsListResponse;
use codex_app_server_protocol::AccountPoolReadParams;
use codex_app_server_protocol::AccountPoolReadResponse;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_config::types::AuthCredentialsStoreMode;
use codex_state::AccountPoolEventRecord;
use codex_state::LegacyAccountImport;
use codex_state::StateRuntime;
use pretty_assertions::assert_eq;
use serde::Serialize;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::time::timeout;

const LEGACY_DEFAULT_POOL_ID: &str = "legacy-default";
const MISSING_POOL_ID: &str = "missing-pool";
const NOT_FOUND_ERROR_CODE: i64 = -32004;
const PRIMARY_ACCOUNT_ID: &str = "acct-1";
const SECONDARY_ACCOUNT_ID: &str = "acct-2";

#[tokio::test]
async fn account_pool_read_returns_summary_for_known_pool() -> Result<()> {
    let codex_home = TempDir::new()?;
    seed_two_accounts(codex_home.path()).await?;
    let mut mcp = initialized_mcp(codex_home.path()).await?;

    let response: AccountPoolReadResponse = send_account_pool_request(
        &mut mcp,
        "accountPool/read",
        AccountPoolReadParams {
            pool_id: LEGACY_DEFAULT_POOL_ID.to_string(),
        },
    )
    .await?;

    assert_eq!(response.pool_id, LEGACY_DEFAULT_POOL_ID);
    assert_eq!(response.summary.total_accounts, 2);
    assert_eq!(response.summary.active_leases, 0);
    assert_eq!(response.summary.available_accounts, Some(2));
    assert_eq!(response.summary.paused_accounts, None);
    assert_eq!(response.policy.allocation_mode, "exclusive");
    assert_eq!(response.policy.allow_context_reuse, false);
    assert_eq!(response.policy.proactive_switch_threshold_percent, Some(91));
    assert_eq!(response.policy.min_switch_interval_secs, Some(7));

    Ok(())
}

#[tokio::test]
async fn account_pool_accounts_list_returns_accounts_for_known_pool() -> Result<()> {
    let codex_home = TempDir::new()?;
    let runtime = seed_two_accounts(codex_home.path()).await?;
    runtime
        .set_account_enabled(SECONDARY_ACCOUNT_ID, false)
        .await?;
    let mut mcp = initialized_mcp(codex_home.path()).await?;

    let response: AccountPoolAccountsListResponse = send_account_pool_request(
        &mut mcp,
        "accountPool/accounts/list",
        AccountPoolAccountsListParams {
            pool_id: LEGACY_DEFAULT_POOL_ID.to_string(),
            cursor: None,
            limit: None,
            states: Some(vec![AccountOperationalState::Available]),
            account_kinds: Some(vec!["chatgpt".to_string()]),
        },
    )
    .await?;

    assert_eq!(response.next_cursor, None);
    assert_eq!(response.data.len(), 1);
    assert_eq!(response.data[0].account_id, PRIMARY_ACCOUNT_ID);
    assert_eq!(
        response.data[0].operational_state,
        Some(AccountOperationalState::Available)
    );
    assert_eq!(response.data[0].current_lease, None);
    assert_eq!(response.data[0].quota, None);
    assert_eq!(
        response.data[0]
            .selection
            .as_ref()
            .map(|selection| selection.eligible),
        Some(true)
    );

    Ok(())
}

#[tokio::test]
async fn account_pool_read_rejects_unknown_pool() -> Result<()> {
    let codex_home = TempDir::new()?;
    seed_two_accounts(codex_home.path()).await?;
    let mut mcp = initialized_mcp(codex_home.path()).await?;

    let error = send_account_pool_error(
        &mut mcp,
        "accountPool/read",
        AccountPoolReadParams {
            pool_id: MISSING_POOL_ID.to_string(),
        },
    )
    .await?;

    assert_eq!(error.error.code, NOT_FOUND_ERROR_CODE);
    assert!(
        error.error.message.contains(MISSING_POOL_ID),
        "unexpected error: {error:?}"
    );

    Ok(())
}

#[tokio::test]
async fn account_pool_read_rejects_unconfigured_pool_even_with_state() -> Result<()> {
    let codex_home = TempDir::new()?;
    seed_two_accounts(codex_home.path()).await?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    create_config_toml_without_accounts(codex_home.path(), &server.uri())?;
    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    mcp.initialize().await?;

    let error = send_account_pool_error(
        &mut mcp,
        "accountPool/read",
        AccountPoolReadParams {
            pool_id: LEGACY_DEFAULT_POOL_ID.to_string(),
        },
    )
    .await?;

    assert_eq!(error.error.code, NOT_FOUND_ERROR_CODE);
    assert!(
        error.error.message.contains(LEGACY_DEFAULT_POOL_ID),
        "unexpected error: {error:?}"
    );

    Ok(())
}

#[tokio::test]
async fn account_pool_read_succeeds_with_multiple_loaded_threads() -> Result<()> {
    let codex_home = TempDir::new()?;
    seed_two_accounts(codex_home.path()).await?;
    let mut mcp = initialized_mcp(codex_home.path()).await?;

    let first_id = mcp
        .send_thread_start_request(ThreadStartParams::default())
        .await?;
    let _: ThreadStartResponse = to_response(
        timeout(
            DEFAULT_READ_TIMEOUT,
            mcp.read_stream_until_response_message(RequestId::Integer(first_id)),
        )
        .await??,
    )?;

    let second_id = mcp
        .send_thread_start_request(ThreadStartParams::default())
        .await?;
    let _: ThreadStartResponse = to_response(
        timeout(
            DEFAULT_READ_TIMEOUT,
            mcp.read_stream_until_response_message(RequestId::Integer(second_id)),
        )
        .await??,
    )?;

    let response: AccountPoolReadResponse = send_account_pool_request(
        &mut mcp,
        "accountPool/read",
        AccountPoolReadParams {
            pool_id: LEGACY_DEFAULT_POOL_ID.to_string(),
        },
    )
    .await?;

    assert_eq!(response.pool_id, LEGACY_DEFAULT_POOL_ID);
    assert_eq!(response.summary.total_accounts, 2);

    Ok(())
}

#[tokio::test]
async fn account_pool_events_list_paginates_with_cursor_only() -> Result<()> {
    let codex_home = TempDir::new()?;
    let runtime = seed_two_accounts(codex_home.path()).await?;
    runtime
        .append_account_pool_event(test_event("evt-1", 4_000_000_001))
        .await?;
    runtime
        .append_account_pool_event(test_event("evt-2", 4_000_000_002))
        .await?;
    let mut mcp = initialized_mcp(codex_home.path()).await?;

    let first_page: AccountPoolEventsListResponse = send_account_pool_request(
        &mut mcp,
        "accountPool/events/list",
        AccountPoolEventsListParams {
            pool_id: LEGACY_DEFAULT_POOL_ID.to_string(),
            account_id: None,
            types: Some(vec![AccountPoolEventType::LeaseAcquired]),
            cursor: None,
            limit: Some(1),
        },
    )
    .await?;

    assert_eq!(first_page.data.len(), 1);
    assert_eq!(first_page.data[0].event_id, "evt-2");
    assert_eq!(first_page.data[0].details, Some(json!({"source": "test"})));
    let cursor = first_page
        .next_cursor
        .expect("first page should have a cursor");

    let second_page: AccountPoolEventsListResponse = send_account_pool_request(
        &mut mcp,
        "accountPool/events/list",
        AccountPoolEventsListParams {
            pool_id: LEGACY_DEFAULT_POOL_ID.to_string(),
            account_id: None,
            types: Some(vec![AccountPoolEventType::LeaseAcquired]),
            cursor: Some(cursor),
            limit: Some(1),
        },
    )
    .await?;

    assert_eq!(second_page.data.len(), 1);
    assert_eq!(second_page.data[0].event_id, "evt-1");
    assert_eq!(second_page.next_cursor, None);

    Ok(())
}

#[tokio::test]
async fn account_pool_events_list_rejects_invalid_cursor() -> Result<()> {
    let codex_home = TempDir::new()?;
    seed_two_accounts(codex_home.path()).await?;
    let mut mcp = initialized_mcp(codex_home.path()).await?;

    let error = send_account_pool_error(
        &mut mcp,
        "accountPool/events/list",
        AccountPoolEventsListParams {
            pool_id: LEGACY_DEFAULT_POOL_ID.to_string(),
            account_id: None,
            types: None,
            cursor: Some("not-a-cursor".to_string()),
            limit: Some(1),
        },
    )
    .await?;

    assert_eq!(error.error.code, INVALID_PARAMS_ERROR_CODE);
    assert!(
        error.error.message.contains("cursor"),
        "unexpected error: {error:?}"
    );

    Ok(())
}

#[tokio::test]
async fn account_pool_diagnostics_read_returns_derived_status_for_known_pool() -> Result<()> {
    let codex_home = TempDir::new()?;
    seed_two_accounts(codex_home.path()).await?;
    let mut mcp = initialized_mcp(codex_home.path()).await?;

    let response: AccountPoolDiagnosticsReadResponse = send_account_pool_request(
        &mut mcp,
        "accountPool/diagnostics/read",
        AccountPoolDiagnosticsReadParams {
            pool_id: LEGACY_DEFAULT_POOL_ID.to_string(),
        },
    )
    .await?;

    assert_eq!(response.pool_id, LEGACY_DEFAULT_POOL_ID);
    assert_eq!(response.status, AccountPoolDiagnosticsStatus::Healthy);
    assert_eq!(response.issues, Vec::new());

    Ok(())
}

async fn send_account_pool_request<TParams, TResponse>(
    mcp: &mut McpProcess,
    method: &str,
    params: TParams,
) -> Result<TResponse>
where
    TParams: Serialize,
    TResponse: serde::de::DeserializeOwned,
{
    let request_id = mcp
        .send_raw_request(method, Some(serde_json::to_value(params)?))
        .await?;
    let response: JSONRPCResponse = mcp
        .read_stream_until_response_message(RequestId::Integer(request_id))
        .await?;
    to_response(response)
}

async fn send_account_pool_error<TParams>(
    mcp: &mut McpProcess,
    method: &str,
    params: TParams,
) -> Result<JSONRPCError>
where
    TParams: Serialize,
{
    let request_id = mcp
        .send_raw_request(method, Some(serde_json::to_value(params)?))
        .await?;
    let error = mcp
        .read_stream_until_error_message(RequestId::Integer(request_id))
        .await?;
    Ok(error)
}

async fn initialized_mcp(codex_home: &std::path::Path) -> Result<McpProcess> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    create_pooled_config_toml(codex_home, &server.uri())?;

    let mut mcp = McpProcess::new_with_env(codex_home, &[("OPENAI_API_KEY", None)]).await?;
    mcp.initialize().await?;
    Ok(mcp)
}

async fn seed_default_pool_state(codex_home: &std::path::Path) -> Result<Arc<StateRuntime>> {
    let runtime = StateRuntime::init(codex_home.to_path_buf(), "mock_provider".to_string()).await?;
    runtime
        .import_legacy_default_account(LegacyAccountImport {
            account_id: PRIMARY_ACCOUNT_ID.to_string(),
        })
        .await?;
    write_pooled_auth(codex_home, PRIMARY_ACCOUNT_ID, PRIMARY_ACCOUNT_ID)?;
    Ok(runtime)
}

async fn seed_two_accounts(codex_home: &std::path::Path) -> Result<Arc<StateRuntime>> {
    let runtime = seed_default_pool_state(codex_home).await?;
    runtime
        .import_legacy_default_account(LegacyAccountImport {
            account_id: SECONDARY_ACCOUNT_ID.to_string(),
        })
        .await?;
    write_pooled_auth(codex_home, SECONDARY_ACCOUNT_ID, SECONDARY_ACCOUNT_ID)?;
    Ok(runtime)
}

fn write_pooled_auth(
    codex_home: &std::path::Path,
    backend_account_handle: &str,
    account_id: &str,
) -> Result<()> {
    let auth_home = codex_home
        .join(".pooled-auth/backends/local/accounts")
        .join(backend_account_handle);
    write_chatgpt_auth(
        auth_home.as_path(),
        ChatGptAuthFixture::new(format!("pooled-access-{account_id}"))
            .account_id(account_id)
            .chatgpt_account_id(account_id),
        AuthCredentialsStoreMode::File,
    )
}

fn test_event(event_id: &str, occurred_at: i64) -> AccountPoolEventRecord {
    AccountPoolEventRecord {
        event_id: event_id.to_string(),
        occurred_at: timestamp(occurred_at),
        pool_id: LEGACY_DEFAULT_POOL_ID.to_string(),
        account_id: Some(PRIMARY_ACCOUNT_ID.to_string()),
        lease_id: Some("lease-1".to_string()),
        holder_instance_id: Some("holder-1".to_string()),
        event_type: "leaseAcquired".to_string(),
        reason_code: None,
        message: format!("event {event_id}"),
        details_json: Some(json!({"source": "test"})),
    }
}

fn timestamp(seconds: i64) -> DateTime<Utc> {
    match DateTime::from_timestamp(seconds, 0) {
        Some(timestamp) => timestamp,
        None => panic!("invalid timestamp: {seconds}"),
    }
}

fn create_pooled_config_toml(
    codex_home: &std::path::Path,
    server_uri: &str,
) -> std::io::Result<()> {
    let config_toml = codex_home.join("config.toml");
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "read-only"

model_provider = "mock_provider"

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
supports_websockets = false

[accounts]
backend = "local"
default_pool = "{LEGACY_DEFAULT_POOL_ID}"
allocation_mode = "exclusive"
proactive_switch_threshold_percent = 91
min_switch_interval_secs = 7

[accounts.pools.legacy-default]
allow_context_reuse = false
account_kinds = ["chatgpt"]
"#
        ),
    )
}

fn create_config_toml_without_accounts(
    codex_home: &std::path::Path,
    server_uri: &str,
) -> std::io::Result<()> {
    let config_toml = codex_home.join("config.toml");
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "read-only"

model_provider = "mock_provider"

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
supports_websockets = false
"#
        ),
    )
}
