use super::connection_handling_websocket::DEFAULT_READ_TIMEOUT;
use super::connection_handling_websocket::connect_websocket;
use super::connection_handling_websocket::read_jsonrpc_message;
use super::connection_handling_websocket::read_response_for_id;
use super::connection_handling_websocket::send_initialize_request;
use super::connection_handling_websocket::send_request;
use super::connection_handling_websocket::spawn_websocket_server;
use anyhow::Result;
use app_test_support::ChatGptAuthFixture;
use app_test_support::McpProcess;
use app_test_support::create_final_assistant_message_sse_response;
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
use codex_app_server_protocol::ItemCompletedNotification;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ReviewDelivery;
use codex_app_server_protocol::ReviewStartParams;
use codex_app_server_protocol::ReviewTarget;
use codex_app_server_protocol::ThreadArchiveParams;
use codex_app_server_protocol::ThreadArchiveResponse;
use codex_app_server_protocol::ThreadForkParams;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnCompletedNotification;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::UserInput;
use codex_config::types::AuthCredentialsStoreMode;
use codex_state::AccountPoolEventRecord;
use codex_state::AccountStartupSelectionUpdate;
use codex_state::LegacyAccountImport;
use codex_state::StateRuntime;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
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
    create_pooled_config_toml_with_missing_default_pool(codex_home.path(), &server.uri())?;
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
async fn account_pool_read_succeeds_with_one_loaded_thread() -> Result<()> {
    let codex_home = TempDir::new()?;
    seed_two_accounts(codex_home.path()).await?;
    let mut mcp = initialized_mcp(codex_home.path()).await?;

    let thread = start_thread(&mut mcp).await?;
    assert!(!thread.id.is_empty());

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
async fn stdio_pooled_mode_rejects_second_loaded_top_level_thread() -> Result<()> {
    let codex_home = TempDir::new()?;
    seed_two_accounts(codex_home.path()).await?;
    let mut mcp = initialized_mcp(codex_home.path()).await?;

    let first = start_thread(&mut mcp).await?;
    let error = start_thread_error(&mut mcp).await?;

    assert_eq!(
        pooled_runtime_error_code(&error),
        Some("pooledRuntimeAlreadyLoaded")
    );
    assert!(!first.id.is_empty());
    Ok(())
}

#[tokio::test]
async fn stdio_pooled_mode_rejects_second_top_level_thread_without_resolved_startup_pool()
-> Result<()> {
    let codex_home = TempDir::new()?;
    seed_two_accounts(codex_home.path()).await?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    create_pooled_config_toml_with_missing_default_pool(codex_home.path(), &server.uri())?;
    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    mcp.initialize().await?;

    let first = start_thread(&mut mcp).await?;
    let error = start_thread_error(&mut mcp).await?;

    assert_eq!(
        pooled_runtime_error_code(&error),
        Some("pooledRuntimeAlreadyLoaded")
    );
    assert!(!first.id.is_empty());
    Ok(())
}

#[tokio::test]
async fn stdio_pooled_mode_uses_effective_request_config_for_top_level_gate() -> Result<()> {
    let codex_home = TempDir::new()?;
    let runtime = seed_two_accounts(codex_home.path()).await?;
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: None,
            preferred_account_id: None,
            suppressed: false,
        })
        .await?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    create_config_toml_without_accounts(codex_home.path(), &server.uri())?;
    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    mcp.initialize().await?;

    let request_config = pooled_accounts_request_config();
    let first = start_thread_with_config(&mut mcp, request_config.clone()).await?;
    let error = start_thread_error_with_config(&mut mcp, request_config).await?;

    assert_eq!(
        pooled_runtime_error_code(&error),
        Some("pooledRuntimeAlreadyLoaded")
    );
    assert!(!first.id.is_empty());
    Ok(())
}

