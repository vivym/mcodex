use super::connection_handling_websocket::DEFAULT_READ_TIMEOUT;
use super::connection_handling_websocket::WsClient;
use super::connection_handling_websocket::connect_websocket;
use super::connection_handling_websocket::read_jsonrpc_message;
use super::connection_handling_websocket::read_response_for_id;
use super::connection_handling_websocket::send_initialize_request;
use super::connection_handling_websocket::send_request;
use super::connection_handling_websocket::spawn_websocket_server;
use anyhow::Context;
use anyhow::Result;
use app_test_support::ChatGptIdTokenClaims;
use app_test_support::McpProcess;
use app_test_support::create_mock_responses_server_sequence_unchecked;
use app_test_support::encode_id_token;
use app_test_support::to_response;
use codex_app_server_protocol::AccountLeaseReadResponse;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_state::AccountStartupSelectionUpdate;
use codex_state::LegacyAccountImport;
use codex_state::StateRuntime;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::time::timeout;

const PRIMARY_ACCOUNT_ID: &str = "acct-1";

#[tokio::test]
async fn account_lease_read_reports_process_local_pool_state() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_pooled_config_toml(codex_home.path(), &server.uri())?;
    seed_default_pool_state(codex_home.path()).await?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(response.suppressed, false);
    assert_eq!(response.health_state.as_deref(), Some("healthy"));

    Ok(())
}

#[tokio::test]
async fn account_lease_updated_emits_on_resume() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_pooled_config_toml(codex_home.path(), &server.uri())?;
    let runtime = seed_default_pool_state(codex_home.path()).await?;
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("legacy-default".to_string()),
            preferred_account_id: Some(PRIMARY_ACCOUNT_ID.to_string()),
            suppressed: true,
        })
        .await?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let _: JSONRPCResponse = mcp.account_lease_resume().await?;
    assert_eq!(
        mcp.next_notification_method().await?,
        "accountLease/updated"
    );

    Ok(())
}

#[tokio::test]
async fn account_lease_resume_preserves_persisted_default_pool() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_pooled_config_toml_with_default_pool(
        codex_home.path(),
        &server.uri(),
        "config-default",
    )?;
    let runtime = seed_default_pool_state(codex_home.path()).await?;
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("persisted-default".to_string()),
            preferred_account_id: Some(PRIMARY_ACCOUNT_ID.to_string()),
            suppressed: true,
        })
        .await?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let _: JSONRPCResponse = mcp.account_lease_resume().await?;

    let selection = runtime.read_account_startup_selection().await?;
    assert_eq!(
        selection.default_pool_id.as_deref(),
        Some("persisted-default")
    );
    assert_eq!(selection.preferred_account_id, None);
    assert_eq!(selection.suppressed, false);

    Ok(())
}

#[tokio::test]
async fn pooled_mode_rejects_multi_thread_stdio_runtime() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_pooled_config_toml(codex_home.path(), &server.uri())?;
    seed_default_pool_state(codex_home.path()).await?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

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

    let request_id = mcp.send_account_lease_read_request().await?;
    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert!(
        error.error.message.contains("one loaded thread"),
        "unexpected error: {error:?}"
    );

    Ok(())
}

#[tokio::test]
async fn pooled_mode_rejects_websocket_runtime() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_pooled_config_toml(codex_home.path(), &server.uri())?;
    seed_default_pool_state(codex_home.path()).await?;

    let (mut process, bind_addr) = spawn_websocket_server(codex_home.path()).await?;
    let mut ws = connect_websocket(bind_addr).await?;
    send_initialize_request(&mut ws, /*id*/ 1, "ws_account_lease_client").await?;
    let init = read_response_for_id(&mut ws, /*id*/ 1).await?;
    assert_eq!(init.id, RequestId::Integer(1));

    send_request(
        &mut ws,
        "accountLease/read",
        /*id*/ 2,
        /*params*/ None,
    )
    .await?;
    let error = read_error_for_id(&mut ws, /*id*/ 2).await?;
    assert!(
        error
            .error
            .message
            .contains("pooled lease mode is only supported for stdio"),
        "unexpected websocket error: {error:?}"
    );

    process
        .kill()
        .await
        .context("failed to stop websocket app-server process")?;
    Ok(())
}

#[tokio::test]
async fn account_logout_with_runtime_local_chatgpt_tokens_is_not_durable() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_pooled_config_toml(codex_home.path(), &server.uri())?;
    seed_default_pool_state(codex_home.path()).await?;

    let access_token = encode_id_token(
        &ChatGptIdTokenClaims::new()
            .email("runtime@example.com")
            .plan_type("pro")
            .chatgpt_account_id("runtime-org"),
    )?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let login_id = mcp
        .send_chatgpt_auth_tokens_login_request(
            access_token,
            "runtime-org".to_string(),
            Some("pro".to_string()),
        )
        .await?;
    let _: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(login_id)),
    )
    .await??;

    let logout_id = mcp.send_logout_account_request().await?;
    let _: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(logout_id)),
    )
    .await??;

    let lease: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(lease.suppressed, false);

    Ok(())
}

async fn seed_default_pool_state(codex_home: &std::path::Path) -> Result<Arc<StateRuntime>> {
    let runtime = StateRuntime::init(codex_home.to_path_buf(), "mock_provider".to_string()).await?;
    runtime
        .import_legacy_default_account(LegacyAccountImport {
            account_id: PRIMARY_ACCOUNT_ID.to_string(),
        })
        .await?;
    Ok(runtime)
}

fn create_pooled_config_toml(
    codex_home: &std::path::Path,
    server_uri: &str,
) -> std::io::Result<()> {
    create_pooled_config_toml_with_default_pool(codex_home, server_uri, "legacy-default")
}

fn create_pooled_config_toml_with_default_pool(
    codex_home: &std::path::Path,
    server_uri: &str,
    default_pool: &str,
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
default_pool = "{default_pool}"
allocation_mode = "exclusive"

[accounts.pools.legacy-default]
allow_context_reuse = false
account_kinds = ["chatgpt"]

[accounts.pools.config-default]
allow_context_reuse = false
account_kinds = ["chatgpt"]

[accounts.pools.persisted-default]
allow_context_reuse = false
account_kinds = ["chatgpt"]
"#
        ),
    )
}

async fn read_error_for_id(stream: &mut WsClient, id: i64) -> Result<JSONRPCError> {
    let target_id = RequestId::Integer(id);
    loop {
        let message = read_jsonrpc_message(stream).await?;
        if let JSONRPCMessage::Error(error) = message
            && error.id == target_id
        {
            return Ok(error);
        }
    }
}
