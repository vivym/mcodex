use super::LocalAccountPoolBackend;
use crate::backend::AccountPoolExecutionBackend;
use crate::types::LeaseGrant;
use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use codex_login::auth::LeaseAuthBinding;
use codex_login::auth::LocalLeaseScopedAuthSession;
use codex_state::AccountHealthEvent;
use codex_state::AccountLeaseError;
use codex_state::AccountStartupSelectionState;
use codex_state::LeaseKey;
use codex_state::LeaseRenewal;
use std::sync::Arc;

#[async_trait]
impl AccountPoolExecutionBackend for LocalAccountPoolBackend {
    async fn acquire_lease(
        &self,
        pool_id: &str,
        holder_instance_id: &str,
    ) -> std::result::Result<LeaseGrant, AccountLeaseError> {
        let lease = self
            .runtime
            .acquire_account_lease(pool_id, holder_instance_id, self.lease_ttl)
            .await?;
        let registered_account = self
            .runtime
            .read_registered_account(lease.account_id.as_str())
            .await
            .map_err(|err| AccountLeaseError::Storage(err.to_string()))?
            .ok_or_else(|| {
                AccountLeaseError::Storage(
                    "registered account missing for acquired lease".to_string(),
                )
            })?;

        let binding = LeaseAuthBinding {
            account_id: lease.account_id.clone(),
            backend_account_handle: registered_account.backend_account_handle,
            lease_epoch: lease.lease_epoch as u64,
        };
        self.write_backend_private_lease_epoch(
            binding.backend_account_handle.as_str(),
            binding.lease_epoch,
        )
        .await
        .map_err(|err| AccountLeaseError::Storage(err.to_string()))?;
        let auth_home = self.backend_private_auth_home(&binding.backend_account_handle);
        let auth_session = Arc::new(LocalLeaseScopedAuthSession::new(binding, auth_home));
        Ok(LeaseGrant::from_record(lease, auth_session, None))
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
        let released = self.runtime.release_account_lease(lease, now).await?;
        if released
            && let Some(registered_account) = self
                .runtime
                .read_registered_account(lease.account_id.as_str())
                .await?
        {
            self.clear_backend_private_lease_epoch(
                registered_account.backend_account_handle.as_str(),
            )
            .await?;
        }

        Ok(released)
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