#[tokio::test]
async fn stdio_pooled_mode_blocks_request_config_resume_and_fork_as_second_top_level_context()
-> Result<()> {
    let codex_home = TempDir::new()?;
    let runtime = seed_two_accounts(codex_home.path()).await?;
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: None,
            preferred_account_id: None,
            suppressed: false,
        })
        .await?;
    let responses = vec![create_final_assistant_message_sse_response("Done")?];
    let server = create_mock_responses_server_sequence_unchecked(responses).await;
    create_config_toml_without_accounts(codex_home.path(), &server.uri())?;
    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    mcp.initialize().await?;

    let loaded = start_thread_with_config(&mut mcp, pooled_accounts_request_config()).await?;
    start_turn_and_wait_completed(&mut mcp, loaded.id.as_str()).await?;
    let rollout_path = loaded
        .path
        .clone()
        .expect("started thread should report a rollout path");

    let resume_id = mcp
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: "00000000-0000-0000-0000-000000000000".to_string(),
            history: None,
            path: Some(rollout_path),
            model: None,
            model_provider: None,
            service_tier: None,
            cwd: None,
            approval_policy: None,
            approvals_reviewer: None,
            sandbox: None,
            config: None,
            base_instructions: None,
            developer_instructions: None,
            personality: None,
            persist_extended_history: false,
        })
        .await?;
    let resume_error = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(resume_id)),
    )
    .await??;
    assert_eq!(
        pooled_runtime_error_code(&resume_error),
        Some("pooledRuntimeAlreadyLoaded")
    );

    let fork_id = mcp
        .send_thread_fork_request(ThreadForkParams {
            thread_id: loaded.id,
            ..Default::default()
        })
        .await?;
    let fork_error = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(fork_id)),
    )
    .await??;
    assert_eq!(
        pooled_runtime_error_code(&fork_error),
        Some("pooledRuntimeAlreadyLoaded")
    );

    Ok(())
}

#[tokio::test]
async fn stdio_pooled_mode_releases_host_when_loaded_thread_archives() -> Result<()> {
    let codex_home = TempDir::new()?;
    seed_two_accounts(codex_home.path()).await?;
    let responses = vec![create_final_assistant_message_sse_response("Done")?];
    let mut mcp = initialized_mcp_with_responses(codex_home.path(), responses).await?;

    let first = start_thread(&mut mcp).await?;
    start_turn_and_wait_completed(&mut mcp, first.id.as_str()).await?;
    archive_thread(&mut mcp, first.id.as_str()).await?;

    let second = start_thread(&mut mcp).await?;

    assert_ne!(first.id, second.id);
    Ok(())
}

#[tokio::test]
async fn stdio_pooled_mode_transfers_loaded_owner_across_sibling_spawned_threads() -> Result<()> {
    const PARENT_PROMPT: &str = "spawn two pooled siblings";
    const CHILD_A_PROMPT: &str = "child A pooled work";
    const CHILD_B_PROMPT: &str = "child B pooled work";
    const SPAWN_A_CALL_ID: &str = "spawn-pooled-a";
    const SPAWN_B_CALL_ID: &str = "spawn-pooled-b";

    let codex_home = TempDir::new()?;
    seed_two_accounts(codex_home.path()).await?;
    let server = responses::start_mock_server().await;
    let spawn_a_args = serde_json::to_string(&json!({ "message": CHILD_A_PROMPT }))?;
    let spawn_b_args = serde_json::to_string(&json!({ "message": CHILD_B_PROMPT }))?;
    let _parent_turn = responses::mount_sse_once_match(
        &server,
        |req: &wiremock::Request| body_contains(req, PARENT_PROMPT),
        responses::sse(vec![
            responses::ev_response_created("resp-parent-1"),
            responses::ev_function_call(SPAWN_A_CALL_ID, "spawn_agent", &spawn_a_args),
            responses::ev_function_call(SPAWN_B_CALL_ID, "spawn_agent", &spawn_b_args),
            responses::ev_completed("resp-parent-1"),
        ]),
    )
    .await;
    let _child_a_turn = responses::mount_sse_once_match(
        &server,
        |req: &wiremock::Request| {
            body_contains(req, CHILD_A_PROMPT)
                && !body_contains(req, SPAWN_A_CALL_ID)
                && !body_contains(req, SPAWN_B_CALL_ID)
        },
        responses::sse(vec![
            responses::ev_response_created("resp-child-a"),
            responses::ev_assistant_message("msg-child-a", "child A done"),
            responses::ev_completed("resp-child-a"),
        ]),
    )
    .await;
    let _child_b_turn = responses::mount_sse_once_match(
        &server,
        |req: &wiremock::Request| {
            body_contains(req, CHILD_B_PROMPT)
                && !body_contains(req, SPAWN_A_CALL_ID)
                && !body_contains(req, SPAWN_B_CALL_ID)
        },
        responses::sse(vec![
            responses::ev_response_created("resp-child-b"),
            responses::ev_assistant_message("msg-child-b", "child B done"),
            responses::ev_completed("resp-child-b"),
        ]),
    )
    .await;
    let _parent_follow_up = responses::mount_sse_once_match(
        &server,
        |req: &wiremock::Request| {
            body_contains(req, SPAWN_A_CALL_ID) && body_contains(req, SPAWN_B_CALL_ID)
        },
        responses::sse(vec![
            responses::ev_response_created("resp-parent-2"),
            responses::ev_assistant_message("msg-parent-2", "parent done"),
            responses::ev_completed("resp-parent-2"),
        ]),
    )
    .await;
    let _parent_follow_up_after_children = responses::mount_sse_once_match(
        &server,
        |req: &wiremock::Request| {
            body_contains(req, "child A done") && body_contains(req, "child B done")
        },
        responses::sse(vec![
            responses::ev_response_created("resp-parent-3"),
            responses::ev_assistant_message("msg-parent-3", "parent done after children"),
            responses::ev_completed("resp-parent-3"),
        ]),
    )
    .await;
    create_pooled_collab_config_toml(codex_home.path(), &server.uri())?;
    let mut mcp = McpProcess::new(codex_home.path()).await?;
    mcp.initialize().await?;

    let parent = start_thread(&mut mcp).await?;
    let child_thread_ids = spawn_two_children_and_wait_completed(
        &mut mcp,
        parent.id.as_str(),
        PARENT_PROMPT,
        [SPAWN_A_CALL_ID, SPAWN_B_CALL_ID],
    )
    .await?;
    let mut child_thread_ids = child_thread_ids;
    child_thread_ids.sort();
    let first_owner = child_thread_ids[0].clone();
    let sibling = child_thread_ids[1].clone();

    archive_thread(&mut mcp, parent.id.as_str()).await?;
    archive_thread(&mut mcp, first_owner.as_str()).await?;
    let still_blocked = start_thread_error(&mut mcp).await?;
    assert_eq!(
        pooled_runtime_error_code(&still_blocked),
        Some("pooledRuntimeAlreadyLoaded")
    );

    archive_thread(&mut mcp, sibling.as_str()).await?;
    let next_top_level = start_thread(&mut mcp).await?;
    assert!(!next_top_level.id.is_empty());
    Ok(())
}

