use super::connection_handling_websocket::DEFAULT_READ_TIMEOUT;
use super::connection_handling_websocket::WsClient;
use super::connection_handling_websocket::connect_websocket;
use super::connection_handling_websocket::read_jsonrpc_message;
use super::connection_handling_websocket::read_notification_for_method;
use super::connection_handling_websocket::read_response_for_id;
use super::connection_handling_websocket::send_initialize_request;
use super::connection_handling_websocket::send_request;
use super::connection_handling_websocket::spawn_websocket_server;
use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use anyhow::ensure;
use app_test_support::ChatGptAuthFixture;
use app_test_support::ChatGptIdTokenClaims;
use app_test_support::McpProcess;
use app_test_support::create_final_assistant_message_sse_response;
use app_test_support::create_mock_responses_server_sequence_unchecked;
use app_test_support::encode_id_token;
use app_test_support::to_response;
use app_test_support::write_chatgpt_auth;
use codex_app_server_protocol::AccountLeaseReadResponse;
use codex_app_server_protocol::AccountLeaseUpdatedNotification;
use codex_app_server_protocol::AccountStartupAvailability;
use codex_app_server_protocol::AccountStartupResolutionIssueSource;
use codex_app_server_protocol::AccountStartupResolutionIssueType;
use codex_app_server_protocol::ErrorNotification;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::ThreadStatus;
use codex_app_server_protocol::ThreadStatusChangedNotification;
use codex_app_server_protocol::TurnCompletedNotification;
use codex_app_server_protocol::TurnError;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::UserInput;
use codex_config::types::AuthCredentialsStoreMode;
use codex_state::AccountRegistryEntryUpdate;
use codex_state::AccountStartupSelectionUpdate;
use codex_state::LegacyAccountImport;
use codex_state::StateRuntime;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::time::Duration;
use tokio::time::timeout;
use wiremock::Mock;
use wiremock::Respond;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path_regex;

const PRIMARY_ACCOUNT_ID: &str = "acct-1";
const SECONDARY_ACCOUNT_ID: &str = "acct-2";
const ACCOUNT_LEASE_ROTATION_TIMEOUT: Duration = Duration::from_secs(30);

#[tokio::test]
async fn account_lease_read_includes_startup_snapshot_for_single_pool_fallback() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_policy_only_pooled_config_toml(codex_home.path(), &server.uri())?;
    seed_account_in_pool(codex_home.path(), PRIMARY_ACCOUNT_ID, "legacy-default", 0).await?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(response.pool_id.as_deref(), Some("legacy-default"));
    assert_eq!(
        response.startup.effective_pool_id.as_deref(),
        Some("legacy-default")
    );
    assert_eq!(
        response.startup.effective_pool_resolution_source,
        "singleVisiblePool"
    );
    assert_eq!(
        response.startup.startup_availability,
        AccountStartupAvailability::Available
    );
    assert_eq!(
        response.startup.selection_eligibility,
        "automaticAccountSelected"
    );

    Ok(())
}

#[tokio::test]
async fn account_lease_read_preserves_candidate_pools_for_multi_pool_blocker() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_pooled_config_toml_without_default(
        codex_home.path(),
        &server.uri(),
        &["team-main", "team-other"],
    )?;
    seed_account_in_pool(codex_home.path(), "acct-team-main", "team-main", 0).await?;
    seed_account_in_pool(codex_home.path(), "acct-team-other", "team-other", 1).await?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(
        response.startup.startup_availability,
        AccountStartupAvailability::MultiplePoolsRequireDefault
    );
    let issue = response
        .startup
        .startup_resolution_issue
        .expect("startup resolution issue should be present");
    assert_eq!(
        issue.r#type,
        AccountStartupResolutionIssueType::MultiplePoolsRequireDefault
    );
    assert_eq!(issue.source, AccountStartupResolutionIssueSource::None);
    assert_eq!(issue.candidate_pool_count, Some(2));
    let candidate_pool_ids = issue
        .candidate_pools
        .expect("candidate pools should be present")
        .into_iter()
        .map(|pool| pool.pool_id)
        .collect::<Vec<_>>();
    assert_eq!(candidate_pool_ids, vec!["team-main", "team-other"]);

    Ok(())
}

#[tokio::test]
async fn account_lease_read_preserves_candidate_pools_for_invalid_config_default() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_pooled_config_toml_with_default_pool(codex_home.path(), &server.uri(), "missing-pool")?;
    seed_account_in_pool(codex_home.path(), "acct-team-main", "team-main", 0).await?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(
        response.startup.startup_availability,
        AccountStartupAvailability::InvalidExplicitDefault
    );
    let issue = response
        .startup
        .startup_resolution_issue
        .expect("startup resolution issue should be present");
    assert_eq!(
        issue.r#type,
        AccountStartupResolutionIssueType::ConfigDefaultPoolUnavailable
    );
    assert_eq!(
        issue.source,
        AccountStartupResolutionIssueSource::ConfigDefault
    );
    let candidate_pool_ids = issue
        .candidate_pools
        .expect("candidate pools should be present")
        .into_iter()
        .map(|pool| pool.pool_id)
        .collect::<Vec<_>>();
    assert_eq!(candidate_pool_ids, vec!["team-main"]);

    Ok(())
}

