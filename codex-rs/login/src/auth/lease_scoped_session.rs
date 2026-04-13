use crate::auth::CodexAuth;
use crate::auth::LeasedTurnAuth;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use codex_config::types::AuthCredentialsStoreMode;
use std::path::PathBuf;

/// Binding that ties a session to the account and lease that created it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseAuthBinding {
    pub account_id: String,
    pub backend_account_handle: String,
    pub lease_epoch: u64,
}

/// Lease-scoped auth snapshot provider for request execution and long-lived consumers.
pub trait LeaseScopedAuthSession: Send + Sync {
    fn leased_turn_auth(&self) -> Result<LeasedTurnAuth>;
    fn refresh_leased_turn_auth(&self) -> Result<LeasedTurnAuth>;
    fn binding(&self) -> &LeaseAuthBinding;
    fn ensure_current(&self) -> Result<()>;
}

/// Local lease-scoped auth session backed by a backend-private auth namespace.
#[derive(Clone, Debug)]
pub struct LocalLeaseScopedAuthSession {
    binding: LeaseAuthBinding,
    auth_home: PathBuf,
}

impl LocalLeaseScopedAuthSession {
    pub fn new(binding: LeaseAuthBinding, auth_home: PathBuf) -> Self {
        Self { binding, auth_home }
    }

    fn load_bound_auth(&self) -> Result<CodexAuth> {
        let auth = match CodexAuth::from_auth_storage(
            self.auth_home.as_path(),
            AuthCredentialsStoreMode::File,
        )? {
            Some(auth) => auth,
            None => {
                bail!(
                    "missing pooled auth for backend account {}",
                    self.binding.backend_account_handle
                );
            }
        };

        let account_id = auth
            .get_account_id()
            .ok_or_else(|| anyhow!("pooled auth is missing an account id"))?;
        if account_id != self.binding.account_id {
            bail!(
                "pooled auth rebinding detected for backend account {}",
                self.binding.backend_account_handle
            );
        }

        Ok(auth)
    }

    fn leased_turn_auth_from_current(&self) -> Result<LeasedTurnAuth> {
        Ok(LeasedTurnAuth::from_codex_auth(self.load_bound_auth()?))
    }
}

impl LeaseScopedAuthSession for LocalLeaseScopedAuthSession {
    fn leased_turn_auth(&self) -> Result<LeasedTurnAuth> {
        self.leased_turn_auth_from_current()
    }

    fn refresh_leased_turn_auth(&self) -> Result<LeasedTurnAuth> {
        self.leased_turn_auth_from_current()
    }

    fn binding(&self) -> &LeaseAuthBinding {
        &self.binding
    }

    fn ensure_current(&self) -> Result<()> {
        let _ = self.load_bound_auth()?;
        Ok(())
    }
}
