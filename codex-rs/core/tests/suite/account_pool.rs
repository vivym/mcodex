use anyhow::Result;
use base64::Engine;
use chrono::Utc;
use codex_config::types::AccountPoolDefinitionToml;
use codex_config::types::AccountsConfigToml;
use codex_core::AccountLeaseRuntimeReason;
use codex_login::AuthCredentialsStoreMode;
use codex_login::AuthDotJson;
use codex_login::CodexAuth;
use codex_login::TokenData;
use codex_login::save_auth;
use codex_login::token_data::parse_chatgpt_jwt_claims;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::built_in_model_providers;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use codex_state::AccountHealthEvent;
use codex_state::AccountHealthState;
use codex_state::AccountStartupSelectionUpdate;
use codex_state::LegacyAccountImport;
use core_test_support::responses::ResponseMock;
use core_test_support::responses::ResponsesRequest;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_completed_with_tokens;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_response_sequence;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use core_test_support::wait_for_event_match;
use core_test_support::wait_for_event_with_timeout;
use http::Method;
use pretty_assertions::assert_eq;
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use wiremock::Mock;
use wiremock::Respond;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path_regex;

const PRIMARY_ACCOUNT_ID: &str = "account_id";
const SECONDARY_ACCOUNT_ID: &str = "account_id_b";
const LEGACY_DEFAULT_POOL_ID: &str = "legacy-default";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn account_lease_snapshot_reports_active_lease_and_next_eligible_time() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("m1", "snapshot active lease"),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let mut builder = pooled_accounts_builder();
    let test = builder.build(&server).await?;
    seed_account(&test, PRIMARY_ACCOUNT_ID).await?;
    let Some(state_db) = test.codex.state_db() else {
        return Err(anyhow::anyhow!(
            "state db should be available in core integration tests"
        ));
    };
    state_db
        .record_account_health_event(AccountHealthEvent {
            account_id: PRIMARY_ACCOUNT_ID.to_string(),
            pool_id: LEGACY_DEFAULT_POOL_ID.to_string(),
            health_state: AccountHealthState::Healthy,
            sequence_number: 1,
            observed_at: chrono::Utc::now(),
        })
        .await?;

    let turn_error = submit_turn_and_wait(&test, "snapshot turn").await?;
    assert!(turn_error.is_none());

    let snapshot = test
        .codex
        .account_lease_snapshot()
        .await
        .expect("pooled session should expose lease snapshot");
    assert_eq!(snapshot.active, true);
    assert_eq!(snapshot.suppressed, false);
    assert_eq!(snapshot.account_id.as_deref(), Some(PRIMARY_ACCOUNT_ID));
    assert_eq!(snapshot.pool_id.as_deref(), Some(LEGACY_DEFAULT_POOL_ID));
    assert!(snapshot.lease_id.is_some());
    assert_eq!(snapshot.lease_epoch, Some(1));
    assert_eq!(snapshot.health_state, Some(AccountHealthState::Healthy));
    assert_eq!(snapshot.transport_reset_generation, None);
    assert!(snapshot.next_eligible_at.is_some());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn account_lease_snapshot_records_remote_reset_generation_when_account_changes() -> Result<()>
{
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    mount_response_sequence(
        &server,
        vec![
            ResponseTemplate::new(429)
                .insert_header("content-type", "application/json")
                .insert_header("x-codex-primary-used-percent", "100.0")
                .insert_header("x-codex-primary-window-minutes", "15")
                .set_body_json(json!({
                    "error": {
                        "type": "usage_limit_reached",
                        "message": "limit reached"
                    }
                })),
            sse_with_primary_usage_percent("resp-2", 10.0),
        ],
    )
    .await;

    let mut builder = pooled_accounts_builder();
    let test = builder.build(&server).await?;
    seed_two_accounts(&test).await?;

    let first_turn_error = submit_turn_and_wait(&test, "rotate snapshot").await?;
    assert!(
        first_turn_error.is_some(),
        "turn 1 should fail with usage-limit"
    );

    let second_turn_error = submit_turn_and_wait(&test, "post-rotate snapshot").await?;
    assert!(second_turn_error.is_none());

    let snapshot = test
        .codex
        .account_lease_snapshot()
        .await
        .expect("pooled session should expose lease snapshot");
    assert_eq!(snapshot.account_id.as_deref(), Some(SECONDARY_ACCOUNT_ID));
    assert_eq!(snapshot.transport_reset_generation, Some(1));
    assert!(snapshot.last_remote_context_reset_turn_id.is_some());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn account_lease_snapshot_clears_revoked_live_lease_after_external_disable() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("m1", "snapshot revoked lease"),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let mut builder = pooled_accounts_builder();
    let test = builder.build(&server).await?;
    seed_account(&test, PRIMARY_ACCOUNT_ID).await?;
    let Some(state_db) = test.codex.state_db() else {
        return Err(anyhow::anyhow!(
            "state db should be available in core integration tests"
        ));
    };
    state_db
        .record_account_health_event(AccountHealthEvent {
            account_id: PRIMARY_ACCOUNT_ID.to_string(),
            pool_id: LEGACY_DEFAULT_POOL_ID.to_string(),
            health_state: AccountHealthState::Healthy,
            sequence_number: 1,
            observed_at: chrono::Utc::now(),
        })
        .await?;

    let turn_error = submit_turn_and_wait(&test, "snapshot revoked lease").await?;
    assert!(turn_error.is_none());

    let active_snapshot = test
        .codex
        .account_lease_snapshot()
        .await
        .expect("pooled session should expose lease snapshot");
    assert_eq!(active_snapshot.active, true);
    assert_eq!(
        active_snapshot.account_id.as_deref(),
        Some(PRIMARY_ACCOUNT_ID)
    );
    assert!(active_snapshot.lease_id.is_some());
    assert_eq!(active_snapshot.lease_epoch, Some(1));
    assert_eq!(
        active_snapshot.health_state,
        Some(AccountHealthState::Healthy)
    );
    assert!(active_snapshot.next_eligible_at.is_some());

    assert!(
        state_db
            .set_account_enabled(PRIMARY_ACCOUNT_ID, false)
            .await?,
        "external disable should update the pooled account state"
    );

    let snapshot = test
        .codex
        .account_lease_snapshot()
        .await
        .expect("pooled session should expose lease snapshot");
    assert_eq!(snapshot.active, false);
    assert_eq!(snapshot.account_id, None);
    assert_eq!(snapshot.pool_id, None);
    assert_eq!(snapshot.lease_id, None);
    assert_eq!(snapshot.lease_epoch, None);
    assert_eq!(snapshot.health_state, None);
    assert_eq!(snapshot.next_eligible_at, None);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn account_lease_snapshot_clears_pending_non_replayable_turn_reason_after_rotation()
-> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    mount_response_sequence(
        &server,
        vec![
            ResponseTemplate::new(429)
                .insert_header("content-type", "application/json")
                .insert_header("x-codex-primary-used-percent", "100.0")
                .insert_header("x-codex-primary-window-minutes", "15")
                .set_body_json(json!({
                    "error": {
                        "type": "usage_limit_reached",
                        "message": "limit reached"
                    }
                })),
            sse_with_primary_usage_percent("resp-2", 10.0),
        ],
    )
    .await;

    let mut builder = pooled_accounts_builder();
    let test = builder.build(&server).await?;
    seed_two_accounts(&test).await?;

    let first_turn_error = submit_turn_and_wait(&test, "rotate snapshot").await?;
    assert!(
        first_turn_error.is_some(),
        "turn 1 should fail with usage-limit"
    );

    let first_snapshot = test
        .codex
        .account_lease_snapshot()
        .await
        .expect("pooled session should expose lease snapshot");
    assert_eq!(first_snapshot.active, true);
    assert_eq!(
        first_snapshot.account_id.as_deref(),
        Some(PRIMARY_ACCOUNT_ID)
    );
    assert_eq!(
        first_snapshot.switch_reason,
        Some(AccountLeaseRuntimeReason::NonReplayableTurn)
    );
    assert_eq!(first_snapshot.suppression_reason, None);

    let second_turn_error = submit_turn_and_wait(&test, "post-rotate snapshot").await?;
    assert!(second_turn_error.is_none());

    let second_snapshot = test
        .codex
        .account_lease_snapshot()
        .await
        .expect("pooled session should expose lease snapshot");
    assert_eq!(second_snapshot.active, true);
    assert_eq!(
        second_snapshot.account_id.as_deref(),
        Some(SECONDARY_ACCOUNT_ID)
    );
    assert_ne!(
        second_snapshot.switch_reason,
        Some(AccountLeaseRuntimeReason::NonReplayableTurn)
    );
    assert_eq!(second_snapshot.suppression_reason, None);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nearing_limit_snapshot_rotates_the_next_turn_before_exhaustion() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let response_mock = mount_response_sequence(
        &server,
        vec![
            sse_with_primary_usage_percent("resp-1", 92.0),
            sse_with_primary_usage_percent("resp-2", 15.0),
        ],
    )
    .await;

    let mut builder = pooled_accounts_builder();
    let test = builder.build(&server).await?;
    seed_two_accounts(&test).await?;

    let first_turn_error = submit_turn_and_wait(&test, "near-limit turn").await?;
    assert!(first_turn_error.is_none());
    wait_for_account_health_transition(
        &test,
        AccountHealthState::RateLimited,
        AccountLeaseRuntimeReason::NonReplayableTurn,
    )
    .await?;

    let second_turn_error = submit_turn_and_wait(&test, "post-rotation turn").await?;
    assert!(second_turn_error.is_none());

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2, "expected one request per turn");
    assert_account_ids_in_order(&requests, &[PRIMARY_ACCOUNT_ID, SECONDARY_ACCOUNT_ID]);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn usage_limit_reached_rotates_only_future_turns_on_responses_transport() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let response_mock = mount_response_sequence(
        &server,
        vec![
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
            sse_with_primary_usage_percent("resp-2", 12.0),
        ],
    )
    .await;

    let mut builder = pooled_accounts_builder();
    let test = builder.build(&server).await?;
    seed_two_accounts(&test).await?;

    let first_turn_error = submit_turn_and_wait(&test, "usage-limit turn").await?;
    assert!(
        first_turn_error.is_some(),
        "expected usage-limit error on turn 1"
    );
    assert_eq!(
        response_mock.requests().len(),
        1,
        "usage-limit failure should not auto-replay the current turn"
    );

    let second_turn_error = submit_turn_and_wait(&test, "follow-up turn").await?;
    assert!(second_turn_error.is_none());

    let requests = response_mock.requests();
    assert_eq!(
        requests.len(),
        2,
        "current turn should not be auto-replayed"
    );
    assert_account_ids_in_order(&requests, &[PRIMARY_ACCOUNT_ID, SECONDARY_ACCOUNT_ID]);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthorized_failure_marks_account_unavailable_for_next_turn() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let response_mock = mount_response_sequence(
        &server,
        vec![
            ResponseTemplate::new(401)
                .insert_header("content-type", "application/json")
                .set_body_string("unauthorized"),
            ResponseTemplate::new(401)
                .insert_header("content-type", "application/json")
                .set_body_string("unauthorized"),
            sse_with_primary_usage_percent("resp-2", 11.0),
        ],
    )
    .await;

    let mut builder = pooled_accounts_builder();
    let test = builder.build(&server).await?;
    seed_two_accounts(&test).await?;

    let first_turn_error = submit_turn_and_wait(&test, "unauthorized turn").await?;
    assert_eq!(
        first_turn_error
            .as_ref()
            .and_then(|err| err.codex_error_info.clone()),
        Some(CodexErrorInfo::Unauthorized)
    );

    let second_turn_error = submit_turn_and_wait(&test, "after unauthorized").await?;
    assert!(second_turn_error.is_none());

    let requests = response_mock.requests();
    assert_eq!(
        requests.len(),
        3,
        "expected one in-turn unauthorized retry before next-turn rotation"
    );
    assert_account_ids_in_order(
        &requests,
        &[PRIMARY_ACCOUNT_ID, PRIMARY_ACCOUNT_ID, SECONDARY_ACCOUNT_ID],
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthorized_retry_uses_leased_auth_session_not_shared_auth_manager() -> Result<()> {
    skip_if_no_network!(Ok(()));

    struct UnauthorizedThenRefreshedLeaseResponder {
        next_call: std::sync::atomic::AtomicUsize,
        codex_home: std::path::PathBuf,
    }

    impl Respond for UnauthorizedThenRefreshedLeaseResponder {
        fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
            let call_index = self
                .next_call
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match call_index {
                0 => {
                    write_pooled_auth(
                        self.codex_home.as_path(),
                        PRIMARY_ACCOUNT_ID,
                        PRIMARY_ACCOUNT_ID,
                        "pooled-access-refreshed",
                    )
                    .unwrap_or_else(|err| panic!("refresh pooled auth for retry: {err}"));
                    ResponseTemplate::new(401)
                        .insert_header("content-type", "application/json")
                        .set_body_string("unauthorized")
                }
                1 => sse_with_primary_usage_percent("resp-1", 11.0),
                _ => panic!("unexpected responses request {call_index}"),
            }
        }
    }

    let server = start_mock_server().await;

    let shared_home = Arc::new(TempDir::new()?);
    write_shared_auth(
        shared_home.path(),
        "shared-account",
        "shared-access-initial",
    )?;
    let shared_auth =
        CodexAuth::from_auth_storage(shared_home.path(), AuthCredentialsStoreMode::File)?
            .expect("expected shared auth from tempdir");

    let mut builder = pooled_accounts_builder()
        .with_home(Arc::clone(&shared_home))
        .with_auth(shared_auth);
    let test = builder.build(&server).await?;
    seed_account(&test, PRIMARY_ACCOUNT_ID).await?;
    write_pooled_auth(
        test.codex_home_path(),
        PRIMARY_ACCOUNT_ID,
        PRIMARY_ACCOUNT_ID,
        "pooled-access-stale",
    )?;

    Mock::given(method("POST"))
        .and(path_regex(".*/responses$"))
        .respond_with(UnauthorizedThenRefreshedLeaseResponder {
            next_call: std::sync::atomic::AtomicUsize::new(0),
            codex_home: test.codex_home_path().to_path_buf(),
        })
        .mount(&server)
        .await;

    let turn_error = submit_turn_and_wait(&test, "unauthorized retry").await?;
    assert!(turn_error.is_none());

    let response_requests = server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| {
            request.method == Method::POST && request.url.path().ends_with("/responses")
        })
        .collect::<Vec<_>>();
    assert_eq!(response_requests.len(), 2);

    let account_ids = response_requests
        .iter()
        .map(|request| {
            request
                .headers
                .get("chatgpt-account-id")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        account_ids,
        vec![
            Some(PRIMARY_ACCOUNT_ID.to_string()),
            Some(PRIMARY_ACCOUNT_ID.to_string())
        ]
    );

    let auth_headers = response_requests
        .iter()
        .map(|request| {
            request
                .headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        auth_headers,
        vec![
            Some("Bearer pooled-access-stale".to_string()),
            Some("Bearer pooled-access-refreshed".to_string())
        ]
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rotation_without_context_reuse_mints_new_remote_session_id() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let response_mock = mount_response_sequence(
        &server,
        vec![
            ResponseTemplate::new(429)
                .insert_header("content-type", "application/json")
                .insert_header("x-codex-primary-used-percent", "100.0")
                .insert_header("x-codex-primary-window-minutes", "15")
                .set_body_json(json!({
                    "error": {
                        "type": "usage_limit_reached",
                        "message": "limit reached"
                    }
                })),
            sse_with_primary_usage_percent("resp-2", 10.0),
        ],
    )
    .await;

    let mut builder = pooled_accounts_builder();
    let test = builder.build(&server).await?;
    seed_two_accounts(&test).await?;

    let first_turn_error = submit_turn_and_wait(&test, "rotate-without-context-reuse").await?;
    assert!(
        first_turn_error.is_some(),
        "turn 1 should fail with usage-limit"
    );

    let second_turn_error = submit_turn_and_wait(&test, "post-rotate").await?;
    assert!(second_turn_error.is_none());

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2);
    assert_account_ids_in_order(&requests, &[PRIMARY_ACCOUNT_ID, SECONDARY_ACCOUNT_ID]);

    let first_session_id = requests[0]
        .header("session_id")
        .expect("first request missing session_id header");
    let second_session_id = requests[1]
        .header("session_id")
        .expect("second request missing session_id header");
    assert_ne!(
        first_session_id, second_session_id,
        "rotation without context reuse should mint a fresh remote session id"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_selected_pool_without_context_reuse_mints_new_remote_session_id() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let response_mock = mount_response_sequence(
        &server,
        vec![
            ResponseTemplate::new(429)
                .insert_header("content-type", "application/json")
                .insert_header("x-codex-primary-used-percent", "100.0")
                .insert_header("x-codex-primary-window-minutes", "15")
                .set_body_json(json!({
                    "error": {
                        "type": "usage_limit_reached",
                        "message": "limit reached"
                    }
                })),
            sse_with_primary_usage_percent("resp-2", 10.0),
        ],
    )
    .await;

    let mut builder = test_codex()
        .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
        .with_config(|config| {
            config.accounts = Some(accounts_config_without_default_pool());
        });
    let test = builder.build(&server).await?;
    seed_two_accounts(&test).await?;
    let Some(state_db) = test.codex.state_db() else {
        return Err(anyhow::anyhow!(
            "state db should be available in core integration tests"
        ));
    };
    state_db
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some(LEGACY_DEFAULT_POOL_ID.to_string()),
            preferred_account_id: None,
            suppressed: false,
        })
        .await?;

    let first_turn_error = submit_turn_and_wait(&test, "rotate-startup-selected-pool").await?;
    assert!(
        first_turn_error.is_some(),
        "turn 1 should fail with usage-limit"
    );

    let second_turn_error =
        submit_turn_and_wait(&test, "post-rotate-startup-selected-pool").await?;
    assert!(second_turn_error.is_none());

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2);
    assert_account_ids_in_order(&requests, &[PRIMARY_ACCOUNT_ID, SECONDARY_ACCOUNT_ID]);

    let first_session_id = requests[0]
        .header("session_id")
        .expect("first request missing session_id header");
    let second_session_id = requests[1]
        .header("session_id")
        .expect("second request missing session_id header");
    assert_ne!(
        first_session_id, second_session_id,
        "startup-selected pool with context reuse disabled should mint a fresh remote session id"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn suppressed_startup_selection_blocks_fresh_runtime_pool_acquisition() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mut builder = pooled_accounts_builder();
    let test = builder.build(&server).await?;
    seed_two_accounts(&test).await?;
    let Some(state_db) = test.codex.state_db() else {
        return Err(anyhow::anyhow!(
            "state db should be available in core integration tests"
        ));
    };
    state_db
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some(LEGACY_DEFAULT_POOL_ID.to_string()),
            preferred_account_id: Some(PRIMARY_ACCOUNT_ID.to_string()),
            suppressed: true,
        })
        .await?;

    let turn_error = submit_turn_and_wait(&test, "suppressed pooled turn").await?;
    let turn_error = turn_error.expect("suppressed fresh runtime should fail closed");
    assert!(
        turn_error
            .message
            .to_ascii_lowercase()
            .contains("pooled account"),
        "unexpected suppression error: {}",
        turn_error.message
    );

    let requests = server.received_requests().await.unwrap_or_default();
    let responses_requests = requests
        .iter()
        .filter(|request| {
            request.method == Method::POST && request.url.path().ends_with("/responses")
        })
        .count();
    assert_eq!(
        responses_requests, 0,
        "suppressed fresh runtime should not acquire or use a pooled account"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preferred_startup_selection_is_used_for_fresh_runtime() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let response_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("m1", "preferred runtime"),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let mut builder = pooled_accounts_builder();
    let test = builder.build(&server).await?;
    seed_two_accounts(&test).await?;
    let Some(state_db) = test.codex.state_db() else {
        return Err(anyhow::anyhow!(
            "state db should be available in core integration tests"
        ));
    };
    state_db
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some(LEGACY_DEFAULT_POOL_ID.to_string()),
            preferred_account_id: Some(SECONDARY_ACCOUNT_ID.to_string()),
            suppressed: false,
        })
        .await?;

    let turn_error = submit_turn_and_wait(&test, "preferred pooled turn").await?;
    assert!(turn_error.is_none());
    assert_account_ids_in_order(&response_mock.requests(), &[SECONDARY_ACCOUNT_ID]);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exhausted_pool_fails_closed_without_legacy_auth_fallback() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let mut builder = pooled_accounts_builder();
    let test = builder.build(&server).await?;

    let first_turn_error = submit_turn_and_wait(&test, "no eligible pooled account turn").await?;
    assert!(
        first_turn_error.is_some(),
        "pooled mode should fail closed when no eligible account is available"
    );
    let error_message = first_turn_error
        .as_ref()
        .map(|event| event.message.to_ascii_lowercase())
        .unwrap_or_default();
    assert!(
        error_message.contains("pooled account"),
        "unexpected pooled-mode exhaustion message: {error_message}"
    );
    let requests = server.received_requests().await.unwrap_or_default();
    let responses_requests = requests
        .iter()
        .filter(|request| {
            request.method == Method::POST && request.url.path().ends_with("/responses")
        })
        .count();
    assert_eq!(
        responses_requests, 0,
        "pooled mode should not send a fallback request with shared auth"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_compact_in_pooled_mode_does_not_fail_closed_without_eligible_lease() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let compact_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-compact"),
            ev_assistant_message("m-compact", "local compact summary"),
            ev_completed("resp-compact"),
        ]),
    )
    .await;

    let model_provider = non_openai_model_provider(&server);
    let mut builder = pooled_accounts_builder().with_config(move |config| {
        config.model_provider = model_provider;
    });
    let test = builder.build(&server).await?;

    test.codex.submit(Op::Compact).await?;

    let mut pooled_fail_closed_error = None;
    loop {
        let event = wait_for_event(&test.codex, |_| true).await;
        match event {
            EventMsg::Error(error_event) => {
                if error_event
                    .message
                    .to_ascii_lowercase()
                    .contains("pooled account")
                {
                    pooled_fail_closed_error = Some(error_event.message);
                }
            }
            EventMsg::TurnComplete(_) => break,
            _ => {}
        }
    }
    assert!(
        pooled_fail_closed_error.is_none(),
        "local compact should not fail closed due to pooled lease selection"
    );
    assert_eq!(
        compact_mock.requests().len(),
        1,
        "local compact should still execute when no pooled lease is available"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pre_turn_remote_compact_usage_limit_reached_rotates_next_turn() -> Result<()> {
    skip_if_no_network!(Ok(()));

    assert_pre_turn_remote_compact_failure_rotates_next_turn(
        AccountHealthState::RateLimited,
        ResponseTemplate::new(429)
            .insert_header("content-type", "application/json")
            .set_body_json(json!({
                "error": {
                    "type": "usage_limit_reached",
                    "message": "limit reached",
                    "resets_at": 1704067242
                }
            })),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pre_turn_remote_compact_refresh_failure_rotates_next_turn() -> Result<()> {
    skip_if_no_network!(Ok(()));

    assert_pre_turn_remote_compact_failure_rotates_next_turn(
        AccountHealthState::Unauthorized,
        ResponseTemplate::new(401)
            .insert_header("content-type", "application/json")
            .set_body_string("unauthorized"),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_releases_active_lease_for_next_runtime() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_assistant_message("m1", "runtime one"),
                ev_completed("resp-1"),
            ]),
            sse(vec![
                ev_response_created("resp-2"),
                ev_assistant_message("m2", "runtime two"),
                ev_completed("resp-2"),
            ]),
        ],
    )
    .await;

    let shared_home = Arc::new(TempDir::new()?);
    let mut first_builder = pooled_accounts_builder().with_home(Arc::clone(&shared_home));
    let first = first_builder.build(&server).await?;
    seed_account(&first, PRIMARY_ACCOUNT_ID).await?;

    let first_turn_error = submit_turn_and_wait(&first, "first runtime turn").await?;
    assert!(first_turn_error.is_none());

    first.codex.submit(Op::Shutdown {}).await?;
    wait_for_event(&first.codex, |event| {
        matches!(event, EventMsg::ShutdownComplete)
    })
    .await;

    let mut second_builder = pooled_accounts_builder().with_home(shared_home);
    let second = second_builder.build(&server).await?;
    let second_turn_error = submit_turn_and_wait(&second, "second runtime turn").await?;
    assert!(
        second_turn_error.is_none(),
        "shutdown should release pooled account lease for immediate reuse"
    );

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2, "expected one request per runtime turn");
    assert_account_ids_in_order(&requests, &[PRIMARY_ACCOUNT_ID, PRIMARY_ACCOUNT_ID]);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pooled_request_uses_lease_scoped_auth_session_not_shared_auth_snapshot() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let shared_home = Arc::new(TempDir::new()?);
    write_shared_auth(
        shared_home.path(),
        "shared-account",
        "shared-access-initial",
    )?;
    let shared_auth =
        CodexAuth::from_auth_storage(shared_home.path(), AuthCredentialsStoreMode::File)?
            .expect("expected shared auth from tempdir");

    let server = start_mock_server().await;
    let response_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("m1", "lease auth turn"),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let mut builder = pooled_accounts_builder()
        .with_home(Arc::clone(&shared_home))
        .with_auth(shared_auth);
    let test = builder.build(&server).await?;
    seed_account(&test, PRIMARY_ACCOUNT_ID).await?;
    write_pooled_auth(
        test.codex_home_path(),
        PRIMARY_ACCOUNT_ID,
        PRIMARY_ACCOUNT_ID,
        "pooled-access-primary",
    )?;

    let turn_error = submit_turn_and_wait(&test, "lease auth turn").await?;
    assert!(turn_error.is_none());

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 1, "expected exactly one pooled request");
    let response_auth_headers = requests
        .iter()
        .map(|request| request.header("authorization"))
        .collect::<Vec<_>>();
    assert_eq!(
        response_auth_headers,
        vec![Some("Bearer pooled-access-primary".to_string())],
        "pooled request should use the lease-scoped auth snapshot instead of shared auth"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pooled_request_ignores_shared_external_auth_when_lease_is_active() -> Result<()> {
    skip_if_no_network!(Ok(()));

    #[derive(Debug)]
    struct FixedExternalApiKeyAuth {
        api_key: String,
    }

    #[async_trait::async_trait]
    impl codex_login::ExternalAuth for FixedExternalApiKeyAuth {
        fn auth_mode(&self) -> codex_app_server_protocol::AuthMode {
            codex_app_server_protocol::AuthMode::ApiKey
        }

        async fn resolve(&self) -> std::io::Result<Option<codex_login::ExternalAuthTokens>> {
            Ok(Some(codex_login::ExternalAuthTokens::access_token_only(
                self.api_key.clone(),
            )))
        }

        async fn refresh(
            &self,
            _context: codex_login::ExternalAuthRefreshContext,
        ) -> std::io::Result<codex_login::ExternalAuthTokens> {
            Ok(codex_login::ExternalAuthTokens::access_token_only(
                self.api_key.clone(),
            ))
        }
    }

    let server = start_mock_server().await;
    let shared_home = Arc::new(TempDir::new()?);
    let mut builder = pooled_accounts_builder().with_home(Arc::clone(&shared_home));
    let test = builder.build(&server).await?;
    seed_account(&test, PRIMARY_ACCOUNT_ID).await?;
    let pooled_auth_home = test
        .codex_home_path()
        .join(".pooled-auth/backends/local/accounts")
        .join(PRIMARY_ACCOUNT_ID);
    write_chatgpt_auth(
        pooled_auth_home.as_path(),
        PRIMARY_ACCOUNT_ID,
        "pooled-access-initial",
    )?;
    test.thread_manager
        .auth_manager()
        .set_external_auth(Arc::new(FixedExternalApiKeyAuth {
            api_key: "shared-external-api-key".to_string(),
        }));

    let response_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("m1", "lease auth turn"),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let turn_error = submit_turn_and_wait(&test, "lease auth turn").await?;
    assert!(turn_error.is_none());

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 1, "expected exactly one pooled request");
    let auth_headers = requests
        .iter()
        .map(|request| request.header("authorization"))
        .collect::<Vec<_>>();
    assert_eq!(
        auth_headers,
        vec![Some("Bearer pooled-access-initial".to_string())]
    );

    let account_ids = requests
        .iter()
        .map(|request| request.header("chatgpt-account-id"))
        .collect::<Vec<_>>();
    assert_eq!(account_ids, vec![Some(PRIMARY_ACCOUNT_ID.to_string())]);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lease_rotation_rebinds_fresh_non_request_auth_reads_to_the_new_lease() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    mount_response_sequence(
        &server,
        vec![
            sse_with_primary_usage_percent("resp-1", 92.0),
            sse_with_primary_usage_percent("resp-2", 15.0),
        ],
    )
    .await;

    let shared_home = Arc::new(TempDir::new()?);
    write_shared_auth(
        shared_home.path(),
        "shared-account",
        "shared-access-initial",
    )?;
    let shared_auth =
        CodexAuth::from_auth_storage(shared_home.path(), AuthCredentialsStoreMode::File)?
            .expect("expected shared auth from tempdir");

    let mut builder = pooled_accounts_builder()
        .with_home(Arc::clone(&shared_home))
        .with_auth(shared_auth);
    let test = builder.build(&server).await?;
    seed_two_accounts(&test).await?;
    write_pooled_auth(
        test.codex_home_path(),
        PRIMARY_ACCOUNT_ID,
        PRIMARY_ACCOUNT_ID,
        "pooled-access-primary",
    )?;
    write_pooled_auth(
        test.codex_home_path(),
        SECONDARY_ACCOUNT_ID,
        SECONDARY_ACCOUNT_ID,
        "pooled-access-secondary",
    )?;

    let first_turn_error = submit_turn_and_wait(&test, "near-limit turn").await?;
    assert!(first_turn_error.is_none());
    wait_for_account_health_transition(
        &test,
        AccountHealthState::RateLimited,
        AccountLeaseRuntimeReason::NonReplayableTurn,
    )
    .await?;
    assert_eq!(
        test.codex
            .current_lease_bridge_account_id()
            .await
            .as_deref(),
        Some(PRIMARY_ACCOUNT_ID)
    );

    let second_turn_error = submit_turn_and_wait(&test, "post-rotation turn").await?;
    assert!(second_turn_error.is_none());
    assert_eq!(
        test.codex
            .current_lease_bridge_account_id()
            .await
            .as_deref(),
        Some(SECONDARY_ACCOUNT_ID)
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn long_running_turn_heartbeat_keeps_lease_exclusive() -> Result<()> {
    skip_if_no_network!(Ok(()));

    struct SeqResponder {
        next_call: std::sync::atomic::AtomicUsize,
        responses: Vec<ResponseTemplate>,
    }

    impl Respond for SeqResponder {
        fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
            let call_index = self
                .next_call
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.responses
                .get(call_index)
                .cloned()
                .unwrap_or_else(|| panic!("missing responses response for call {call_index}"))
        }
    }

    let server = start_mock_server().await;
    Mock::given(method("POST"))
        .and(path_regex(".*/responses$"))
        .respond_with(SeqResponder {
            next_call: std::sync::atomic::AtomicUsize::new(0),
            responses: vec![
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_delay(Duration::from_secs(8))
                    .set_body_raw(
                        sse(vec![
                            ev_response_created("resp-1"),
                            ev_assistant_message("m1", "long running turn"),
                            ev_completed("resp-1"),
                        ]),
                        "text/event-stream",
                    ),
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(
                        sse(vec![
                            ev_response_created("resp-2"),
                            ev_assistant_message("m2", "contender turn"),
                            ev_completed("resp-2"),
                        ]),
                        "text/event-stream",
                    ),
            ],
        })
        .up_to_n_times(2)
        .mount(&server)
        .await;

    let shared_home = Arc::new(TempDir::new()?);
    let mut first_builder = pooled_accounts_builder()
        .with_home(Arc::clone(&shared_home))
        .with_config(|config| {
            if let Some(accounts) = config.accounts.as_mut() {
                accounts.lease_ttl_secs = Some(4);
                accounts.heartbeat_interval_secs = Some(1);
            }
        });
    let first = first_builder.build(&server).await?;
    seed_account(&first, PRIMARY_ACCOUNT_ID).await?;

    let mut second_builder = pooled_accounts_builder()
        .with_home(shared_home)
        .with_config(|config| {
            if let Some(accounts) = config.accounts.as_mut() {
                accounts.lease_ttl_secs = Some(4);
                accounts.heartbeat_interval_secs = Some(1);
            }
        });
    let second = second_builder.build(&server).await?;

    first
        .codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "long-running turn".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await?;

    let first_turn_id = wait_for_event_match(&first.codex, |event| match event {
        EventMsg::TurnStarted(event) => Some(event.turn_id.clone()),
        _ => None,
    })
    .await;

    tokio::time::sleep(Duration::from_secs(5)).await;

    let contender_turn_error = submit_turn_and_wait(&second, "contender turn").await?;
    let contender_turn_error = contender_turn_error
        .expect("contender runtime should fail-closed while active lease heartbeat is healthy");
    assert!(
        contender_turn_error
            .message
            .to_ascii_lowercase()
            .contains("pooled account"),
        "unexpected fail-closed error: {}",
        contender_turn_error.message
    );

    wait_for_event_with_timeout(
        &first.codex,
        |event| match event {
            EventMsg::TurnComplete(event) => event.turn_id == first_turn_id,
            _ => false,
        },
        Duration::from_secs(30),
    )
    .await;

    let requests = server.received_requests().await.unwrap_or_default();
    let responses_requests = requests
        .iter()
        .filter(|request| {
            request.method == Method::POST && request.url.path().ends_with("/responses")
        })
        .count();
    assert_eq!(
        responses_requests, 1,
        "contender runtime should not issue /responses while lease remains active"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn long_running_manual_remote_compact_heartbeat_keeps_lease_exclusive() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let response_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("m1", "before compact"),
            ev_completed("resp-1"),
        ]),
    )
    .await;
    let compact_mock = core_test_support::responses::mount_compact_response_once(
        &server,
        ResponseTemplate::new(200)
            .insert_header("content-type", "application/json")
            .set_delay(Duration::from_secs(8))
            .set_body_json(json!({
                "output": [{
                    "type": "compaction",
                    "encrypted_content": "REMOTE_COMPACT_SUMMARY"
                }]
            })),
    )
    .await;

    let shared_home = Arc::new(TempDir::new()?);
    let mut first_builder = pooled_accounts_builder()
        .with_home(Arc::clone(&shared_home))
        .with_config(|config| {
            if let Some(accounts) = config.accounts.as_mut() {
                accounts.lease_ttl_secs = Some(4);
                accounts.heartbeat_interval_secs = Some(1);
            }
        });
    let first = first_builder.build(&server).await?;
    seed_account(&first, PRIMARY_ACCOUNT_ID).await?;

    let mut second_builder = pooled_accounts_builder()
        .with_home(shared_home)
        .with_config(|config| {
            if let Some(accounts) = config.accounts.as_mut() {
                accounts.lease_ttl_secs = Some(4);
                accounts.heartbeat_interval_secs = Some(1);
            }
        });
    let second = second_builder.build(&server).await?;

    let first_turn_error = submit_turn_and_wait(&first, "before manual compact").await?;
    assert!(first_turn_error.is_none());

    first.codex.submit(Op::Compact).await?;
    wait_for_compact_request(&compact_mock).await;

    tokio::time::sleep(Duration::from_secs(5)).await;

    let contender_turn_error = submit_turn_and_wait(&second, "contender turn").await?;
    let contender_turn_error = contender_turn_error
        .expect("contender runtime should fail-closed while manual compact heartbeat is healthy");
    assert!(
        contender_turn_error
            .message
            .to_ascii_lowercase()
            .contains("pooled account"),
        "unexpected fail-closed error: {}",
        contender_turn_error.message
    );

    wait_for_event(&first.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(
        response_mock.requests().len(),
        1,
        "contender runtime should not issue /responses while compact lease remains active"
    );

    Ok(())
}

