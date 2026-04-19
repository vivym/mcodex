use super::LocalAccountPoolBackend;
use crate::backend::AccountPoolExecutionBackend;
use crate::quota::ProbeOutcome;
use crate::types::LeaseGrant;
use async_trait::async_trait;
use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use codex_login::auth::LeaseAuthBinding;
use codex_login::auth::LocalLeaseScopedAuthSession;
use codex_state::AccountHealthEvent;
use codex_state::AccountLeaseError;
use codex_state::AccountQuotaStateRecord;
use codex_state::AccountStartupSelectionState;
use codex_state::AccountStartupStatus;
use codex_state::LeaseKey;
use codex_state::LeaseRenewal;
use std::sync::Arc;

#[async_trait]
impl AccountPoolExecutionBackend for LocalAccountPoolBackend {
    async fn plan_runtime_selection(
        &self,
        request: &crate::types::SelectionRequest,
        holder_instance_id: &str,
    ) -> anyhow::Result<(String, crate::SelectionPlan)> {
        LocalAccountPoolBackend::plan_runtime_selection(self, request, holder_instance_id).await
    }

    async fn acquire_lease(
        &self,
        pool_id: &str,
        holder_instance_id: &str,
    ) -> std::result::Result<LeaseGrant, AccountLeaseError> {
        self.acquire_lease_excluding(pool_id, holder_instance_id, &[])
            .await
    }

    async fn read_active_holder_lease(
        &self,
        holder_instance_id: &str,
    ) -> anyhow::Result<Option<LeaseGrant>> {
        let Some(lease) = self
            .runtime
            .read_active_holder_lease(holder_instance_id)
            .await?
        else {
            return Ok(None);
        };

        self.grant_for_lease_record(&lease)
            .await
            .map(Some)
            .map_err(|err| {
                anyhow::anyhow!(
                    "failed to rehydrate active holder lease {holder_instance_id}: {err}"
                )
            })
    }

    async fn acquire_lease_excluding(
        &self,
        pool_id: &str,
        holder_instance_id: &str,
        excluded_account_ids: &[String],
    ) -> std::result::Result<LeaseGrant, AccountLeaseError> {
        let lease = self
            .runtime
            .acquire_account_lease_excluding(
                pool_id,
                holder_instance_id,
                self.lease_ttl,
                excluded_account_ids,
            )
            .await?;
        self.grant_for_lease_record(&lease).await
    }

    async fn acquire_preferred_lease(
        &self,
        pool_id: &str,
        account_id: &str,
        holder_instance_id: &str,
    ) -> std::result::Result<LeaseGrant, AccountLeaseError> {
        let lease = self
            .runtime
            .acquire_preferred_account_lease(
                pool_id,
                account_id,
                holder_instance_id,
                self.lease_ttl,
            )
            .await?;
        self.grant_for_lease_record(&lease).await
    }

    async fn acquire_probe_lease(
        &self,
        pool_id: &str,
        account_id: &str,
        selection_family: &str,
        reserved_until: DateTime<Utc>,
        holder_instance_id: &str,
    ) -> std::result::Result<LeaseGrant, AccountLeaseError> {
        let lease = self
            .runtime
            .acquire_quota_probe_account_lease(
                pool_id,
                account_id,
                selection_family,
                reserved_until,
                holder_instance_id,
                self.lease_ttl,
            )
            .await?;
        self.grant_for_lease_record(&lease).await
    }

    async fn reserve_quota_probe(
        &self,
        account_id: &str,
        selection_family: &str,
        now: DateTime<Utc>,
        reserved_for: Duration,
    ) -> anyhow::Result<bool> {
        let Some(quota_state) = self
            .runtime
            .read_selection_quota_state(account_id, selection_family)
            .await?
        else {
            return Ok(false);
        };

        self.runtime
            .reserve_account_quota_probe(
                account_id,
                quota_state.limit_id.as_str(),
                now,
                now + reserved_for,
            )
            .await
    }