#[tokio::test]
async fn stdio_pooled_mode_blocks_detached_review_that_would_create_second_top_level_context()
-> Result<()> {
    let codex_home = TempDir::new()?;
    seed_two_accounts(codex_home.path()).await?;
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    create_config_toml_without_accounts(codex_home.path(), &server.uri())?;
    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    mcp.initialize().await?;

    let loaded = start_thread_with_config(&mut mcp, pooled_accounts_request_config()).await?;

    let review_request_id = mcp
        .send_review_start_request(ReviewStartParams {
            thread_id: loaded.id,
            delivery: Some(ReviewDelivery::Detached),
            target: ReviewTarget::Custom {
                instructions: "detached review should be gated".to_string(),
            },
        })
        .await?;
    let message = read_error_or_response_for_id(&mut mcp, review_request_id).await?;
    let JSONRPCMessage::Error(error) = message else {
        panic!("detached review should be rejected while pooled runtime is loaded: {message:?}");
    };

    assert_eq!(
        pooled_runtime_error_code(&error),
        Some("pooledRuntimeAlreadyLoaded")
    );
    Ok(())
}

#[tokio::test]
async fn stdio_pooled_mode_blocks_resume_and_fork_that_would_create_second_top_level_context()
-> Result<()> {
    let codex_home = TempDir::new()?;
    seed_two_accounts(codex_home.path()).await?;
    let responses = vec![create_final_assistant_message_sse_response("Done")?];
    let mut mcp = initialized_mcp_with_responses(codex_home.path(), responses).await?;

    let loaded = start_thread(&mut mcp).await?;
    start_turn_and_wait_completed(&mut mcp, loaded.id.as_str()).await?;
    let rollout_path = loaded
        .path
        .expect("started thread should report a rollout path");

    let resume_id = mcp
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: "00000000-0000-0000-0000-000000000000".to_string(),
            history: None,
            path: Some(rollout_path),
            model: None,
            model_provider: None,
            service_tier: None,
            cwd: None,
            approval_policy: None,
            approvals_reviewer: None,
            sandbox: None,
            config: None,
            base_instructions: None,
            developer_instructions: None,
            personality: None,
            persist_extended_history: false,
        })
        .await?;
    let resume_error = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(resume_id)),
    )
    .await??;
    assert_eq!(
        pooled_runtime_error_code(&resume_error),
        Some("pooledRuntimeAlreadyLoaded")
    );

    let fork_id = mcp
        .send_thread_fork_request(ThreadForkParams {
            thread_id: loaded.id,
            ..Default::default()
        })
        .await?;
    let fork_error = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(fork_id)),
    )
    .await??;
    assert_eq!(
        pooled_runtime_error_code(&fork_error),
        Some("pooledRuntimeAlreadyLoaded")
    );

    Ok(())
}

