use crate::backend::AccountPoolExecutionBackend;
use crate::bootstrap::LegacyAuthBootstrap;
use crate::lease_lifecycle::LeaseHealthEvent;
use crate::types::AccountPoolConfig;
use crate::types::HealthEventDisposition;
use crate::types::LeaseGrant;
use crate::types::LeasedAccount;
use crate::types::RateLimitSnapshot;
use crate::types::SelectionRequest;
use crate::types::UsageLimitEvent;
use anyhow::Context;
use chrono::DateTime;
use chrono::Utc;
use codex_state::AccountLeaseError;
use codex_state::AccountStartupSelectionState;
use codex_state::LeaseKey;
use codex_state::LeaseRenewal;

enum LeaseRenewalDisposition {
    NotNeeded(LeaseGrant),
    Renewed(LeaseGrant),
    Missing { pool_id: String },
}

pub struct AccountPoolManager<B: AccountPoolExecutionBackend, L: LegacyAuthBootstrap> {
    backend: B,
    legacy_bootstrap: L,
    config: AccountPoolConfig,
    holder_instance_id: String,
    active_lease: Option<LeaseGrant>,
    next_health_event_sequence: i64,
    bootstrapped_legacy_auth: bool,
}

impl<B: AccountPoolExecutionBackend, L: LegacyAuthBootstrap> AccountPoolManager<B, L> {
    pub fn new(
        backend: B,
        legacy_bootstrap: L,
        config: AccountPoolConfig,
        holder_instance_id: String,
    ) -> anyhow::Result<Self> {
        config.validate()?;

        Ok(Self {
            backend,
            legacy_bootstrap,
            config,
            holder_instance_id,
            active_lease: None,
            next_health_event_sequence: 0,
            bootstrapped_legacy_auth: false,
        })
    }

    pub async fn bootstrap_from_legacy_auth(&mut self) -> anyhow::Result<()> {
        if self.bootstrapped_legacy_auth {
            return Ok(());
        }

        let startup = self.backend.read_startup_selection().await?;
        if startup.default_pool_id.is_some()
            || startup.preferred_account_id.is_some()
            || startup.suppressed
        {
            self.bootstrapped_legacy_auth = true;
            return Ok(());
        }

        let _ = self.legacy_bootstrap.current_legacy_auth().await?;

        self.bootstrapped_legacy_auth = true;
        Ok(())
    }

    pub async fn ensure_active_lease(
        &mut self,
        request: SelectionRequest,
    ) -> anyhow::Result<LeasedAccount> {
        let now = request.now.unwrap_or_else(Utc::now);
        if self.active_lease.is_some() {
            match self.try_renew_active_lease_if_needed(now).await? {
                LeaseRenewalDisposition::NotNeeded(lease)
                | LeaseRenewalDisposition::Renewed(lease) => return Ok(lease.leased_account()),
                LeaseRenewalDisposition::Missing { pool_id } => {
                    return self.acquire_fresh_lease(pool_id).await;
                }
            }
        }

        self.acquire_fresh_lease(self.resolve_pool_id(request.pool_id).await?)
            .await
    }

    pub async fn renew_active_lease_if_needed(
        &mut self,
        now: DateTime<Utc>,
    ) -> anyhow::Result<LeasedAccount> {
        if self.active_lease.is_some() {
            match self.try_renew_active_lease_if_needed(now).await? {
                LeaseRenewalDisposition::NotNeeded(lease)
                | LeaseRenewalDisposition::Renewed(lease) => return Ok(lease.leased_account()),
                LeaseRenewalDisposition::Missing { pool_id } => {
                    return self.acquire_fresh_lease(pool_id).await;
                }
            }
        }

        self.acquire_fresh_lease(
            self.resolve_pool_id(self.config.default_pool_id.clone())
                .await?,
        )
        .await
    }

    pub async fn heartbeat_active_lease(&mut self, now: DateTime<Utc>) -> anyhow::Result<()> {
        let Some(active_lease) = self.active_lease.clone() else {
            return Ok(());
        };

        match self.backend.renew_lease(&active_lease.key(), now).await? {
            LeaseRenewal::Renewed(record) => {
                self.active_lease = Some(active_lease.with_record(record));
            }
            LeaseRenewal::Missing => {
                self.active_lease = None;
                self.next_health_event_sequence = 0;
            }
        }

        Ok(())
    }

    pub async fn release_active_lease(&mut self) -> anyhow::Result<()> {
        if let Some(active_lease) = self.active_lease.as_ref() {
            let _ = self
                .backend
                .release_lease(&active_lease.key(), Utc::now())
                .await?;
        }
        self.active_lease = None;
        self.next_health_event_sequence = 0;
        Ok(())
    }

