use crate::auth::CodexAuth;
use crate::auth::login_with_chatgpt_auth_tokens;
use codex_config::types::AuthCredentialsStoreMode;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

static LEASE_EPOCH_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Turn-scoped leased auth snapshot detached from shared manager state.
#[derive(Clone, Debug)]
pub struct LeasedTurnAuth {
    auth: CodexAuth,
    lease_epoch: u64,
}

impl LeasedTurnAuth {
    pub fn new(auth: CodexAuth) -> Self {
        Self {
            auth,
            lease_epoch: LEASE_EPOCH_COUNTER.fetch_add(1, Ordering::Relaxed),
        }
    }

    pub fn chatgpt(chatgpt_account_id: impl Into<String>, access_token: impl Into<String>) -> Self {
        let chatgpt_account_id = chatgpt_account_id.into();
        let access_token = access_token.into();
        let codex_home = leased_auth_storage_dir();
        std::fs::create_dir_all(&codex_home)
            .unwrap_or_else(|err| panic!("failed to create leased auth storage: {err}"));
        login_with_chatgpt_auth_tokens(&codex_home, &access_token, &chatgpt_account_id, None)
            .unwrap_or_else(|err| panic!("failed to persist leased ChatGPT auth: {err}"));
        let auth = CodexAuth::from_auth_storage(&codex_home, AuthCredentialsStoreMode::Ephemeral)
            .unwrap_or_else(|err| panic!("failed to load leased ChatGPT auth: {err}"))
            .unwrap_or_else(|| panic!("leased ChatGPT auth was not saved"));
        Self::new(auth)
    }

    pub fn account_id(&self) -> Option<String> {
        self.auth.get_account_id()
    }

    pub fn auth(&self) -> &CodexAuth {
        &self.auth
    }

    pub fn lease_epoch(&self) -> u64 {
        self.lease_epoch
    }
}

fn leased_auth_storage_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("codex-leased-auth-{}-{nonce}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::LeasedTurnAuth;
    use base64::Engine;
    use pretty_assertions::assert_eq;
    use serde::Serialize;

    fn fake_access_token() -> String {
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
                "chatgpt_account_id": "acct-1",
            },
        });
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        let header_b64 = b64(&serde_json::to_vec(&header).expect("serialize header"));
        let payload_b64 = b64(&serde_json::to_vec(&payload).expect("serialize payload"));
        let signature_b64 = b64(b"sig");
        format!("{header_b64}.{payload_b64}.{signature_b64}")
    }

    #[test]
    fn leased_turn_auth_does_not_read_shared_auth_manager() {
        let leased = LeasedTurnAuth::chatgpt("acct-1", fake_access_token());
        assert_eq!(leased.account_id(), Some("acct-1".to_string()));
    }
}
