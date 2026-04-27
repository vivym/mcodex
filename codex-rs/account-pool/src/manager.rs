use crate::SelectionAction;
use crate::SelectionIntent;
use crate::backend::AccountPoolExecutionBackend;
use crate::bootstrap::LegacyAuthBootstrap;
use crate::lease_lifecycle::LeaseHealthEvent;
use crate::proactive_switch::ProactiveSwitchObservation;
use crate::proactive_switch::ProactiveSwitchOutcome;
use crate::proactive_switch::ProactiveSwitchState;
use crate::types::AccountPoolConfig;
use crate::types::HealthEventDisposition;
use crate::types::LeaseGrant;
use crate::types::LeasedAccount;
use crate::types::RateLimitSnapshot;
use crate::types::SelectionRequest;
use crate::types::UsageLimitEvent;
use anyhow::Context;
use chrono::DateTime;
use chrono::Duration;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingRotation {
    HardFailure,
    SoftProactive,
}

pub struct AccountPoolManager<B: AccountPoolExecutionBackend, L: LegacyAuthBootstrap> {
    backend: B,
    legacy_bootstrap: L,
    config: AccountPoolConfig,
    holder_instance_id: String,
    active_lease: Option<LeaseGrant>,
    next_health_event_sequence: i64,
    bootstrapped_legacy_auth: bool,
    proactive_switch_state: ProactiveSwitchState,
    pending_rotation: Option<PendingRotation>,
    last_proactively_replaced_account_id: Option<String>,
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
            proactive_switch_state: ProactiveSwitchState::default(),
            pending_rotation: None,
            last_proactively_replaced_account_id: None,
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
        if let Some(lease) = self.try_rotate_active_lease_if_needed(now).await? {
            return Ok(lease);
        }
        if self.active_lease.is_some() {
            let _ = self.proactive_switch_state.revalidate_before_turn(now);
            match self.try_renew_active_lease_if_needed(now).await? {
                LeaseRenewalDisposition::NotNeeded(lease)
                | LeaseRenewalDisposition::Renewed(lease) => return Ok(lease.leased_account()),
                LeaseRenewalDisposition::Missing { pool_id } => {
                    return self
                        .acquire_selected_lease(SelectionRequest {
                            now: Some(now),
                            pool_id: Some(pool_id),
                            ..request
                        })
                        .await
                        .map_err(Into::into);
                }
            }
        }

        if let Some(lease) = self.try_rehydrate_holder_lease().await? {
            return Ok(lease);
        }

