use anyhow::Result;
use base64::Engine;
use chrono::Utc;
use codex_config::types::AccountPoolDefinitionToml;
use codex_config::types::AccountsConfigToml;
use codex_login::AuthCredentialsStoreMode;
use codex_login::CodexAuth;
use codex_login::TokenData;
use codex_login::auth::AuthDotJson;
use codex_login::save_auth;
use codex_login::token_data::parse_chatgpt_jwt_claims;
use codex_model_provider_info::WireApi;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::user_input::UserInput;
use codex_state::LegacyAccountImport;
use core_test_support::responses;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use tokio::time::Duration;
use tokio::time::timeout;
use wiremock::Mock;
use wiremock::ResponseTemplate;
use wiremock::http::Method;
use wiremock::matchers::method;
use wiremock::matchers::path_regex;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_fallback_switches_to_http_on_upgrade_required_connect() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    Mock::given(method("GET"))
        .and(path_regex(".*/responses$"))
        .respond_with(ResponseTemplate::new(426))
        .mount(&server)
        .await;

    let response_mock = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;

    let mut builder = test_codex().with_config({
        let base_url = format!("{}/v1", server.uri());
        move |config| {
            config.model_provider.base_url = Some(base_url);
            config.model_provider.wire_api = WireApi::Responses;
            config.model_provider.supports_websockets = true;
            // If we don't treat 426 specially, the sampling loop would retry the WebSocket
            // handshake before switching to the HTTP transport.
            config.model_provider.stream_max_retries = Some(2);
            config.model_provider.request_max_retries = Some(0);
        }
    });
    let test = builder.build(&server).await?;

    test.submit_turn("hello").await?;

    let requests = server.received_requests().await.unwrap_or_default();
    let websocket_attempts = requests
        .iter()
        .filter(|req| req.method == Method::GET && req.url.path().ends_with("/responses"))
        .count();
    let http_attempts = requests
        .iter()
        .filter(|req| req.method == Method::POST && req.url.path().ends_with("/responses"))
        .count();

    // The startup prewarm request sees 426 and immediately switches the session to HTTP fallback,
    // so the first turn goes straight to HTTP with no additional websocket connect attempt.
    assert_eq!(websocket_attempts, 1);
    assert_eq!(http_attempts, 1);
    assert_eq!(response_mock.requests().len(), 1);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_fallback_in_pooled_mode_uses_leased_account_for_first_websocket_attempt()