#[tokio::test]
async fn account_lease_read_keeps_live_top_level_fields_separate_from_startup_snapshot()
-> Result<()> {
    let responses = vec![create_final_assistant_message_sse_response("Done")?];
    let server = create_mock_responses_server_sequence_unchecked(responses).await;
    let codex_home = TempDir::new()?;
    create_pooled_config_toml_without_default(
        codex_home.path(),
        &server.uri(),
        &["legacy-default", "config-default"],
    )?;
    let runtime = seed_default_pool_state(codex_home.path()).await?;
    seed_account_in_pool(codex_home.path(), SECONDARY_ACCOUNT_ID, "config-default", 1).await?;
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("legacy-default".to_string()),
            preferred_account_id: None,
            suppressed: false,
        })
        .await?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread = start_thread(&mut mcp).await?;
    let _turn = start_turn(&mut mcp, &thread.id, "lease legacy default").await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("config-default".to_string()),
            preferred_account_id: None,
            suppressed: false,
        })
        .await?;

    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(response.pool_id.as_deref(), Some("legacy-default"));
    assert_eq!(
        response.startup.effective_pool_id.as_deref(),
        Some("config-default")
    );
    assert_eq!(
        response.startup.effective_pool_resolution_source,
        "persistedSelection"
    );
    assert_eq!(response.startup.configured_default_pool_id, None);
    assert_eq!(
        response.startup.persisted_default_pool_id.as_deref(),
        Some("config-default")
    );

    Ok(())
}

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
async fn account_lease_read_reports_disabled_preferred_account_preview_reason() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_pooled_config_toml(codex_home.path(), &server.uri())?;
    let runtime = seed_default_pool_state(codex_home.path()).await?;
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("legacy-default".to_string()),
            preferred_account_id: Some(PRIMARY_ACCOUNT_ID.to_string()),
            suppressed: false,
        })
        .await?;
    runtime
        .set_account_enabled(PRIMARY_ACCOUNT_ID, false)
        .await?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(response.active, false);
    assert_eq!(response.account_id, None);
    assert_eq!(response.pool_id.as_deref(), Some("legacy-default"));
    assert_eq!(
        response.switch_reason.as_deref(),
        Some("preferredAccountDisabled")
    );
    assert_eq!(response.health_state.as_deref(), Some("unavailable"));

    Ok(())
}

#[tokio::test]
async fn account_lease_read_reports_live_active_lease_fields_after_turn_start() -> Result<()> {
    let responses = vec![create_final_assistant_message_sse_response("Done")?];
    let server = create_mock_responses_server_sequence_unchecked(responses).await;
    let codex_home = TempDir::new()?;
    create_pooled_config_toml(codex_home.path(), &server.uri())?;
    seed_default_pool_state(codex_home.path()).await?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread = start_thread(&mut mcp).await?;
    let _turn = start_turn(&mut mcp, &thread.id, "lease me").await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(response.active, true);
    assert_eq!(response.account_id.as_deref(), Some(PRIMARY_ACCOUNT_ID));
    assert_eq!(response.pool_id.as_deref(), Some("legacy-default"));
    assert!(response.lease_id.is_some());
    assert_eq!(response.lease_epoch, Some(1));

    Ok(())
}

#[tokio::test]
async fn account_lease_read_and_resume_use_loaded_thread_request_config() -> Result<()> {
    let responses = vec![create_final_assistant_message_sse_response("Done")?];
    let server = create_mock_responses_server_sequence_unchecked(responses).await;
    let codex_home = TempDir::new()?;
    create_config_toml_without_accounts(codex_home.path(), &server.uri())?;
    let runtime = seed_default_pool_state(codex_home.path()).await?;
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: None,
            preferred_account_id: None,
            suppressed: false,
        })
        .await?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread = start_thread_with_config(&mut mcp, pooled_accounts_request_config()).await?;
    let _turn = start_turn(&mut mcp, &thread.id, "request scoped lease").await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(response.active, true);
    assert_eq!(response.account_id.as_deref(), Some(PRIMARY_ACCOUNT_ID));
    assert_eq!(response.pool_id.as_deref(), Some("legacy-default"));
    assert_eq!(
        response.effective_pool_resolution_source.as_deref(),
        Some("configDefault")
    );
    assert_eq!(
        response.configured_default_pool_id.as_deref(),
        Some("legacy-default")
    );

    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_matching_notification(
            "initial accountLease/updated before resume",
            is_primary_account_lease_update,
        ),
    )
    .await??;
    mcp.clear_message_buffer();

    let _: JSONRPCResponse = mcp.account_lease_resume().await?;
    let notification = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_matching_notification(
            "accountLease/updated after resume",
            is_primary_account_lease_update,
        ),
    )
    .await??;
    let updated: AccountLeaseUpdatedNotification =
        serde_json::from_value(notification.params.expect("params must be present"))?;
    assert_eq!(updated.account_id.as_deref(), Some(PRIMARY_ACCOUNT_ID));
    assert_eq!(updated.pool_id.as_deref(), Some("legacy-default"));
    assert_eq!(updated.suppressed, false);
    assert!(updated.lease_acquired_at.is_some());

    let after_resume: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(after_resume.pool_id.as_deref(), Some("legacy-default"));
    assert_eq!(
        after_resume.effective_pool_resolution_source.as_deref(),
        Some("configDefault")
    );

    Ok(())
}

