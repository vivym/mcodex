use super::LocalAccountPoolBackend;
use crate::backend::AccountPoolExecutionBackend;
use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use codex_state::AccountHealthEvent;
use codex_state::AccountLeaseError;
use codex_state::AccountLeaseRecord;
use codex_state::AccountStartupSelectionState;
use codex_state::LeaseKey;
use codex_state::LeaseRenewal;

#[async_trait]
impl AccountPoolExecutionBackend for LocalAccountPoolBackend {
    async fn acquire_lease(
        &self,
        pool_id: &str,
        holder_instance_id: &str,
    ) -> std::result::Result<AccountLeaseRecord, AccountLeaseError> {
        self.runtime
            .acquire_account_lease(pool_id, holder_instance_id, self.lease_ttl)
            .await
    }

    async fn renew_lease(
        &self,
        lease: &LeaseKey,
        now: DateTime<Utc>,
    ) -> anyhow::Result<LeaseRenewal> {
        self.runtime
            .renew_account_lease(lease, now, self.lease_ttl)
            .await
    }

    async fn release_lease(&self, lease: &LeaseKey, now: DateTime<Utc>) -> anyhow::Result<bool> {
        self.runtime.release_account_lease(lease, now).await
    }

    async fn record_health_event(&self, event: AccountHealthEvent) -> anyhow::Result<()> {
        self.runtime.record_account_health_event(event).await
    }

    async fn read_account_health_event_sequence(
        &self,
        account_id: &str,
    ) -> anyhow::Result<Option<i64>> {
        self.runtime
            .read_account_health_event_sequence(account_id)
            .await
    }

    async fn read_startup_selection(&self) -> anyhow::Result<AccountStartupSelectionState> {
        self.runtime.read_account_startup_selection().await
    }
}
