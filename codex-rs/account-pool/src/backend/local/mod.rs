mod control;
mod execution;

use chrono::Duration;
use codex_state::StateRuntime;
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
}