#[tokio::test]
async fn account_lease_read_and_update_report_live_proactive_switch_suppression_fields()
-> Result<()> {
    let server = core_test_support::responses::start_mock_server().await;
    Mock::given(method("POST"))
        .and(path_regex(".*/responses$"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .insert_header("x-codex-primary-used-percent", "92.0")
                .insert_header("x-codex-primary-window-minutes", "60")
                .set_body_raw(
                    create_final_assistant_message_sse_response("Done")?,
                    "text/event-stream",
                ),
        )
        .mount(&server)
        .await;
    let codex_home = TempDir::new()?;
    create_pooled_config_toml_with_min_switch_interval(codex_home.path(), &server.uri(), 5)?;
    seed_default_pool_state(codex_home.path()).await?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread = start_thread(&mut mcp).await?;
    let turn = start_turn(&mut mcp, &thread.id, "hold this lease").await?;
    let notification = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_matching_notification(
            "accountLease/updated for proactive switch suppression",
            |notification| {
                notification.method == "accountLease/updated"
                    && notification
                        .params
                        .as_ref()
                        .and_then(|params| params.get("proactiveSwitchPending"))
                        .and_then(serde_json::Value::as_bool)
                        == Some(true)
            },
        ),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_matching_notification(
            "turn/completed for proactive switch suppression turn",
            |notification| {
                notification.method == "turn/completed"
                    && notification
                        .params
                        .as_ref()
                        .and_then(|params| params.get("turn"))
                        .and_then(|turn_params| turn_params.get("id"))
                        .and_then(serde_json::Value::as_str)
                        == Some(turn.id.as_str())
            },
        ),
    )
    .await??;

    let updated: AccountLeaseUpdatedNotification =
        serde_json::from_value(notification.params.expect("params must be present"))?;
    assert_eq!(updated.account_id.as_deref(), Some(PRIMARY_ACCOUNT_ID));
    assert_eq!(updated.pool_id.as_deref(), Some("legacy-default"));
    assert_eq!(updated.suppressed, false);
    assert_eq!(updated.min_switch_interval_secs, Some(5));
    assert_eq!(updated.proactive_switch_pending, Some(true));
    assert_eq!(updated.proactive_switch_suppressed, Some(true));
    assert!(updated.lease_acquired_at.is_some());
    assert!(updated.proactive_switch_allowed_at.is_some());

    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(response.account_id.as_deref(), Some(PRIMARY_ACCOUNT_ID));
    assert_eq!(response.min_switch_interval_secs, Some(5));
    assert_eq!(response.proactive_switch_pending, Some(true));
    assert_eq!(response.proactive_switch_suppressed, Some(true));
    assert!(response.lease_acquired_at.is_some());
    assert!(response.proactive_switch_allowed_at.is_some());

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
    let notification = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("accountLease/updated"),
    )
    .await??;
    let updated: AccountLeaseUpdatedNotification =
        serde_json::from_value(notification.params.expect("params must be present"))?;
    assert_eq!(updated.account_id.as_deref(), Some(PRIMARY_ACCOUNT_ID));
    assert_eq!(updated.pool_id.as_deref(), Some("legacy-default"));
    assert_eq!(updated.suppressed, false);

    Ok(())
}