        self.acquire_selected_lease(SelectionRequest {
            pool_id: Some(self.resolve_pool_id(request.pool_id.clone()).await?),
            ..request
        })
        .await
        .map_err(Into::into)
    }

    pub async fn renew_active_lease_if_needed(
        &mut self,
        now: DateTime<Utc>,
    ) -> anyhow::Result<LeasedAccount> {
        if let Some(lease) = self.try_rotate_active_lease_if_needed(now).await? {
            return Ok(lease);
        }
        if self.active_lease.is_some() {
            let _ = self.proactive_switch_state.revalidate_before_turn(now);
            match self.try_renew_active_lease_if_needed(now).await? {
                LeaseRenewalDisposition::NotNeeded(lease)
                | LeaseRenewalDisposition::Renewed(lease) => return Ok(lease.leased_account()),
                LeaseRenewalDisposition::Missing { pool_id } => {
                    return self
                        .acquire_selected_lease(SelectionRequest {
                            now: Some(now),
                            pool_id: Some(pool_id),
                            ..SelectionRequest::default()
                        })
                        .await
                        .map_err(Into::into);
                }
            }
        }

        if let Some(lease) = self.try_rehydrate_holder_lease().await? {
            return Ok(lease);
        }

        self.acquire_selected_lease(SelectionRequest {
            now: Some(now),
            pool_id: Some(
                self.resolve_pool_id(self.config.default_pool_id.clone())
                    .await?,
            ),
            ..SelectionRequest::default()
        })
        .await
        .map_err(Into::into)
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
                self.proactive_switch_state.reset();
                self.pending_rotation = None;
                self.last_proactively_replaced_account_id = None;
            }
        }

        Ok(())
    }

    pub async fn release_active_lease(&mut self) -> anyhow::Result<()> {
        self.release_active_lease_at(Utc::now()).await?;
        self.pending_rotation = None;
        self.last_proactively_replaced_account_id = None;
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
            self.proactive_switch_state.reset();
            if matches!(self.pending_rotation, Some(PendingRotation::SoftProactive)) {
                self.pending_rotation = None;
            }
            return Ok(HealthEventDisposition::Applied);
        }
        let active_lease = self.active_lease.clone().context("active lease missing")?;
        match self
            .proactive_switch_state
            .observe_soft_pressure(ProactiveSwitchObservation {
                lease_acquired_at: active_lease.acquired_at(),
                observed_at: snapshot.observed_at,
                min_switch_interval: self.config.min_switch_interval_duration(),
            }) {
            ProactiveSwitchOutcome::NoAction | ProactiveSwitchOutcome::Suppressed { .. } => {}
            ProactiveSwitchOutcome::RotateOnNextTurn => {
                if self.pending_rotation != Some(PendingRotation::HardFailure) {
                    self.pending_rotation = Some(PendingRotation::SoftProactive);
                }
            }
        }

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
        self.proactive_switch_state.reset();
        self.pending_rotation = Some(PendingRotation::HardFailure);

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
        self.proactive_switch_state.reset();
        self.pending_rotation = Some(PendingRotation::HardFailure);

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
            self.proactive_switch_state.reset();
            self.pending_rotation = None;
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
                self.proactive_switch_state.reset();
                self.pending_rotation = None;
                self.last_proactively_replaced_account_id = None;
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
        let status = self
            .backend
            .read_account_startup_status(self.config.default_pool_id.as_deref())
            .await?;
        if let Some(pool_id) = status.preview.effective_pool_id {
            return Ok(pool_id);
        }
        if let Some(issue) = status.startup_resolution_issue
            && let Some(pool_id) = issue.pool_id
        {
            anyhow::bail!("account pool default is unavailable: {pool_id}");
        }

        anyhow::bail!("account pool has no default pool configured")
    }

    async fn try_rotate_active_lease_if_needed(
        &mut self,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Option<LeasedAccount>> {
        let Some(pending_rotation) = self.pending_rotation.take() else {
            return Ok(None);
        };
        let Some(active_lease) = self.active_lease.clone() else {
            return Ok(None);
        };
        let current_account_id = active_lease.account_id().to_string();
        let pool_id = active_lease.pool_id().to_string();
        let intent = match pending_rotation {
            PendingRotation::HardFailure => SelectionIntent::HardFailover,
            PendingRotation::SoftProactive => SelectionIntent::SoftRotation,
        };
        let leased_account = self
            .acquire_selected_rotation_lease(
                SelectionRequest {
                    now: Some(now),
                    pool_id: Some(pool_id),
                    intent,
                    selection_family: None,
                    preferred_account_id: None,
                    current_account_id: Some(current_account_id.clone()),
                    just_replaced_account_id: self.last_proactively_replaced_account_id.clone(),
                    reserved_probe_target_account_id: None,
                    proactive_threshold_percent: self.config.proactive_switch_threshold_percent,
                },
                active_lease,
            )
            .await?;

        if pending_rotation == PendingRotation::HardFailure {
            self.last_proactively_replaced_account_id = None;
        }
        if pending_rotation == PendingRotation::SoftProactive
            && leased_account.account_id() != current_account_id
        {
            self.last_proactively_replaced_account_id = Some(current_account_id);
        }

        Ok(Some(leased_account))
    }

    async fn release_active_lease_at(&mut self, now: DateTime<Utc>) -> anyhow::Result<()> {
        if let Some(active_lease) = self.active_lease.as_ref() {
            let _ = self.backend.release_lease(&active_lease.key(), now).await?;
        }
        self.active_lease = None;
        self.next_health_event_sequence = 0;
        self.proactive_switch_state.reset();
        Ok(())
    }

    async fn acquire_selected_rotation_lease(
        &mut self,
        request: SelectionRequest,
        active_lease: LeaseGrant,
    ) -> std::result::Result<LeasedAccount, AccountLeaseError> {
        let mut request = request;
        let keep_current_on_no_candidate = matches!(request.intent, SelectionIntent::SoftRotation);
        let current_account_id = active_lease.account_id().to_string();
        let mut current_released = false;
        let mut current_reacquire_exhausted = false;
        let pool_id = request.pool_id.as_deref().ok_or_else(|| {
            AccountLeaseError::Storage("runtime selection requires pool id".to_string())
        })?;
        loop {
            let (selection_family, plan) = self
                .backend
                .plan_runtime_selection(&request, &self.holder_instance_id)
                .await
                .map_err(|err| AccountLeaseError::Storage(err.to_string()))?;

            match plan.terminal_action {
                SelectionAction::Select(account_id) => {
                    if !current_released {
                        self.release_active_lease_at(request.now.unwrap_or_else(Utc::now))
                            .await
                            .map_err(|err| AccountLeaseError::Storage(err.to_string()))?;
                        current_released = true;
                    }
                    match self
                        .backend
                        .acquire_preferred_lease(
                            pool_id,
                            account_id.as_str(),
                            selection_family.as_str(),
                            &self.holder_instance_id,
                        )
                        .await
                    {
                        Ok(grant) => return self.adopt_active_grant(grant).await,
                        Err(AccountLeaseError::NoEligibleAccount) => continue,
                        Err(err) => return Err(err),
                    }
                }
                SelectionAction::Probe(account_id) => {
                    let now = request.now.unwrap_or_else(Utc::now);
                    let Some(reservation) = self
                        .backend
                        .reserve_quota_probe(
                            account_id.as_str(),
                            selection_family.as_str(),
                            now,
                            self.probe_reservation_duration(),
                        )
                        .await
                        .map_err(|err| AccountLeaseError::Storage(err.to_string()))?
                    else {
                        continue;
                    };

                    let probe_holder_instance_id = self.probe_holder_instance_id();
                    let verification_lease = match self
                        .backend
                        .acquire_probe_lease(
                            pool_id,
                            account_id.as_str(),
                            &reservation,
                            probe_holder_instance_id.as_str(),
                        )
                        .await
                    {
                        Ok(lease) => lease,
                        Err(AccountLeaseError::NoEligibleAccount) => continue,
                        Err(err) => return Err(err),
                    };
                    let probe_result = self
                        .backend
                        .refresh_quota_probe(&verification_lease, &reservation)
                        .await
                        .map_err(|err| AccountLeaseError::Storage(err.to_string()));
                    let release_result = self
                        .backend
                        .release_lease(
                            &verification_lease.key(),
                            request.now.unwrap_or_else(Utc::now),
                        )
                        .await
                        .map_err(|err| AccountLeaseError::Storage(err.to_string()));
                    probe_result?;
                    let _ = release_result?;
                    continue;
                }
                SelectionAction::StayOnCurrent | SelectionAction::NoCandidate => {
                    if keep_current_on_no_candidate {
                        if !current_released {
                            return Ok(active_lease.leased_account());
                        }
                        if current_reacquire_exhausted {
                            return Err(AccountLeaseError::NoEligibleAccount);
                        }
                        current_reacquire_exhausted = true;
                        match self
                            .backend
                            .acquire_preferred_lease(
                                pool_id,
                                current_account_id.as_str(),
                                selection_family.as_str(),
                                &self.holder_instance_id,
                            )
                            .await
                        {
                            Ok(grant) => return self.adopt_active_grant(grant).await,
                            Err(AccountLeaseError::NoEligibleAccount) => {
                                request.current_account_id = None;
                                continue;
                            }
                            Err(err) => return Err(err),
                        }
                    }
                    self.release_active_lease_at(request.now.unwrap_or_else(Utc::now))
                        .await
                        .map_err(|err| AccountLeaseError::Storage(err.to_string()))?;
                    return Err(AccountLeaseError::NoEligibleAccount);
                }
            }
        }
    }

    async fn acquire_selected_lease(
        &mut self,
        request: SelectionRequest,
    ) -> std::result::Result<LeasedAccount, AccountLeaseError> {
        let pool_id = request.pool_id.as_deref().ok_or_else(|| {
            AccountLeaseError::Storage("runtime selection requires pool id".to_string())
        })?;
        loop {
            let (selection_family, plan) = self
                .backend
                .plan_runtime_selection(&request, &self.holder_instance_id)
                .await
                .map_err(|err| AccountLeaseError::Storage(err.to_string()))?;

            match plan.terminal_action {
                SelectionAction::Select(account_id) => match self
                    .backend
                    .acquire_preferred_lease(
                        pool_id,
                        account_id.as_str(),
                        selection_family.as_str(),
                        &self.holder_instance_id,
                    )
                    .await
                {
                    Ok(grant) => return self.adopt_active_grant(grant).await,
                    Err(AccountLeaseError::NoEligibleAccount) => continue,
                    Err(err) => return Err(err),
                },
                SelectionAction::Probe(account_id) => {
                    let now = request.now.unwrap_or_else(Utc::now);
                    let Some(reservation) = self
                        .backend
                        .reserve_quota_probe(
                            account_id.as_str(),
                            selection_family.as_str(),
                            now,
                            self.probe_reservation_duration(),
                        )
                        .await
                        .map_err(|err| AccountLeaseError::Storage(err.to_string()))?
                    else {
                        continue;
                    };
                    let verification_lease = match self
                        .backend
                        .acquire_probe_lease(
                            pool_id,
                            account_id.as_str(),
                            &reservation,
                            self.probe_holder_instance_id().as_str(),
                        )
                        .await
                    {
                        Ok(lease) => lease,
                        Err(AccountLeaseError::NoEligibleAccount) => continue,
                        Err(err) => return Err(err),
                    };
                    let probe_result = self
                        .backend
                        .refresh_quota_probe(&verification_lease, &reservation)
                        .await
                        .map_err(|err| AccountLeaseError::Storage(err.to_string()));
                    let release_result = self
                        .backend
                        .release_lease(
                            &verification_lease.key(),
                            request.now.unwrap_or_else(Utc::now),
                        )
                        .await
                        .map_err(|err| AccountLeaseError::Storage(err.to_string()));
                    probe_result?;
                    let _ = release_result?;
                }
                SelectionAction::StayOnCurrent | SelectionAction::NoCandidate => {
                    return Err(AccountLeaseError::NoEligibleAccount);
                }
            }
        }
    }

    async fn adopt_active_grant(
        &mut self,
        grant: LeaseGrant,
    ) -> std::result::Result<LeasedAccount, AccountLeaseError> {
        let leased_account = grant.leased_account();
        self.next_health_event_sequence = self
            .backend
            .read_account_health_event_sequence(leased_account.account_id())
            .await
            .map_err(|err| AccountLeaseError::Storage(err.to_string()))?
            .unwrap_or(0);
        self.active_lease = Some(grant);
        self.proactive_switch_state.reset();
        Ok(leased_account)
    }

    async fn try_rehydrate_holder_lease(&mut self) -> anyhow::Result<Option<LeasedAccount>> {
        let Some(grant) = self
            .backend
            .read_active_holder_lease(&self.holder_instance_id)
            .await?
        else {
            return Ok(None);
        };

        self.adopt_active_grant(grant)
            .await
            .map(Some)
            .map_err(|err| anyhow::anyhow!(err.to_string()))
    }

    fn probe_holder_instance_id(&self) -> String {
        format!("{}:probe", self.holder_instance_id)
    }

    fn probe_reservation_duration(&self) -> Duration {
        Duration::seconds(30)
    }
}