fn pooled_accounts_builder() -> core_test_support::test_codex::TestCodexBuilder {
    test_codex()
        .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
        .with_config(|config| {
            config.accounts = Some(accounts_config());
        })
}

fn accounts_config() -> AccountsConfigToml {
    let mut pools = HashMap::new();
    pools.insert(
        LEGACY_DEFAULT_POOL_ID.to_string(),
        AccountPoolDefinitionToml {
            allow_context_reuse: Some(false),
            account_kinds: None,
        },
    );
    AccountsConfigToml {
        backend: None,
        default_pool: Some(LEGACY_DEFAULT_POOL_ID.to_string()),
        proactive_switch_threshold_percent: Some(85),
        lease_ttl_secs: None,
        heartbeat_interval_secs: None,
        min_switch_interval_secs: None,
        allocation_mode: None,
        pools: Some(pools),
    }
}

fn accounts_config_without_default_pool() -> AccountsConfigToml {
    let mut pools = HashMap::new();
    pools.insert(
        LEGACY_DEFAULT_POOL_ID.to_string(),
        AccountPoolDefinitionToml {
            allow_context_reuse: Some(false),
            account_kinds: None,
        },
    );
    AccountsConfigToml {
        backend: None,
        default_pool: None,
        proactive_switch_threshold_percent: Some(85),
        lease_ttl_secs: None,
        heartbeat_interval_secs: None,
        min_switch_interval_secs: None,
        allocation_mode: None,
        pools: Some(pools),
    }
}