#[tokio::test]
async fn account_lease_updated_emits_when_automatic_switch_changes_live_snapshot() -> Result<()> {
    let server = create_rate_limit_then_success_server().await;
    let codex_home = TempDir::new()?;
    create_pooled_config_toml(codex_home.path(), &server.uri())?;
    seed_two_accounts(codex_home.path()).await?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(ACCOUNT_LEASE_ROTATION_TIMEOUT, mcp.initialize())
        .await
        .context("timed out initializing pooled account lease test mcp process")??;

    let thread = start_thread_with_timeout(&mut mcp, ACCOUNT_LEASE_ROTATION_TIMEOUT).await?;

    let _first_turn = start_turn_with_timeout(
        &mut mcp,
        &thread.id,
        "hit the limit",
        ACCOUNT_LEASE_ROTATION_TIMEOUT,
    )
    .await?;
    timeout(
        ACCOUNT_LEASE_ROTATION_TIMEOUT,
        mcp.read_stream_until_notification_message("error"),
    )
    .await
    .context("timed out waiting for first turn rate-limit error")??;
    let failed_turn_completed = timeout(
        ACCOUNT_LEASE_ROTATION_TIMEOUT,
        mcp.read_stream_until_matching_notification(
            "turn/completed for first failed turn",
            |notification| {
                notification.method == "turn/completed"
                    && notification
                        .params
                        .as_ref()
                        .and_then(|params| params.get("turn"))
                        .and_then(|turn| turn.get("id"))
                        .and_then(serde_json::Value::as_str)
                        == Some(_first_turn.id.as_str())
            },
        ),
    )
    .await
    .context("timed out waiting for first failed turn/completed notification")??;
    let failed_turn_completed: TurnCompletedNotification = serde_json::from_value(
        failed_turn_completed
            .params
            .context("first failed turn/completed notification missing params")?,
    )?;
    ensure!(
        failed_turn_completed.turn.error.is_some(),
        "first turn should complete with rate-limit error"
    );
    wait_for_terminal_thread_status_after_failed_turn(
        &mut mcp,
        &thread.id,
        ACCOUNT_LEASE_ROTATION_TIMEOUT,
    )
    .await?;
    // The rotated lease update may be emitted on the first turn error or on the next turn start,
    // so keep buffered notifications until we observe the secondary-account snapshot.

    let second_turn = start_turn_with_timeout(
        &mut mcp,
        &thread.id,
        "rotate to the next account",
        ACCOUNT_LEASE_ROTATION_TIMEOUT,
    )
    .await?;
    let mut rotated_turn_completed = false;
    let notification = match timeout(
        ACCOUNT_LEASE_ROTATION_TIMEOUT,
        wait_for_secondary_account_lease_update(
            &mut mcp,
            &thread.id,
            second_turn.id.as_str(),
            &mut rotated_turn_completed,
        ),
    )
    .await
    {
        Ok(notification) => notification?,
        Err(_) => {
            let pending_notifications = mcp.pending_notification_methods();
            let lease_snapshot: AccountLeaseReadResponse = to_response(
                mcp.read_account_lease()
                    .await
                    .context("failed to read account lease after rotated notification timeout")?,
            )?;
            bail!(
                "timed out waiting for rotated accountLease/updated notification; pending_notifications={pending_notifications:?}; lease_snapshot={lease_snapshot:?}"
            );
        }
    };
    let updated: AccountLeaseUpdatedNotification =
        serde_json::from_value(notification.params.expect("params must be present"))?;
    assert_eq!(updated.account_id.as_deref(), Some(SECONDARY_ACCOUNT_ID));
    assert_eq!(updated.pool_id.as_deref(), Some("legacy-default"));
    assert_eq!(updated.suppressed, false);

    if !rotated_turn_completed {
        timeout(
            ACCOUNT_LEASE_ROTATION_TIMEOUT,
            wait_for_rotated_turn_completed(&mut mcp, &thread.id, second_turn.id.as_str()),
        )
        .await
        .context("timed out waiting for rotated turn/completed notification")??;
    }

    Ok(())
}

async fn wait_for_secondary_account_lease_update(
    mcp: &mut McpProcess,
    thread_id: &str,
    turn_id: &str,
    rotated_turn_completed: &mut bool,
) -> Result<JSONRPCNotification> {
    loop {
        let message = mcp.read_next_message().await?;
        let JSONRPCMessage::Notification(notification) = message else {
            continue;
        };
        ensure_no_turn_error_notification(&notification, thread_id, turn_id)?;
        if is_secondary_account_lease_update(&notification) {
            return Ok(notification);
        }
        if is_completed_turn_notification(&notification, thread_id, turn_id)? {
            *rotated_turn_completed = true;
        }
    }
}

async fn wait_for_rotated_turn_completed(
    mcp: &mut McpProcess,
    thread_id: &str,
    turn_id: &str,
) -> Result<()> {
    loop {
        let message = mcp.read_next_message().await?;
        let JSONRPCMessage::Notification(notification) = message else {
            continue;
        };
        ensure_no_turn_error_notification(&notification, thread_id, turn_id)?;
        if is_completed_turn_notification(&notification, thread_id, turn_id)? {
            return Ok(());
        }
    }
}

fn is_completed_turn_notification(
    notification: &JSONRPCNotification,
    thread_id: &str,
    turn_id: &str,
) -> Result<bool> {
    if notification.method != "turn/completed" {
        return Ok(false);
    }
    let params = notification
        .params
        .clone()
        .context("turn/completed notification missing params")?;
    let completed: TurnCompletedNotification = serde_json::from_value(params)?;
    if completed.thread_id != thread_id || completed.turn.id != turn_id {
        return Ok(false);
    }
    ensure!(
        completed.turn.error.is_none(),
        "rotated account turn should complete without error: {:?}",
        completed.turn.error
    );
    Ok(true)
}