#[tokio::test]
async fn websocket_app_server_rejects_pooled_runtime_host_creation() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_pooled_config_toml(codex_home.path(), &server.uri())?;
    seed_default_pool_state(codex_home.path()).await?;

    let (mut process, bind_addr) = spawn_websocket_server(codex_home.path()).await?;
    let mut ws = connect_websocket(bind_addr).await?;
    send_initialize_request(&mut ws, /*id*/ 1, "ws_account_pool_client").await?;
    let init = read_response_for_id(&mut ws, /*id*/ 1).await?;
    assert_eq!(init.id, RequestId::Integer(1));

    send_request(
        &mut ws,
        "thread/start",
        /*id*/ 2,
        Some(serde_json::to_value(ThreadStartParams::default())?),
    )
    .await?;
    let error = read_websocket_error_for_id(&mut ws, /*id*/ 2).await?;

    assert_eq!(
        pooled_runtime_error_code(&error),
        Some("pooledRuntimeUnsupportedTransport")
    );

    send_request(
        &mut ws,
        "thread/resume",
        /*id*/ 3,
        Some(serde_json::to_value(ThreadResumeParams {
            thread_id: "00000000-0000-0000-0000-000000000000".to_string(),
            ..Default::default()
        })?),
    )
    .await?;
    let resume_error = read_websocket_error_for_id(&mut ws, /*id*/ 3).await?;

    assert_eq!(
        pooled_runtime_error_code(&resume_error),
        Some("pooledRuntimeUnsupportedTransport")
    );

    process.kill().await?;
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
    initialized_mcp_with_server(codex_home, &server.uri()).await
}

async fn initialized_mcp_with_responses(
    codex_home: &std::path::Path,
    responses: Vec<String>,
) -> Result<McpProcess> {
    let server = create_mock_responses_server_sequence_unchecked(responses).await;
    initialized_mcp_with_server(codex_home, &server.uri()).await
}

async fn initialized_mcp_with_server(
    codex_home: &std::path::Path,
    server_uri: &str,
) -> Result<McpProcess> {
    create_pooled_config_toml(codex_home, server_uri)?;

    let mut mcp = McpProcess::new_with_env(codex_home, &[("OPENAI_API_KEY", None)]).await?;
    mcp.initialize().await?;
    Ok(mcp)
}

async fn start_thread(mcp: &mut McpProcess) -> Result<codex_app_server_protocol::Thread> {
    let request_id = mcp
        .send_thread_start_request(ThreadStartParams::default())
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response(response)?;
    Ok(thread)
}