fn non_openai_model_provider(server: &wiremock::MockServer) -> ModelProviderInfo {
    let mut provider = built_in_model_providers(/* openai_base_url */ None)["openai"].clone();
    provider.name = "OpenAI (test)".into();
    provider.base_url = Some(format!("{}/v1", server.uri()));
    provider.supports_websockets = false;
    provider
}

async fn seed_two_accounts(test: &TestCodex) -> Result<()> {
    seed_account(test, PRIMARY_ACCOUNT_ID).await?;
    seed_account(test, SECONDARY_ACCOUNT_ID).await?;
    Ok(())
}

async fn seed_account(test: &TestCodex, account_id: &str) -> Result<()> {
    let Some(state_db) = test.codex.state_db() else {
        return Err(anyhow::anyhow!(
            "state db should be available in core integration tests"
        ));
    };
    state_db
        .import_legacy_default_account(LegacyAccountImport {
            account_id: account_id.to_string(),
        })
        .await?;
    write_pooled_auth(
        test.codex_home_path(),
        account_id,
        account_id,
        &format!("pooled-access-{account_id}"),
    )?;
    Ok(())
}

async fn wait_for_compact_request(mock_response: &ResponseMock) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if !mock_response.requests().is_empty() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for compact request"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn assert_pre_turn_remote_compact_failure_rotates_next_turn(
    expected_health_state: AccountHealthState,
    compact_failure_response: ResponseTemplate,
) -> Result<()> {
    struct CompactSeqResponder {
        next_call: std::sync::atomic::AtomicUsize,
        responses: Vec<ResponseTemplate>,
    }

    impl Respond for CompactSeqResponder {
        fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
            let call_index = self
                .next_call
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.responses
                .get(call_index)
                .unwrap_or_else(|| panic!("missing compact response for call {call_index}"))
                .clone()
        }
    }

    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_assistant_message("m1", "before compact"),
                ev_completed_with_tokens("resp-1", /*total_tokens*/ 500),
            ]),
            sse(vec![
                ev_response_created("resp-2"),
                ev_assistant_message("m2", "after compact"),
                ev_completed("resp-2"),
            ]),
        ],
    )
    .await;

    let compact_success_response = ResponseTemplate::new(200)
        .insert_header("content-type", "application/json")
        .set_body_json(json!({
            "output": [{
                "type": "compaction",
                "encrypted_content": "REMOTE_COMPACT_SUMMARY"
            }]
        }));
    let compact_responses = vec![compact_failure_response, compact_success_response];
    let compact_call_count = compact_responses.len() as u64;
    Mock::given(method("POST"))
        .and(path_regex(".*/responses/compact$"))
        .respond_with(CompactSeqResponder {
            next_call: std::sync::atomic::AtomicUsize::new(0),
            responses: compact_responses,
        })
        .up_to_n_times(compact_call_count)
        .expect(compact_call_count)
        .mount(&server)
        .await;

    let mut builder = pooled_accounts_builder().with_config(|config| {
        config.model_auto_compact_token_limit = Some(120);
    });
    let test = builder.build(&server).await?;
    seed_two_accounts(&test).await?;

    let first_turn_error = submit_turn_and_wait(&test, "seed usage for compact").await?;
    assert!(first_turn_error.is_none());

    let second_turn_error = submit_turn_and_wait(&test, "turn with failing compact").await?;
    assert!(
        second_turn_error.is_some(),
        "pre-turn compact failure should fail the current turn"
    );
    wait_for_account_health_transition(
        &test,
        expected_health_state,
        AccountLeaseRuntimeReason::NonReplayableTurn,
    )
    .await?;

    let third_turn_error = submit_turn_and_wait(&test, "turn after compact failure").await?;
    assert!(third_turn_error.is_none());

    let response_requests = response_mock.requests();
    assert_eq!(
        response_requests.len(),
        2,
        "pre-turn compact failure should not auto-replay the failed turn"
    );
    assert_account_ids_in_order(
        &response_requests,
        &[PRIMARY_ACCOUNT_ID, SECONDARY_ACCOUNT_ID],
    );

    let all_requests = server.received_requests().await.unwrap_or_default();
    let compact_request_account_ids = all_requests
        .iter()
        .filter(|request| {
            request.method == Method::POST && request.url.path().ends_with("/responses/compact")
        })
        .map(|request| {
            request
                .headers
                .get("chatgpt-account-id")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        compact_request_account_ids,
        vec![
            Some(PRIMARY_ACCOUNT_ID.to_string()),
            Some(SECONDARY_ACCOUNT_ID.to_string()),
        ],
        "expected compact failures to mark the active account unavailable before next turn"
    );
    let compact_request_session_ids = all_requests
        .iter()
        .filter(|request| {
            request.method == Method::POST && request.url.path().ends_with("/responses/compact")
        })
        .map(|request| {
            request
                .headers
                .get("session_id")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    assert!(
        compact_request_session_ids
            .iter()
            .all(std::option::Option::is_some),
        "expected compact requests to include a session_id header"
    );
    assert_ne!(
        compact_request_session_ids[0], compact_request_session_ids[1],
        "rotation with allow_context_reuse=false should reset remote session identity before pre-turn compact"
    );

    Ok(())
}

fn sse_with_primary_usage_percent(response_id: &str, used_percent: f64) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .insert_header("x-codex-primary-used-percent", used_percent.to_string())
        .insert_header("x-codex-primary-window-minutes", "60")
        .set_body_raw(
            sse(vec![
                ev_response_created(response_id),
                ev_completed(response_id),
            ]),
            "text/event-stream",
        )
}

fn assert_account_ids_in_order(requests: &[ResponsesRequest], expected: &[&str]) {
    assert_eq!(
        requests.len(),
        expected.len(),
        "request count mismatch for account assertions"
    );
    let actual = requests
        .iter()
        .map(|request| request.header("chatgpt-account-id"))
        .collect::<Vec<_>>();
    let expected = expected
        .iter()
        .map(|account_id| Some((*account_id).to_string()))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

async fn submit_turn_and_wait(
    test: &TestCodex,
    text: &str,
) -> Result<Option<codex_protocol::protocol::ErrorEvent>> {
    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: text.to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await?;

    let turn_id = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::TurnStarted(event) => Some(event.turn_id.clone()),
        _ => None,
    })
    .await;

    let mut saw_error = None;
    loop {
        let event = wait_for_event(&test.codex, |_| true).await;
        match event {
            EventMsg::Error(error_event) => {
                saw_error = Some(error_event);
            }
            EventMsg::TurnComplete(event) if event.turn_id == turn_id => {
                return Ok(saw_error);
            }
            _ => {}
        }
    }
}