fn ensure_no_turn_error_notification(
    notification: &JSONRPCNotification,
    thread_id: &str,
    turn_id: &str,
) -> Result<()> {
    if notification.method != "error" {
        return Ok(());
    }
    let params = notification
        .params
        .clone()
        .context("error notification missing params")?;
    let error: ErrorNotification = serde_json::from_value(params)?;
    if error.thread_id == thread_id && error.turn_id == turn_id && !error.will_retry {
        bail!(
            "rotated account turn emitted error notification: {:?}",
            error.error
        );
    }
    Ok(())
}

#[test]
fn rotated_turn_error_guard_allows_retryable_same_turn_errors() {
    let notification = JSONRPCNotification {
        method: "error".to_string(),
        params: Some(json!(ErrorNotification {
            error: TurnError {
                message: "retrying drained account".to_string(),
                codex_error_info: None,
                additional_details: None,
            },
            will_retry: true,
            thread_id: "thread-1".to_string(),
            turn_id: "turn-1".to_string(),
        })),
    };

    ensure_no_turn_error_notification(&notification, "thread-1", "turn-1")
        .expect("retryable same-turn errors should not interrupt the rotated turn");
}

#[test]
fn rotated_turn_error_guard_rejects_terminal_same_turn_errors() {
    let notification = JSONRPCNotification {
        method: "error".to_string(),
        params: Some(json!(ErrorNotification {
            error: TurnError {
                message: "drained account".to_string(),
                codex_error_info: None,
                additional_details: None,
            },
            will_retry: false,
            thread_id: "thread-1".to_string(),
            turn_id: "turn-1".to_string(),
        })),
    };

    let error = ensure_no_turn_error_notification(&notification, "thread-1", "turn-1")
        .expect_err("rotated turn should reject terminal same-turn errors");
    assert!(
        error
            .to_string()
            .contains("rotated account turn emitted error notification")
    );
}

fn is_primary_account_lease_update(notification: &JSONRPCNotification) -> bool {
    notification.method == "accountLease/updated"
        && notification
            .params
            .as_ref()
            .and_then(|params| params.get("accountId"))
            .and_then(serde_json::Value::as_str)
            == Some(PRIMARY_ACCOUNT_ID)
}

fn is_secondary_account_lease_update(notification: &JSONRPCNotification) -> bool {
    notification.method == "accountLease/updated"
        && notification
            .params
            .as_ref()
            .and_then(|params| params.get("accountId"))
            .and_then(serde_json::Value::as_str)
            == Some(SECONDARY_ACCOUNT_ID)
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
async fn account_lease_read_reports_shared_startup_selection_without_accounts_config() -> Result<()>
{
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml_without_accounts(codex_home.path(), &server.uri())?;
    let runtime = seed_default_pool_state(codex_home.path()).await?;
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("legacy-default".to_string()),
            preferred_account_id: Some(PRIMARY_ACCOUNT_ID.to_string()),
            suppressed: false,
        })
        .await?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(response.active, true);
    assert_eq!(response.account_id.as_deref(), Some(PRIMARY_ACCOUNT_ID));
    assert_eq!(response.pool_id.as_deref(), Some("legacy-default"));

    Ok(())
}

#[tokio::test]
async fn policy_only_config_allows_websocket_account_lease_read_startup_snapshot() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_policy_only_pooled_config_toml(codex_home.path(), &server.uri())?;
    seed_account_in_pool(codex_home.path(), PRIMARY_ACCOUNT_ID, "legacy-default", 0).await?;

    let (mut process, bind_addr) = spawn_websocket_server(codex_home.path()).await?;
    let mut ws = connect_websocket(bind_addr).await?;
    send_initialize_request(
        &mut ws,
        /*id*/ 1,
        "ws_policy_only_account_lease_client",
    )
    .await?;
    let init = read_response_for_id(&mut ws, /*id*/ 1).await?;
    assert_eq!(init.id, RequestId::Integer(1));

    send_request(
        &mut ws,
        "accountLease/read",
        /*id*/ 2,
        /*params*/ None,
    )
    .await?;
    let response: AccountLeaseReadResponse =
        to_response(read_response_for_id(&mut ws, /*id*/ 2).await?)?;
    assert_eq!(
        response.startup.startup_availability,
        AccountStartupAvailability::Available
    );
    assert_eq!(
        response.startup.effective_pool_id.as_deref(),
        Some("legacy-default")
    );

    process
        .kill()
        .await
        .context("failed to stop websocket app-server process")?;
    Ok(())
}

