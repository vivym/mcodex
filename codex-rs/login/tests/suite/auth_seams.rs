use base64::Engine;
use codex_login::AuthCredentialsStoreMode;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_login::LeasedTurnAuth;
use codex_login::LegacyAuthView;
use codex_login::login_with_api_key;
use pretty_assertions::assert_eq;
use serde::Serialize;
use tempfile::tempdir;

pub(crate) async fn legacy_auth_view_reads_auth_manager_snapshot() {
    let manager =
        AuthManager::from_auth_for_testing(CodexAuth::create_dummy_chatgpt_auth_for_testing());

    let legacy = LegacyAuthView::new(&manager);
    let current = legacy.current().await.expect("expected auth snapshot");
    assert_eq!(current.get_account_id(), Some("account_id".to_string()));
}

pub(crate) async fn leased_turn_auth_does_not_read_shared_auth_manager() {
    let codex_home = tempdir().expect("create tempdir");
    let manager = AuthManager::from_auth_for_testing_with_home(
        CodexAuth::create_dummy_chatgpt_auth_for_testing(),
        codex_home.path().to_path_buf(),
    );
    let legacy = LegacyAuthView::new(&manager);
    let leased = LeasedTurnAuth::chatgpt("acct-1", fake_access_token("acct-1"));

    let legacy_before_reload = legacy.current().await.expect("expected auth");
    assert_eq!(
        legacy_before_reload.get_account_id(),
        Some("account_id".to_string())
    );

    login_with_api_key(
        codex_home.path(),
        "sk-shared-new",
        AuthCredentialsStoreMode::File,
    )
    .expect("write shared auth");
    assert!(manager.reload(), "reload should detect auth change");

    let legacy_after_reload = legacy.current().await.expect("expected reloaded auth");
    assert_eq!(legacy_after_reload.api_key(), Some("sk-shared-new"));
    assert_eq!(legacy_after_reload.get_account_id(), None);
    assert_eq!(leased.account_id(), Some("acct-1".to_string()));
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
    let header_b64 = b64(&serde_json::to_vec(&header).expect("serialize header"));
    let payload_b64 = b64(&serde_json::to_vec(&payload).expect("serialize payload"));
    let signature_b64 = b64(b"sig");
    format!("{header_b64}.{payload_b64}.{signature_b64}")
}