    pub async fn report_rate_limits(
        &mut self,
        lease: LeaseKey,
        snapshot: RateLimitSnapshot,
    ) -> anyhow::Result<HealthEventDisposition> {
        if !self.is_current_lease(&lease) {
            return Ok(HealthEventDisposition::IgnoredAsStale);
        }

        if snapshot.used_percent < f64::from(self.config.proactive_switch_threshold_percent) {
            return Ok(HealthEventDisposition::Applied);
        }

        let sequence_number = self.next_health_event_sequence + 1;
        self.next_health_event_sequence = sequence_number;
        let active_lease = self.active_lease.clone().context("active lease missing")?;
        let event = LeaseHealthEvent::RateLimited {
            observed_at: snapshot.observed_at,
        }
        .into_account_health_event(&active_lease.leased_account(), sequence_number);
        self.backend.record_health_event(event).await?;

        Ok(HealthEventDisposition::Applied)
    }

    pub async fn report_usage_limit_reached(
        &mut self,
        lease: LeaseKey,
        event: UsageLimitEvent,
    ) -> anyhow::Result<HealthEventDisposition> {
        if !self.is_current_lease(&lease) {
            return Ok(HealthEventDisposition::IgnoredAsStale);
        }

        let sequence_number = self.next_health_event_sequence + 1;
        self.next_health_event_sequence = sequence_number;
        let active_lease = self.active_lease.clone().context("active lease missing")?;
        let health_event = LeaseHealthEvent::RateLimited {
            observed_at: event.observed_at,
        }
        .into_account_health_event(&active_lease.leased_account(), sequence_number);
        self.backend.record_health_event(health_event).await?;

        Ok(HealthEventDisposition::Applied)
    }

    pub async fn report_unauthorized(
        &mut self,
        lease: LeaseKey,
    ) -> anyhow::Result<HealthEventDisposition> {
        if !self.is_current_lease(&lease) {
            return Ok(HealthEventDisposition::IgnoredAsStale);
        }

        let sequence_number = self.next_health_event_sequence + 1;
        self.next_health_event_sequence = sequence_number;
        let active_lease = self.active_lease.clone().context("active lease missing")?;
        let health_event = LeaseHealthEvent::Unauthorized {
            observed_at: Utc::now(),
        }
        .into_account_health_event(&active_lease.leased_account(), sequence_number);
        self.backend.record_health_event(health_event).await?;

        Ok(HealthEventDisposition::Applied)
    }

    pub fn force_epoch_bump_for_test(&mut self, account_id: &str) -> anyhow::Result<()> {
        if let Some(active_lease) = self.active_lease.as_mut()
            && active_lease.account_id() == account_id
        {
            *active_lease = active_lease
                .clone()
                .with_lease_epoch(active_lease.lease_epoch() + 1);
            self.next_health_event_sequence = 0;
        }

        Ok(())
    }

    pub async fn read_startup_selection_for_test(
        &self,
    ) -> anyhow::Result<AccountStartupSelectionState> {
        self.backend.read_startup_selection().await
    }

    fn is_current_lease(&self, lease: &LeaseKey) -> bool {
        self.active_lease.as_ref().is_some_and(|active_lease| {
            active_lease.key().lease_id == lease.lease_id
                && active_lease.key().account_id == lease.account_id
                && active_lease.key().lease_epoch == lease.lease_epoch
        })
    }

    async fn try_renew_active_lease_if_needed(
        &mut self,
        now: DateTime<Utc>,
    ) -> anyhow::Result<LeaseRenewalDisposition> {
        let active_lease = self
            .active_lease
            .clone()
            .context("active lease missing in renewal path")?;

        if active_lease.remaining_ttl(now) > self.config.derived_pre_turn_safety_margin() {
            return Ok(LeaseRenewalDisposition::NotNeeded(active_lease));
        }

        match self.backend.renew_lease(&active_lease.key(), now).await? {
            LeaseRenewal::Renewed(record) => {
                let renewed = active_lease.with_record(record);
                self.active_lease = Some(renewed.clone());
                Ok(LeaseRenewalDisposition::Renewed(renewed))
            }
            LeaseRenewal::Missing => {
                self.active_lease = None;
                self.next_health_event_sequence = 0;
                Ok(LeaseRenewalDisposition::Missing {
                    pool_id: active_lease.pool_id().to_string(),
                })
            }
        }
    }

    async fn resolve_pool_id(&self, requested_pool_id: Option<String>) -> anyhow::Result<String> {
        if let Some(pool_id) = requested_pool_id {
            return Ok(pool_id);
        }
        if let Some(pool_id) = self.config.default_pool_id.clone() {
            return Ok(pool_id);
        }

        self.backend
            .read_startup_selection()
            .await?
            .default_pool_id
            .context("account pool has no default pool configured")
    }

    async fn acquire_fresh_lease(&mut self, pool_id: String) -> anyhow::Result<LeasedAccount> {
        let grant = self
            .backend
            .acquire_lease(&pool_id, &self.holder_instance_id)
            .await
            .map_err(|err| match err {
                AccountLeaseError::NoEligibleAccount => anyhow::anyhow!(err),
                AccountLeaseError::Storage(_) => anyhow::anyhow!(err),
            })?;
        let leased_account = grant.leased_account();
        self.next_health_event_sequence = self
            .backend
            .read_account_health_event_sequence(leased_account.account_id())
            .await?
            .unwrap_or(0);
        self.active_lease = Some(grant);
        Ok(leased_account)
    }
}
