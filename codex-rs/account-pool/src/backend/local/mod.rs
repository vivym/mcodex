mod control;
mod execution;

use chrono::Duration;
use codex_login::auth::LocalLeaseScopedAuthSession;
use codex_state::StateRuntime;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

/// Local backend backed by `codex-state` SQLite persistence.
#[derive(Clone)]
pub struct LocalAccountPoolBackend {
    runtime: Arc<StateRuntime>,
    lease_ttl: Duration,
}

impl LocalAccountPoolBackend {
    pub fn new(runtime: Arc<StateRuntime>, lease_ttl: Duration) -> Self {
        Self { runtime, lease_ttl }
    }

    pub(crate) fn backend_private_auth_home(&self, backend_account_handle: &str) -> PathBuf {
        self.runtime
            .codex_home()
            .join(".pooled-auth/backends/local/accounts")
            .join(backend_account_handle)
    }

    pub(crate) async fn write_backend_private_lease_epoch(
        &self,
        backend_account_handle: &str,
        lease_epoch: u64,
    ) -> anyhow::Result<()> {
        let auth_home = self.backend_private_auth_home(backend_account_handle);
        LocalLeaseScopedAuthSession::write_lease_epoch_marker(auth_home.as_path(), lease_epoch)
    }

    pub(crate) async fn clear_backend_private_lease_epoch(
        &self,
        backend_account_handle: &str,
    ) -> anyhow::Result<()> {
        let auth_home = self.backend_private_auth_home(backend_account_handle);
        LocalLeaseScopedAuthSession::clear_lease_epoch_marker(auth_home.as_path())
    }

    pub(crate) async fn clear_backend_private_auth_namespace(
        &self,
        backend_account_handle: &str,
    ) -> anyhow::Result<()> {
        let auth_home = self.backend_private_auth_home(backend_account_handle);
        match fs::remove_dir_all(&auth_home) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}