#[tokio::test]
async fn policy_only_config_allows_websocket_account_lease_resume_startup_intent() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_policy_only_pooled_config_toml(codex_home.path(), &server.uri())?;
    let runtime = seed_default_pool_state(codex_home.path()).await?;
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("legacy-default".to_string()),
            preferred_account_id: Some(PRIMARY_ACCOUNT_ID.to_string()),
            suppressed: true,
        })
        .await?;

    let (mut process, bind_addr) = spawn_websocket_server(codex_home.path()).await?;
    let mut ws = connect_websocket(bind_addr).await?;
    send_initialize_request(
        &mut ws,
        /*id*/ 1,
        "ws_policy_only_account_lease_resume_client",
    )
    .await?;
    let init = read_response_for_id(&mut ws, /*id*/ 1).await?;
    assert_eq!(init.id, RequestId::Integer(1));

    send_request(
        &mut ws,
        "accountLease/resume",
        /*id*/ 2,
        /*params*/ None,
    )
    .await?;
    let response = read_response_for_id(&mut ws, /*id*/ 2).await?;
    assert_eq!(response.id, RequestId::Integer(2));
    let notification = read_notification_for_method(&mut ws, "accountLease/updated").await?;
    let updated: AccountLeaseUpdatedNotification =
        serde_json::from_value(notification.params.expect("params must be present"))?;
    assert_eq!(
        updated.startup.startup_availability,
        AccountStartupAvailability::Available
    );
    assert_eq!(
        updated.startup.selection_eligibility,
        "automaticAccountSelected"
    );

    send_request(
        &mut ws,
        "thread/list",
        /*id*/ 3,
        Some(json!({
            "cursor": null,
            "limit": null,
            "sortKey": null,
            "modelProviders": null,
            "sourceKinds": null,
            "archived": null,
            "cwd": null,
            "searchTerm": null
        })),
    )
    .await?;
    let thread_list: codex_app_server_protocol::ThreadListResponse =
        to_response(read_response_for_id(&mut ws, /*id*/ 3).await?)?;
    assert_eq!(thread_list.data, Vec::new());
    let candidates = runtime
        .read_account_lease_selection_candidates("legacy-default")
        .await?;
    assert!(
        candidates
            .into_iter()
            .all(|(_, _, active_lease, _)| active_lease.is_none())
    );

    process
        .kill()
        .await
        .context("failed to stop websocket app-server process")?;
    Ok(())
}

#[tokio::test]
async fn account_lease_read_adds_resolution_fields_without_changing_legacy_fields() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_pooled_config_toml(codex_home.path(), &server.uri())?;
    let runtime = seed_default_pool_state(codex_home.path()).await?;
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("persisted-default".to_string()),
            preferred_account_id: None,
            suppressed: false,
        })
        .await?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(response.active, true);
    assert_eq!(response.account_id.as_deref(), Some(PRIMARY_ACCOUNT_ID));
    assert_eq!(response.pool_id.as_deref(), Some("legacy-default"));
    assert_eq!(
        response.switch_reason.as_deref(),
        Some("automaticAccountSelected")
    );
    assert_eq!(
        response.effective_pool_resolution_source.as_deref(),
        Some("configDefault")
    );
    assert_eq!(
        response.configured_default_pool_id.as_deref(),
        Some("legacy-default")
    );
    assert_eq!(
        response.persisted_default_pool_id.as_deref(),
        Some("persisted-default")
    );

    Ok(())
}

#[tokio::test]
async fn account_lease_read_reports_remote_reset_and_retry_suppressed_reason() -> Result<()> {
    let server = create_rate_limit_then_success_server().await;
    let codex_home = TempDir::new()?;
    create_pooled_config_toml(codex_home.path(), &server.uri())?;
    seed_two_accounts(codex_home.path()).await?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(ACCOUNT_LEASE_ROTATION_TIMEOUT, mcp.initialize())
        .await
        .context("timed out initializing pooled account reset test mcp process")??;

    let thread = start_thread_with_timeout(&mut mcp, ACCOUNT_LEASE_ROTATION_TIMEOUT).await?;
    let _first_turn = start_turn_with_timeout(
        &mut mcp,
        &thread.id,
        "rate limit me",
        ACCOUNT_LEASE_ROTATION_TIMEOUT,
    )
    .await?;
    timeout(
        ACCOUNT_LEASE_ROTATION_TIMEOUT,
        mcp.read_stream_until_notification_message("error"),
    )
    .await
    .context("timed out waiting for account limit error before reset preview")??;
    wait_for_terminal_thread_status_after_failed_turn(
        &mut mcp,
        &thread.id,
        ACCOUNT_LEASE_ROTATION_TIMEOUT,
    )
    .await?;

    let after_failure: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(
        after_failure.account_id.as_deref(),
        Some(PRIMARY_ACCOUNT_ID)
    );
    assert_eq!(
        after_failure.switch_reason.as_deref(),
        Some("nonReplayableTurn")
    );

    let second_turn = start_turn_with_timeout(
        &mut mcp,
        &thread.id,
        "recover on another account",
        ACCOUNT_LEASE_ROTATION_TIMEOUT,
    )
    .await?;
    timeout(
        ACCOUNT_LEASE_ROTATION_TIMEOUT,
        mcp.read_stream_until_matching_notification(
            "turn/completed for recovered turn",
            |notification| {
                notification.method == "turn/completed"
                    && notification
                        .params
                        .as_ref()
                        .and_then(|params| params.get("turn"))
                        .and_then(|turn| turn.get("id"))
                        .and_then(serde_json::Value::as_str)
                        == Some(second_turn.id.as_str())
            },
        ),
    )
    .await
    .context("timed out waiting for recovered turn/completed notification")??;

    let after_rotation: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(
        after_rotation.account_id.as_deref(),
        Some(SECONDARY_ACCOUNT_ID)
    );
    assert_eq!(
        after_rotation.switch_reason.as_deref(),
        Some("automaticAccountSelected")
    );
    assert_eq!(after_rotation.transport_reset_generation, Some(1));
    assert_eq!(
        after_rotation.last_remote_context_reset_turn_id.as_deref(),
        Some(second_turn.id.as_str())
    );

    Ok(())
}