-> Result<()> {
    skip_if_no_network!(Ok(()));

    let pooled_account_id = "account_id_b";
    let server = responses::start_mock_server().await;
    Mock::given(method("GET"))
        .and(path_regex(".*/responses$"))
        .respond_with(ResponseTemplate::new(426))
        .mount(&server)
        .await;

    let response_mock = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;

    let mut builder = test_codex()
        .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
        .with_config({
            let base_url = format!("{}/v1", server.uri());
            move |config| {
                config.model_provider.base_url = Some(base_url);
                config.model_provider.wire_api = WireApi::Responses;
                config.model_provider.supports_websockets = true;
                config.model_provider.stream_max_retries = Some(2);
                config.model_provider.request_max_retries = Some(0);
                let mut pools = HashMap::new();
                pools.insert(
                    "legacy-default".to_string(),
                    AccountPoolDefinitionToml {
                        allow_context_reuse: Some(true),
                        account_kinds: None,
                    },
                );
                config.accounts = Some(AccountsConfigToml {
                    backend: None,
                    default_pool: Some("legacy-default".to_string()),
                    proactive_switch_threshold_percent: Some(85),
                    lease_ttl_secs: None,
                    heartbeat_interval_secs: None,
                    min_switch_interval_secs: None,
                    allocation_mode: None,
                    pools: Some(pools),
                });
            }
        });
    let test = builder.build(&server).await?;

    let Some(state_db) = test.codex.state_db() else {
        return Err(anyhow::anyhow!(
            "state db should be available in core integration tests"
        ));
    };
    state_db
        .import_legacy_default_account(LegacyAccountImport {
            account_id: pooled_account_id.to_string(),
        })
        .await?;
    write_pooled_auth(
        test.codex_home_path(),
        pooled_account_id,
        pooled_account_id,
        &format!("pooled-access-{pooled_account_id}"),
    )?;

    test.submit_turn("hello").await?;

    let requests = server.received_requests().await.unwrap_or_default();
    let websocket_attempts = requests
        .iter()
        .filter(|req| req.method == Method::GET && req.url.path().ends_with("/responses"))
        .collect::<Vec<_>>();
    let http_attempts = requests
        .iter()
        .filter(|req| req.method == Method::POST && req.url.path().ends_with("/responses"))
        .count();
    let websocket_attempt_account = websocket_attempts
        .first()
        .and_then(|request| request.headers.get("chatgpt-account-id"))
        .and_then(|value| value.to_str().ok());

    assert_eq!(websocket_attempts.len(), 1);
    assert_eq!(websocket_attempt_account, Some(pooled_account_id));
    assert_eq!(http_attempts, 1);
    assert_eq!(response_mock.requests().len(), 1);

    Ok(())
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
    save_auth(
        auth_home.as_path(),
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
    let payload = serde_json::json!({
        "email": "user@example.com",
        "email_verified": true,
        "https://api.openai.com/auth": {
            "chatgpt_plan_type": "pro",
            "chatgpt_user_id": "user-12345",
            "chatgpt_account_id": chatgpt_account_id,
        },
    });
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let header_b64 = b64(&serde_json::to_vec(&header).unwrap_or_else(|err| {
        panic!("serialize header: {err}");
    }));
    let payload_b64 = b64(&serde_json::to_vec(&payload).unwrap_or_else(|err| {
        panic!("serialize payload: {err}");
    }));
    let signature_b64 = b64(b"sig");
    format!("{header_b64}.{payload_b64}.{signature_b64}")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_fallback_switches_to_http_after_retries_exhausted() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;

    let mut builder = test_codex().with_config({
        let base_url = format!("{}/v1", server.uri());
        move |config| {
            config.model_provider.base_url = Some(base_url);
            config.model_provider.wire_api = WireApi::Responses;
            config.model_provider.supports_websockets = true;
            config.model_provider.stream_max_retries = Some(2);
            config.model_provider.request_max_retries = Some(0);
        }
    });
    let test = builder.build(&server).await?;

    test.submit_turn("hello").await?;

    let requests = server.received_requests().await.unwrap_or_default();
    let websocket_attempts = requests
        .iter()
        .filter(|req| req.method == Method::GET && req.url.path().ends_with("/responses"))
        .count();
    let http_attempts = requests
        .iter()
        .filter(|req| req.method == Method::POST && req.url.path().ends_with("/responses"))
        .count();

    // Deferred request prewarm is attempted at startup.
    // The first turn then makes 3 websocket stream attempts (initial try + 2 retries),
    // after which fallback activates and the request is replayed over HTTP.
    assert_eq!(websocket_attempts, 4);
    assert_eq!(http_attempts, 1);
    assert_eq!(response_mock.requests().len(), 1);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_fallback_hides_first_websocket_retry_stream_error() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;

    let mut builder = test_codex().with_config({
        let base_url = format!("{}/v1", server.uri());
        move |config| {
            config.model_provider.base_url = Some(base_url);
            config.model_provider.wire_api = WireApi::Responses;
            config.model_provider.supports_websockets = true;
            config.model_provider.stream_max_retries = Some(2);
            config.model_provider.request_max_retries = Some(0);
        }
    });
    let TestCodex {
        codex,
        session_configured,
        cwd,
        ..
    } = builder.build(&server).await?;

    codex
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: "hello".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: cwd.path().to_path_buf(),
            approval_policy: AskForApproval::Never,
            approvals_reviewer: None,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            model: session_configured.model.clone(),
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode: None,
            personality: None,
        })
        .await?;

    let mut stream_error_messages = Vec::new();
    loop {
        let event = timeout(Duration::from_secs(10), codex.next_event())
            .await
            .expect("timeout waiting for event")
            .expect("event stream ended unexpectedly")
            .msg;
        match event {
            EventMsg::StreamError(e) => stream_error_messages.push(e.message),
            EventMsg::TurnComplete(_) => break,
            _ => {}
        }
    }

    let expected_stream_errors = if cfg!(debug_assertions) {
        vec!["Reconnecting... 1/2", "Reconnecting... 2/2"]
    } else {
        vec!["Reconnecting... 2/2"]
    };
    assert_eq!(stream_error_messages, expected_stream_errors);
    assert_eq!(response_mock.requests().len(), 1);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_fallback_is_sticky_across_turns() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
            sse(vec![ev_response_created("resp-2"), ev_completed("resp-2")]),
        ],
    )
    .await;

    let mut builder = test_codex().with_config({
        let base_url = format!("{}/v1", server.uri());
        move |config| {
            config.model_provider.base_url = Some(base_url);
            config.model_provider.wire_api = WireApi::Responses;
            config.model_provider.supports_websockets = true;
            config.model_provider.stream_max_retries = Some(2);
            config.model_provider.request_max_retries = Some(0);
        }
    });
    let test = builder.build(&server).await?;

    test.submit_turn("first").await?;
    test.submit_turn("second").await?;

    let requests = server.received_requests().await.unwrap_or_default();
    let websocket_attempts = requests
        .iter()
        .filter(|req| req.method == Method::GET && req.url.path().ends_with("/responses"))
        .count();
    let http_attempts = requests
        .iter()
        .filter(|req| req.method == Method::POST && req.url.path().ends_with("/responses"))
        .count();

    // WebSocket attempts all happen on the first turn:
    // 1 deferred request prewarm attempt (startup) + 3 stream attempts
    // (initial try + 2 retries) before fallback.
    // Fallback is sticky, so the second turn stays on HTTP and adds no websocket attempts.
    assert_eq!(websocket_attempts, 4);
    assert_eq!(http_attempts, 2);
    assert_eq!(response_mock.requests().len(), 2);

    Ok(())
}
