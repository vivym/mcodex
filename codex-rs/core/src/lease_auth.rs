use std::fmt;
use std::sync::Arc;
use std::sync::RwLock;

use async_trait::async_trait;
use codex_login::AuthManager;
use codex_login::AuthProvider;
use codex_login::AuthRecovery;
use codex_login::AuthRecoveryStepResult;
use codex_login::CodexAuth;
use codex_login::RefreshTokenError;
use codex_login::RefreshingAuthProvider;
use codex_login::SharedAuthProvider;
use codex_login::auth::LeaseScopedAuthSession;

#[derive(Default)]
pub struct SessionLeaseAuth {
    current: RwLock<Option<Arc<dyn LeaseScopedAuthSession>>>,
}

impl fmt::Debug for SessionLeaseAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionLeaseAuth")
            .field("has_current", &self.current_session().is_some())
            .finish()
    }
}

impl SessionLeaseAuth {
    pub(crate) fn replace_current(&self, current: Option<Arc<dyn LeaseScopedAuthSession>>) {
        let mut slot = self
            .current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = current;
    }

    pub(crate) fn clear(&self) {
        self.replace_current(None);
    }

    pub(crate) fn current_session(&self) -> Option<Arc<dyn LeaseScopedAuthSession>> {
        self.current
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn bridge(
        self: &Arc<Self>,
        auth_manager: Arc<AuthManager>,
    ) -> SessionLeaseAuthBridge {
        SessionLeaseAuthBridge {
            lease_auth_session: self.current_session(),
            shared_auth_provider: Arc::new(SharedAuthProvider::new(auth_manager)),
        }
    }

    pub(crate) fn provider(
        self: &Arc<Self>,
        auth_manager: Arc<AuthManager>,
    ) -> SessionLeaseAuthProvider {
        SessionLeaseAuthProvider {
            holder: Arc::clone(self),
            shared_auth_provider: Arc::new(SharedAuthProvider::new(auth_manager)),
        }
    }
}

pub(crate) struct SessionLeaseAuthBridge {
    lease_auth_session: Option<Arc<dyn LeaseScopedAuthSession>>,
    shared_auth_provider: Arc<SharedAuthProvider>,
}

pub(crate) struct SessionLeaseAuthProvider {
    holder: Arc<SessionLeaseAuth>,
    shared_auth_provider: Arc<SharedAuthProvider>,
}

impl SessionLeaseAuthBridge {
    async fn auth_from_lease(&self) -> Option<CodexAuth> {
        if let Some(session) = self.lease_auth_session.as_ref() {
            return session
                .refresh_leased_turn_auth()
                .ok()
                .map(|auth| auth.auth().clone());
        }

        None
    }
}

pub(crate) struct LeaseSessionAuthRecovery {
    lease_auth_session: Arc<dyn LeaseScopedAuthSession>,
    attempted: bool,
}

impl LeaseSessionAuthRecovery {
    pub(crate) fn new(lease_auth_session: Arc<dyn LeaseScopedAuthSession>) -> Self {
        Self {
            lease_auth_session,
            attempted: false,
        }
    }
}

#[async_trait]
impl AuthProvider for SessionLeaseAuthBridge {
    async fn auth(&self) -> Option<CodexAuth> {
        if self.lease_auth_session.is_some() {
            return self.auth_from_lease().await;
        }
        self.shared_auth_provider.auth().await
    }
}

impl RefreshingAuthProvider for SessionLeaseAuthBridge {
    fn unauthorized_recovery(&self) -> Option<Box<dyn AuthRecovery>> {
        self.lease_auth_session.as_ref().map_or_else(
            || self.shared_auth_provider.unauthorized_recovery(),
            |session| Some(Box::new(LeaseSessionAuthRecovery::new(Arc::clone(session)))),
        )
    }
}

#[async_trait]
impl AuthProvider for SessionLeaseAuthProvider {
    async fn auth(&self) -> Option<CodexAuth> {
        SessionLeaseAuthBridge {
            lease_auth_session: self.holder.current_session(),
            shared_auth_provider: Arc::clone(&self.shared_auth_provider),
        }
        .auth()
        .await
    }
}

impl RefreshingAuthProvider for SessionLeaseAuthProvider {
    fn unauthorized_recovery(&self) -> Option<Box<dyn AuthRecovery>> {
        SessionLeaseAuthBridge {
            lease_auth_session: self.holder.current_session(),
            shared_auth_provider: Arc::clone(&self.shared_auth_provider),
        }
        .unauthorized_recovery()
    }
}

#[async_trait]
impl AuthRecovery for LeaseSessionAuthRecovery {
    fn has_next(&self) -> bool {
        !self.attempted
    }