    async fn refresh_quota_probe(
        &self,
        lease: &LeaseGrant,
        selection_family: &str,
    ) -> anyhow::Result<Option<ProbeOutcome>> {
        let observed_at = Utc::now();
        let quota_state = self
            .runtime
            .read_selection_quota_state(lease.account_id(), selection_family)
            .await?;
        if lease.auth_session.ensure_current().is_err() {
            if let Some(quota_state) = quota_state {
                let backoff_until = observed_at + Duration::seconds(30);
                let _ = self
                    .runtime
                    .record_account_quota_probe_ambiguous(
                        lease.account_id(),
                        quota_state.limit_id.as_str(),
                        observed_at,
                        backoff_until,
                        backoff_until,
                    )
                    .await?;
                return Ok(Some(ProbeOutcome::Ambiguous));
            }
            return Ok(None);
        };

        let Some(quota_state) = quota_state else {
            return Ok(None);
        };

        self.record_probe_refresh(lease.account_id(), quota_state, observed_at)
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

    async fn read_account_startup_status(
        &self,
        configured_default_pool_id: Option<&str>,
    ) -> anyhow::Result<AccountStartupStatus> {
        self.runtime
            .read_account_startup_status(configured_default_pool_id)
            .await
    }
}

impl LocalAccountPoolBackend {
    async fn grant_for_lease_record(
        &self,
        lease: &codex_state::AccountLeaseRecord,
    ) -> std::result::Result<LeaseGrant, AccountLeaseError> {
        let registered_account = match self
            .runtime
            .read_registered_account(lease.account_id.as_str())
            .await
        {
            Ok(Some(registered_account)) => registered_account,
            Ok(None) => {
                self.release_failed_acquisition(lease, None).await;
                return Err(AccountLeaseError::Storage(
                    "registered account missing for acquired lease".to_string(),
                ));
            }
            Err(err) => {
                self.release_failed_acquisition(lease, None).await;
                return Err(AccountLeaseError::Storage(err.to_string()));
            }
        };

        let binding = LeaseAuthBinding {
            account_id: lease.account_id.clone(),
            backend_account_handle: registered_account.backend_account_handle,
            lease_epoch: lease.lease_epoch as u64,
        };
        if let Err(err) = self
            .write_backend_private_lease_epoch(
                binding.backend_account_handle.as_str(),
                binding.lease_epoch,
            )
            .await
        {
            self.release_failed_acquisition(lease, Some(&binding.backend_account_handle))
                .await;
            return Err(AccountLeaseError::Storage(err.to_string()));
        }
        let auth_home = self.backend_private_auth_home(&binding.backend_account_handle);
        let auth_session = Arc::new(LocalLeaseScopedAuthSession::new(binding, auth_home));
        Ok(LeaseGrant::from_record(lease.clone(), auth_session, None))
    }

    async fn record_probe_refresh(
        &self,
        account_id: &str,
        quota_state: AccountQuotaStateRecord,
        observed_at: DateTime<Utc>,
    ) -> anyhow::Result<Option<ProbeOutcome>> {
        if !quota_state.exhausted_windows.is_exhausted() {
            let _ = self
                .runtime
                .record_account_quota_probe_success(
                    account_id,
                    quota_state.limit_id.as_str(),
                    observed_at,
                )
                .await?;
            return Ok(Some(ProbeOutcome::Success));
        }

        let next_probe_after = observed_at + Duration::seconds(30);
        let _ = self
            .runtime
            .record_account_quota_probe_still_blocked(
                account_id,
                quota_state.limit_id.as_str(),
                observed_at,
                quota_state.exhausted_windows,
                quota_state.predicted_blocked_until,
                next_probe_after,
            )
            .await?;
        Ok(Some(ProbeOutcome::StillBlocked))
    }
}

impl LocalAccountPoolBackend {
    async fn release_failed_acquisition(
        &self,
        lease: &codex_state::AccountLeaseRecord,
        backend_account_handle: Option<&str>,
    ) {
        if let Some(backend_account_handle) = backend_account_handle
            && let Err(err) = self
                .clear_backend_private_lease_epoch(backend_account_handle)
                .await
        {
            eprintln!(
                "failed to clear backend-private lease epoch marker after lease acquisition failure for {backend_account_handle}: {err}"
            );
        }

        if let Err(err) = self
            .runtime
            .release_account_lease(&lease.lease_key(), Utc::now())
            .await
        {
            eprintln!(
                "failed to release acquired lease after lease acquisition failure for lease {} account {}: {err}",
                lease.lease_id, lease.account_id
            );
        }
    }
}
