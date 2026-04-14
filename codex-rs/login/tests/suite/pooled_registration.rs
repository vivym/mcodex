#![allow(clippy::expect_used)]

use base64::Engine;
use codex_login::AuthCredentialsStoreMode;
use codex_login::CLIENT_ID;
use codex_login::ServerOptions;
use codex_login::pooled_registration::run_pooled_browser_registration;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::net::TcpListener;
use std::time::Duration;
use tempfile::tempdir;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

fn make_jwt(payload: serde_json::Value) -> String {
    let header = json!({ "alg": "none", "typ": "JWT" });
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let header_b64 = b64(&serde_json::to_vec(&header).expect("serialize header"));
    let payload_b64 = b64(&serde_json::to_vec(&payload).expect("serialize payload"));
    let signature_b64 = b64(b"sig");
    format!("{header_b64}.{payload_b64}.{signature_b64}")
}

fn test_server_options(codex_home: &std::path::Path, issuer: String, port: u16) -> ServerOptions {
    let mut opts = ServerOptions::new(
        codex_home.to_path_buf(),
        CLIENT_ID.to_string(),
        None,
        AuthCredentialsStoreMode::File,
    );
    opts.issuer = issuer;
    opts.port = port;
    opts.open_browser = false;
    opts.force_state = Some("test-state".to_string());
    opts
}

fn reserve_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("bind port")
        .local_addr()
        .expect("local addr")
        .port()
}

async fn send_callback_when_ready(port: u16) -> reqwest::Response {
    let client = reqwest::Client::new();
    let callback = format!("http://127.0.0.1:{port}/auth/callback?code=abc&state=test-state");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);

    loop {
        match client.get(&callback).send().await {
            Ok(response) => return response,
            Err(err) if err.is_connect() && tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(err) => panic!("send callback request: {err:?}"),
        }
    }
}

pub(crate) async fn pooled_browser_registration_returns_tokens_without_writing_shared_auth() {
    let codex_home = tempdir().expect("create tempdir");
    let mock_server = MockServer::start().await;
    let jwt = make_jwt(json!({
        "https://api.openai.com/auth": {
            "chatgpt_account_id": "acct-1"
        }
    }));

    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id_token": jwt,
            "access_token": "access-token-1",
            "refresh_token": "refresh-token-1"
        })))
        .mount(&mock_server)
        .await;

    let port = reserve_port();
    let opts = test_server_options(codex_home.path(), mock_server.uri(), port);
    let join = tokio::spawn(async move { run_pooled_browser_registration(opts).await });

    let response = send_callback_when_ready(port).await;
    assert!(response.status().is_success());

    let tokens = join
        .await
        .expect("task should complete")
        .expect("pooled browser registration should succeed");

    assert_eq!(tokens.account_id, "acct-1");
    assert!(!codex_home.path().join("auth.json").exists());
}

pub(crate) async fn pooled_browser_registration_failure_completes_without_hanging() {
    let codex_home = tempdir().expect("create tempdir");
    let mock_server = MockServer::start().await;
    let jwt = make_jwt(json!({
        "https://api.openai.com/auth": {}
    }));

    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id_token": jwt,
            "access_token": "access-token-1",
            "refresh_token": "refresh-token-1"
        })))
        .mount(&mock_server)
        .await;

    let port = reserve_port();
    let opts = test_server_options(codex_home.path(), mock_server.uri(), port);
    let join = tokio::spawn(async move { run_pooled_browser_registration(opts).await });

    let response = send_callback_when_ready(port).await;
    assert!(response.status().is_success());

    let err = tokio::time::timeout(Duration::from_secs(2), join)
        .await
        .expect("registration flow should return instead of hanging")
        .expect("task should complete")
        .expect_err("pooled browser registration should fail");
    assert!(
        err.to_string()
            .contains("registration tokens are missing chatgpt_account_id"),
        "unexpected error: {err}"
    );
    assert!(!codex_home.path().join("auth.json").exists());
}