#[tokio::test]
async fn pooled_mode_rejects_second_top_level_stdio_runtime_creation() -> Result<()> {
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
    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(second_id)),
    )
    .await??;

    assert!(
        error
            .error
            .data
            .as_ref()
            .and_then(|data| data.get("errorCode"))
            .and_then(serde_json::Value::as_str)
            == Some("pooledRuntimeAlreadyLoaded"),
        "unexpected error: {error:?}"
    );

    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(response.suppressed, false);

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
        "thread/start",
        /*id*/ 2,
        Some(json!(ThreadStartParams::default())),
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
    write_pooled_auth(codex_home, PRIMARY_ACCOUNT_ID, PRIMARY_ACCOUNT_ID)?;
    Ok(runtime)
}

async fn seed_account_in_pool(
    codex_home: &std::path::Path,
    account_id: &str,
    pool_id: &str,
    position: i64,
) -> Result<Arc<StateRuntime>> {
    let runtime = StateRuntime::init(codex_home.to_path_buf(), "mock_provider".to_string()).await?;
    runtime
        .upsert_account_registry_entry(AccountRegistryEntryUpdate {
            account_id: account_id.to_string(),
            pool_id: pool_id.to_string(),
            position,
            account_kind: "chatgpt".to_string(),
            backend_family: "local".to_string(),
            workspace_id: None,
            enabled: true,
            healthy: true,
        })
        .await?;
    write_pooled_auth(codex_home, account_id, account_id)?;
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

async fn start_thread(mcp: &mut McpProcess) -> Result<codex_app_server_protocol::Thread> {
    start_thread_with_timeout(mcp, DEFAULT_READ_TIMEOUT).await
}

async fn start_thread_with_config(
    mcp: &mut McpProcess,
    config: HashMap<String, serde_json::Value>,
) -> Result<codex_app_server_protocol::Thread> {
    let thread_id = mcp
        .send_thread_start_request(ThreadStartParams {
            config: Some(config),
            ..Default::default()
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_id)),
    )
    .await
    .context("timed out waiting for request-config thread/start response")??;
    let ThreadStartResponse { thread, .. } = to_response(response)?;
    Ok(thread)
}

async fn start_thread_with_timeout(
    mcp: &mut McpProcess,
    read_timeout: Duration,
) -> Result<codex_app_server_protocol::Thread> {
    let thread_id = mcp
        .send_thread_start_request(ThreadStartParams::default())
        .await?;
    let response: JSONRPCResponse = timeout(
        read_timeout,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_id)),
    )
    .await
    .context("timed out waiting for thread/start response")??;
    let ThreadStartResponse { thread, .. } = to_response(response)?;
    Ok(thread)
}

async fn start_turn(
    mcp: &mut McpProcess,
    thread_id: &str,
    text: &str,
) -> Result<codex_app_server_protocol::Turn> {
    start_turn_with_timeout(mcp, thread_id, text, DEFAULT_READ_TIMEOUT).await
}