async fn start_thread_with_config(
    mcp: &mut McpProcess,
    config: HashMap<String, serde_json::Value>,
) -> Result<codex_app_server_protocol::Thread> {
    let request_id = mcp
        .send_thread_start_request(ThreadStartParams {
            config: Some(config),
            ..Default::default()
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response(response)?;
    Ok(thread)
}

async fn start_thread_error(mcp: &mut McpProcess) -> Result<JSONRPCError> {
    let request_id = mcp
        .send_thread_start_request(ThreadStartParams::default())
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await?
}

async fn start_thread_error_with_config(
    mcp: &mut McpProcess,
    config: HashMap<String, serde_json::Value>,
) -> Result<JSONRPCError> {
    let request_id = mcp
        .send_thread_start_request(ThreadStartParams {
            config: Some(config),
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await?
}

async fn read_error_or_response_for_id(
    mcp: &mut McpProcess,
    request_id: i64,
) -> Result<JSONRPCMessage> {
    let target_id = RequestId::Integer(request_id);
    loop {
        let message = timeout(DEFAULT_READ_TIMEOUT, mcp.read_next_message()).await??;
        match &message {
            JSONRPCMessage::Error(error) if error.id == target_id => return Ok(message),
            JSONRPCMessage::Response(response) if response.id == target_id => return Ok(message),
            _ => {}
        }
    }
}

async fn start_turn_and_wait_completed(mcp: &mut McpProcess, thread_id: &str) -> Result<()> {
    let request_id = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread_id.to_string(),
            input: vec![UserInput::Text {
                text: "materialize rollout".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let _: TurnStartResponse = to_response(response)?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;
    Ok(())
}

async fn spawn_two_children_and_wait_completed(
    mcp: &mut McpProcess,
    thread_id: &str,
    prompt: &str,
    spawn_call_ids: [&str; 2],
) -> Result<Vec<String>> {
    let request_id = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread_id.to_string(),
            input: vec![UserInput::Text {
                text: prompt.to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let turn: TurnStartResponse = to_response(response)?;

    let mut child_thread_ids = Vec::new();
    while child_thread_ids.len() < spawn_call_ids.len() {
        let notification = timeout(
            DEFAULT_READ_TIMEOUT,
            mcp.read_stream_until_notification_message("item/completed"),
        )
        .await??;
        let completed: ItemCompletedNotification = serde_json::from_value(
            notification
                .params
                .ok_or_else(|| anyhow::anyhow!("item/completed notification missing params"))?,
        )?;
        if let ThreadItem::CollabAgentToolCall {
            id,
            receiver_thread_ids,
            ..
        } = completed.item
            && spawn_call_ids.contains(&id.as_str())
        {
            child_thread_ids.extend(receiver_thread_ids);
        }
    }

    timeout(DEFAULT_READ_TIMEOUT, async {
        loop {
            let notification = mcp
                .read_stream_until_notification_message("turn/completed")
                .await?;
            let completed: TurnCompletedNotification =
                serde_json::from_value(notification.params.ok_or_else(|| {
                    anyhow::anyhow!("turn/completed notification missing params")
                })?)?;
            if completed.thread_id == thread_id && completed.turn.id == turn.turn.id {
                anyhow::ensure!(
                    completed.turn.error.is_none(),
                    "parent turn should complete without error: {:?}",
                    completed.turn.error
                );
                return Ok::<(), anyhow::Error>(());
            }
        }
    })
    .await??;

    Ok(child_thread_ids)
}

async fn archive_thread(mcp: &mut McpProcess, thread_id: &str) -> Result<()> {
    let request_id = mcp
        .send_thread_archive_request(ThreadArchiveParams {
            thread_id: thread_id.to_string(),
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let _: ThreadArchiveResponse = to_response(response)?;
    Ok(())
}

fn body_contains(req: &wiremock::Request, text: &str) -> bool {
    String::from_utf8(req.body.clone())
        .ok()
        .is_some_and(|body| body.contains(text))
}

fn pooled_runtime_error_code(error: &JSONRPCError) -> Option<&str> {
    error.error.data.as_ref()?.get("errorCode")?.as_str()
}

fn pooled_accounts_request_config() -> HashMap<String, serde_json::Value> {
    HashMap::from([
        ("accounts.backend".to_string(), json!("local")),
        (
            "accounts.default_pool".to_string(),
            json!(LEGACY_DEFAULT_POOL_ID),
        ),
        ("accounts.allocation_mode".to_string(), json!("exclusive")),
        (
            "accounts.pools.legacy-default.allow_context_reuse".to_string(),
            json!(false),
        ),
        (
            "accounts.pools.legacy-default.account_kinds".to_string(),
            json!(["chatgpt"]),
        ),
    ])
}

async fn read_websocket_error_for_id(
    stream: &mut super::connection_handling_websocket::WsClient,
    id: i64,
) -> Result<JSONRPCError> {
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

fn create_pooled_collab_config_toml(
    codex_home: &std::path::Path,
    server_uri: &str,
) -> std::io::Result<()> {
    create_pooled_config_toml(codex_home, server_uri)?;
    let config_toml = codex_home.join("config.toml");
    let config = std::fs::read_to_string(config_toml.as_path())?;
    std::fs::write(
        config_toml,
        format!(
            r#"{config}
[features]
multi_agent = true
"#
        ),
    )
}

fn create_pooled_config_toml_with_missing_default_pool(
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
default_pool = "missing-default"
allocation_mode = "exclusive"

[accounts.pools.missing-default]
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
