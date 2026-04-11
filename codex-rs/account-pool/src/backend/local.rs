use crate::backend::AccountPoolLeaseBackend;
use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use codex_state::AccountHealthEvent;
use codex_state::AccountLeaseError;
use codex_state::AccountLeaseRecord;
use codex_state::AccountStartupSelectionState;
use codex_state::LeaseKey;
use codex_state::LeaseRenewal;
use codex_state::LegacyAccountImport;
use codex_state::StateRuntime;
use std::sync::Arc;

/// Local backend backed by `codex-state` SQLite persistence.
#[derive(Clone)]
pub struct LocalAccountPoolBackend {
    runtime: Arc<StateRuntime>,
}

impl LocalAccountPoolBackend {
    pub fn new(runtime: Arc<StateRuntime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl AccountPoolLeaseBackend for LocalAccountPoolBackend {
    async fn acquire_lease(
        &self,
        pool_id: &str,
        holder_instance_id: &str,
    ) -> std::result::Result<AccountLeaseRecord, AccountLeaseError> {
        self.runtime
            .acquire_account_lease(pool_id, holder_instance_id)
            .await
    }

    async fn renew_lease(
        &self,
        lease: &LeaseKey,
        now: DateTime<Utc>,
    ) -> anyhow::Result<LeaseRenewal> {
        self.runtime.renew_account_lease(lease, now).await
    }

    async fn record_health_event(&self, event: AccountHealthEvent) -> anyhow::Result<()> {
        self.runtime.record_account_health_event(event).await
    }

    async fn read_startup_selection(&self) -> anyhow::Result<AccountStartupSelectionState> {
        self.runtime.read_account_startup_selection().await
    }

    async fn import_legacy_default_account(
        &self,
        legacy_account: LegacyAccountImport,
    ) -> anyhow::Result<()> {
        self.runtime
            .import_legacy_default_account(legacy_account)
            .await
    }
}