async fn start_turn_with_timeout(
    mcp: &mut McpProcess,
    thread_id: &str,
    text: &str,
    read_timeout: Duration,
) -> Result<codex_app_server_protocol::Turn> {
    let request_id = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread_id.to_string(),
            input: vec![UserInput::Text {
                text: text.to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        read_timeout,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await
    .context("timed out waiting for turn/start response")??;
    let TurnStartResponse { turn } = to_response(response)?;
    Ok(turn)
}

async fn wait_for_terminal_thread_status_after_failed_turn(
    mcp: &mut McpProcess,
    thread_id: &str,
    read_timeout: Duration,
) -> Result<()> {
    timeout(
        read_timeout,
        mcp.read_stream_until_matching_notification(
            "terminal thread/status/changed after failed turn",
            |notification| {
                if notification.method != "thread/status/changed" {
                    return false;
                }
                let Some(params) = notification.params.as_ref() else {
                    return false;
                };
                let Ok(status_changed) =
                    serde_json::from_value::<ThreadStatusChangedNotification>(params.clone())
                else {
                    return false;
                };
                status_changed.thread_id == thread_id
                    && matches!(
                        status_changed.status,
                        ThreadStatus::Idle | ThreadStatus::SystemError | ThreadStatus::NotLoaded
                    )
            },
        ),
    )
    .await
    .context("timed out waiting for terminal thread/status/changed after failed turn")??;
    Ok(())
}

async fn create_rate_limit_then_success_server() -> wiremock::MockServer {
    struct SeqResponder {
        call_count: std::sync::atomic::AtomicUsize,
        responses: Vec<ResponseTemplate>,
    }

    impl Respond for SeqResponder {
        fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
            let call_index = self
                .call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.responses
                .get(call_index)
                .cloned()
                .unwrap_or_else(|| panic!("missing mock response for call {call_index}"))
        }
    }

    let server = core_test_support::responses::start_mock_server().await;
    Mock::given(method("POST"))
        .and(path_regex(".*/responses$"))
        .respond_with(SeqResponder {
            call_count: std::sync::atomic::AtomicUsize::new(0),
            responses: vec![
                ResponseTemplate::new(429)
                    .insert_header("content-type", "application/json")
                    .insert_header("x-codex-primary-used-percent", "100.0")
                    .insert_header("x-codex-primary-window-minutes", "15")
                    .set_body_json(json!({
                        "error": {
                            "type": "usage_limit_reached",
                            "message": "limit reached",
                            "resets_at": 1704067242
                        }
                    })),
                core_test_support::responses::sse_response(core_test_support::responses::sse(
                    vec![
                        core_test_support::responses::ev_response_created("resp-2"),
                        core_test_support::responses::ev_assistant_message("msg-2", "Done"),
                        core_test_support::responses::ev_completed("resp-2"),
                    ],
                )),
            ],
        })
        .mount(&server)
        .await;
    server
}

fn create_pooled_config_toml(
    codex_home: &std::path::Path,
    server_uri: &str,
) -> std::io::Result<()> {
    write_pooled_config_toml(
        codex_home,
        server_uri,
        PooledConfigTomlOptions {
            default_pool: "legacy-default",
            min_switch_interval_secs: 0,
        },
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

fn create_pooled_config_toml_without_default(
    codex_home: &std::path::Path,
    server_uri: &str,
    pool_ids: &[&str],
) -> std::io::Result<()> {
    let pool_sections = pool_ids
        .iter()
        .map(|pool_id| {
            format!(
                r#"
[accounts.pools.{pool_id}]
allow_context_reuse = false
account_kinds = ["chatgpt"]
"#
            )
        })
        .collect::<String>();
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
allocation_mode = "exclusive"
{pool_sections}"#
        ),
    )
}

fn pooled_accounts_request_config() -> HashMap<String, serde_json::Value> {
    HashMap::from([
        ("accounts.backend".to_string(), json!("local")),
        ("accounts.default_pool".to_string(), json!("legacy-default")),
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

fn create_policy_only_pooled_config_toml(
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
allocation_mode = "exclusive"

[accounts.pools.legacy-default]
allow_context_reuse = false
account_kinds = ["chatgpt"]
"#
        ),
    )
}

fn create_pooled_config_toml_with_default_pool(
    codex_home: &std::path::Path,
    server_uri: &str,
    default_pool: &str,
) -> std::io::Result<()> {
    write_pooled_config_toml(
        codex_home,
        server_uri,
        PooledConfigTomlOptions {
            default_pool,
            min_switch_interval_secs: 0,
        },
    )
}

fn create_pooled_config_toml_with_min_switch_interval(
    codex_home: &std::path::Path,
    server_uri: &str,
    min_switch_interval_secs: u64,
) -> std::io::Result<()> {
    write_pooled_config_toml(
        codex_home,
        server_uri,
        PooledConfigTomlOptions {
            default_pool: "legacy-default",
            min_switch_interval_secs,
        },
    )
}

struct PooledConfigTomlOptions<'a> {
    default_pool: &'a str,
    min_switch_interval_secs: u64,
}

fn write_pooled_config_toml(
    codex_home: &std::path::Path,
    server_uri: &str,
    options: PooledConfigTomlOptions<'_>,
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
min_switch_interval_secs = {min_switch_interval_secs}

[accounts.pools.legacy-default]
allow_context_reuse = false
account_kinds = ["chatgpt"]

[accounts.pools.config-default]
allow_context_reuse = false
account_kinds = ["chatgpt"]

[accounts.pools.persisted-default]
allow_context_reuse = false
account_kinds = ["chatgpt"]
"#,
            default_pool = options.default_pool,
            min_switch_interval_secs = options.min_switch_interval_secs,
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