    fn unavailable_reason(&self) -> &'static str {
        if self.attempted {
            "lease_recovery_exhausted"
        } else {
            "recovery_not_started"
        }
    }

    fn mode_name(&self) -> &'static str {
        "lease_scoped"
    }

    fn step_name(&self) -> &'static str {
        "refresh_leased_turn_auth"
    }

    async fn next(&mut self) -> Result<AuthRecoveryStepResult, RefreshTokenError> {
        self.attempted = true;
        self.lease_auth_session
            .refresh_leased_turn_auth()
            .map(|_| AuthRecoveryStepResult::new(Some(true)))
            .map_err(|err| RefreshTokenError::Transient(std::io::Error::other(err.to_string())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::bail;
    use base64::Engine;
    use codex_login::LeasedTurnAuth;
    use codex_login::auth::LeaseAuthBinding;
    use serde::Serialize;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;

    struct TestLeaseScopedAuthSession {
        binding: LeaseAuthBinding,
        current: AtomicBool,
    }

    impl TestLeaseScopedAuthSession {
        fn new(account_id: &str) -> Self {
            Self {
                binding: LeaseAuthBinding {
                    account_id: account_id.to_string(),
                    backend_account_handle: format!("handle-{account_id}"),
                    lease_epoch: 1,
                },
                current: AtomicBool::new(true),
            }
        }

        fn invalidate(&self) {
            self.current.store(false, Ordering::SeqCst);
        }
    }

    impl LeaseScopedAuthSession for TestLeaseScopedAuthSession {
        fn leased_turn_auth(&self) -> anyhow::Result<LeasedTurnAuth> {
            self.refresh_leased_turn_auth()
        }

        fn refresh_leased_turn_auth(&self) -> anyhow::Result<LeasedTurnAuth> {
            if !self.current.load(Ordering::SeqCst) {
                bail!("stale lease session");
            }
            Ok(LeasedTurnAuth::chatgpt(
                self.binding.account_id.clone(),
                fake_access_token(&self.binding.account_id),
            ))
        }

        fn binding(&self) -> &LeaseAuthBinding {
            &self.binding
        }

        fn ensure_current(&self) -> anyhow::Result<()> {
            if !self.current.load(Ordering::SeqCst) {
                bail!("stale lease session");
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn bridge_instance_fails_closed_after_rotation_until_rebound() {
        let shared_auth_manager =
            AuthManager::from_auth_for_testing(CodexAuth::create_dummy_chatgpt_auth_for_testing());
        let holder = Arc::new(SessionLeaseAuth::default());
        let first_session = Arc::new(TestLeaseScopedAuthSession::new("acct-1"));
        holder.replace_current(Some(first_session.clone()));

        let old_bridge = holder.bridge(Arc::clone(&shared_auth_manager));
        assert_eq!(
            old_bridge
                .auth()
                .await
                .and_then(|auth| auth.get_account_id()),
            Some("acct-1".to_string())
        );

        first_session.invalidate();
        let second_session = Arc::new(TestLeaseScopedAuthSession::new("acct-2"));
        holder.replace_current(Some(second_session));

        assert_eq!(old_bridge.auth().await, None);

        let rebound_bridge = holder.bridge(shared_auth_manager);
        assert_eq!(
            rebound_bridge
                .auth()
                .await
                .and_then(|auth| auth.get_account_id()),
            Some("acct-2".to_string())
        );
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
        let header_b64 =
            b64(&serde_json::to_vec(&header)
                .unwrap_or_else(|err| panic!("serialize header: {err}")));
        let payload_b64 =
            b64(&serde_json::to_vec(&payload)
                .unwrap_or_else(|err| panic!("serialize payload: {err}")));
        let signature_b64 = b64(b"sig");
        format!("{header_b64}.{payload_b64}.{signature_b64}")
    }
}