async fn wait_for_account_health_transition(
    test: &TestCodex,
    expected_health_state: AccountHealthState,
    expected_switch_reason: AccountLeaseRuntimeReason,
) -> Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let Some(snapshot) = test.codex.account_lease_snapshot().await else {
            return Err(anyhow::anyhow!(
                "pooled session should expose lease snapshot"
            ));
        };
        if snapshot.health_state == Some(expected_health_state)
            && snapshot.switch_reason == Some(expected_switch_reason)
        {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(anyhow::anyhow!(
                "timed out waiting for account health transition: {snapshot:?}"
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn write_shared_auth(codex_home: &Path, account_id: &str, access_token: &str) -> Result<()> {
    write_chatgpt_auth(codex_home, account_id, access_token)
}

fn write_pooled_auth(
    codex_home: &Path,
    backend_account_handle: &str,
    account_id: &str,
    access_token: &str,
) -> Result<()> {
    let auth_home = codex_home
        .join(".pooled-auth/backends/local/accounts")
        .join(backend_account_handle);
    write_chatgpt_auth(auth_home.as_path(), account_id, access_token)
}

fn write_chatgpt_auth(auth_home: &Path, account_id: &str, access_token: &str) -> Result<()> {
    save_auth(
        auth_home,
        &AuthDotJson {
            auth_mode: None,
            openai_api_key: None,
            tokens: Some(TokenData {
                id_token: parse_chatgpt_jwt_claims(&fake_access_token(account_id))?,
                access_token: access_token.to_string(),
                refresh_token: format!("refresh-{account_id}"),
                account_id: Some(account_id.to_string()),
            }),
            last_refresh: Some(Utc::now()),
        },
        AuthCredentialsStoreMode::File,
    )?;
    Ok(())
}

fn fake_access_token(chatgpt_account_id: &str) -> String {
    #[derive(Serialize)]
    struct Header {
        alg: &'static str,
        typ: &'static str,
    }

    let header = Header {
        alg: "none",
        typ: "JWT",
    };
    let payload = json!({
        "email": "user@example.com",
        "email_verified": true,
        "https://api.openai.com/auth": {
            "chatgpt_plan_type": "pro",
            "chatgpt_user_id": "user-12345",
            "chatgpt_account_id": chatgpt_account_id,
        },
    });
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let header_b64 =
        b64(&serde_json::to_vec(&header).unwrap_or_else(|err| panic!("serialize header: {err}")));
    let payload_b64 =
        b64(&serde_json::to_vec(&payload).unwrap_or_else(|err| panic!("serialize payload: {err}")));
    let signature_b64 = b64(b"sig");
    format!("{header_b64}.{payload_b64}.{signature_b64}")
}
