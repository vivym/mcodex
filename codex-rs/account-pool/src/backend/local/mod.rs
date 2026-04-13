mod control;
mod execution;

use chrono::Duration;
use codex_state::StateRuntime;
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
}
