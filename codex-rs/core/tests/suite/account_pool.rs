use anyhow::Result;
use codex_config::types::AccountPoolDefinitionToml;
use codex_config::types::AccountsConfigToml;
use codex_login::CodexAuth;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use codex_state::LegacyAccountImport;
use core_test_support::responses::ResponsesRequest;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_response_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use http::Method;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::HashMap;
use wiremock::ResponseTemplate;

const PRIMARY_ACCOUNT_ID: &str = "account_id";
const SECONDARY_ACCOUNT_ID: &str = "account_id_b";
const LEGACY_DEFAULT_POOL_ID: &str = "legacy-default";

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
    assert_eq!(requests.len(), 2, "expected one request per turn");
    assert_account_ids_in_order(&requests, &[PRIMARY_ACCOUNT_ID, SECONDARY_ACCOUNT_ID]);

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

async fn seed_two_accounts(test: &TestCodex) -> Result<()> {
    let Some(state_db) = test.codex.state_db() else {
        return Err(anyhow::anyhow!(
            "state db should be available in core integration tests"
        ));
    };
    state_db
        .import_legacy_default_account(LegacyAccountImport {
            account_id: PRIMARY_ACCOUNT_ID.to_string(),
        })
        .await?;
    state_db
        .import_legacy_default_account(LegacyAccountImport {
            account_id: SECONDARY_ACCOUNT_ID.to_string(),
        })
        .await?;
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

    let mut saw_error = None;
    loop {
        let event = wait_for_event(&test.codex, |_| true).await;
        match event {
            EventMsg::Error(error_event) => {
                saw_error = Some(error_event);
            }
            EventMsg::TurnComplete(_) => {
                return Ok(saw_error);
            }
            _ => {}
        }
    }
}
