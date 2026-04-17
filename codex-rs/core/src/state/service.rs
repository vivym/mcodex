use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use crate::RolloutRecorder;
use crate::SkillsManager;
use crate::agent::AgentControl;
use crate::client::ModelClient;
use crate::config::StartedNetworkProxy;
use crate::exec_policy::ExecPolicyManager;
use crate::guardian::GuardianRejection;
use crate::mcp::McpManager;
use crate::plugins::PluginsManager;
use crate::skills_watcher::SkillsWatcher;
use crate::tools::code_mode::CodeModeService;
use crate::tools::network_approval::NetworkApprovalService;
use crate::tools::sandboxing::ApprovalStore;
use crate::unified_exec::UnifiedExecProcessManager;
use anyhow::Context;
use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use codex_account_pool::ProactiveSwitchObservation;
use codex_account_pool::ProactiveSwitchOutcome;
use codex_account_pool::ProactiveSwitchSnapshot;
use codex_account_pool::ProactiveSwitchState;
use codex_analytics::AnalyticsEventsClient;
use codex_app_server_protocol::AccountPoolEventType;
use codex_app_server_protocol::AccountPoolReasonCode;
use codex_config::types::AccountsConfigToml;
use codex_exec_server::Environment;
use codex_hooks::Hooks;
use codex_login::AuthManager;
use codex_login::auth::LeaseAuthBinding;
use codex_login::auth::LeaseScopedAuthSession;
use codex_login::auth::LocalLeaseScopedAuthSession;
use codex_mcp::McpConnectionManager;
use codex_models_manager::manager::ModelsManager;
use codex_otel::SessionTelemetry;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_rollout::state_db::StateDbHandle;
use codex_state::AccountHealthEvent;
use codex_state::AccountHealthState;
use codex_state::AccountLeaseError;
use codex_state::AccountLeaseRecord;
use codex_state::AccountPoolEventRecord;
use codex_state::AccountStartupEligibility;
use codex_state::LeaseRenewal;
use std::path::PathBuf;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(crate) struct SessionServices {
    pub(crate) mcp_connection_manager: Arc<RwLock<McpConnectionManager>>,
    pub(crate) mcp_startup_cancellation_token: Mutex<CancellationToken>,
    pub(crate) unified_exec_manager: UnifiedExecProcessManager,
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) shell_zsh_path: Option<PathBuf>,
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) main_execve_wrapper_exe: Option<PathBuf>,
    pub(crate) analytics_events_client: AnalyticsEventsClient,
    pub(crate) hooks: Hooks,
    pub(crate) rollout: Mutex<Option<RolloutRecorder>>,
    pub(crate) user_shell: Arc<crate::shell::Shell>,
    pub(crate) shell_snapshot_tx: watch::Sender<Option<Arc<crate::shell_snapshot::ShellSnapshot>>>,
    pub(crate) show_raw_agent_reasoning: bool,
    pub(crate) exec_policy: Arc<ExecPolicyManager>,
    pub(crate) auth_manager: Arc<AuthManager>,
    pub(crate) models_manager: Arc<ModelsManager>,
    pub(crate) session_telemetry: SessionTelemetry,
    pub(crate) tool_approvals: Mutex<ApprovalStore>,
    pub(crate) guardian_rejections: Mutex<HashMap<String, GuardianRejection>>,
    pub(crate) skills_manager: Arc<SkillsManager>,
    pub(crate) plugins_manager: Arc<PluginsManager>,
    pub(crate) mcp_manager: Arc<McpManager>,
    pub(crate) skills_watcher: Arc<SkillsWatcher>,
    pub(crate) agent_control: AgentControl,
    pub(crate) network_proxy: Option<StartedNetworkProxy>,
    pub(crate) network_approval: Arc<NetworkApprovalService>,
    pub(crate) state_db: Option<StateDbHandle>,
    pub(crate) account_pool_manager: Option<Arc<Mutex<AccountPoolManager>>>,
    pub(crate) lease_auth: Arc<crate::lease_auth::SessionLeaseAuth>,
    /// Session-scoped model client shared across turns.
    pub(crate) model_client: ModelClient,
    pub(crate) code_mode_service: CodeModeService,
    pub(crate) environment: Option<Arc<Environment>>,
}

impl SessionServices {
    pub(crate) fn build_account_pool_manager(
        state_db: Option<StateDbHandle>,
        accounts: Option<AccountsConfigToml>,
        codex_home: PathBuf,
        holder_instance_id: String,
    ) -> Option<Arc<Mutex<AccountPoolManager>>> {
        let state_db = state_db?;
        let accounts = accounts?;
        Some(Arc::new(Mutex::new(AccountPoolManager::new(
            state_db,
            accounts,
            codex_home,
            holder_instance_id,
        ))))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountLeaseRuntimeSnapshot {
    pub active: bool,
    pub suppressed: bool,
    pub account_id: Option<String>,
    pub pool_id: Option<String>,
    pub lease_id: Option<String>,
    pub lease_epoch: Option<i64>,
    pub lease_acquired_at: Option<DateTime<Utc>>,
    pub health_state: Option<AccountHealthState>,
    pub switch_reason: Option<AccountLeaseRuntimeReason>,
    pub suppression_reason: Option<AccountLeaseRuntimeReason>,
    pub transport_reset_generation: Option<u64>,
    pub last_remote_context_reset_turn_id: Option<String>,
    pub min_switch_interval_secs: Option<u64>,
    pub proactive_switch_pending: Option<bool>,
    pub proactive_switch_suppressed: Option<bool>,
    pub proactive_switch_allowed_at: Option<DateTime<Utc>>,
    pub next_eligible_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountLeaseRuntimeReason {
    StartupSuppressed,
    MissingPool,
    PreferredAccountSelected,
    AutomaticAccountSelected,
    PreferredAccountMissing,
    PreferredAccountInOtherPool,
    PreferredAccountDisabled,
    PreferredAccountUnhealthy,
    PreferredAccountBusy,
    NoEligibleAccount,
    NonReplayableTurn,
}

impl From<&AccountStartupEligibility> for AccountLeaseRuntimeReason {
    fn from(value: &AccountStartupEligibility) -> Self {
        match value {
            AccountStartupEligibility::Suppressed => Self::StartupSuppressed,
            AccountStartupEligibility::MissingPool => Self::MissingPool,
            AccountStartupEligibility::PreferredAccountSelected => Self::PreferredAccountSelected,
            AccountStartupEligibility::AutomaticAccountSelected => Self::AutomaticAccountSelected,
            AccountStartupEligibility::PreferredAccountMissing => Self::PreferredAccountMissing,
            AccountStartupEligibility::PreferredAccountInOtherPool { .. } => {
                Self::PreferredAccountInOtherPool
            }
            AccountStartupEligibility::PreferredAccountDisabled => Self::PreferredAccountDisabled,
            AccountStartupEligibility::PreferredAccountUnhealthy => Self::PreferredAccountUnhealthy,
            AccountStartupEligibility::PreferredAccountBusy => Self::PreferredAccountBusy,
            AccountStartupEligibility::NoEligibleAccount => Self::NoEligibleAccount,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingRotation {
    HardFailure,
    SoftProactive,
}

pub(crate) struct AccountPoolManager {
    state_db: StateDbHandle,
    codex_home: PathBuf,
    default_pool_id: Option<String>,
    proactive_switch_threshold_percent: u8,
    min_switch_interval: Duration,
    allow_context_reuse_by_pool_id: HashMap<String, bool>,
    lease_ttl: Duration,
    heartbeat_interval: StdDuration,
    holder_instance_id: String,
    active_lease: Option<ActiveAccountLease>,
    next_health_event_sequence: i64,
    previous_turn_account_id: Option<String>,
    proactive_switch_state: ProactiveSwitchState,
    pending_rotation: Option<PendingRotation>,
    last_proactively_replaced_account_id: Option<String>,
    switch_reason: Option<AccountLeaseRuntimeReason>,
    suppression_reason: Option<AccountLeaseRuntimeReason>,
    transport_reset_generation: u64,
    last_remote_context_reset_turn_id: Option<String>,
}

pub(crate) struct AccountPoolManagerSnapshotSeed {
    state_db: StateDbHandle,
    holder_instance_id: String,
    active_lease: Option<AccountLeaseRecord>,
    min_switch_interval_secs: u64,
    proactive_switch_snapshot: Option<ProactiveSwitchSnapshot>,
    switch_reason: Option<AccountLeaseRuntimeReason>,
    suppression_reason: Option<AccountLeaseRuntimeReason>,
    transport_reset_generation: u64,
    last_remote_context_reset_turn_id: Option<String>,
}

#[derive(Clone)]
struct ActiveAccountLease {
    record: AccountLeaseRecord,
    auth_session: Arc<dyn LeaseScopedAuthSession>,
}

pub(crate) struct TurnAccountSelection {
    pub(crate) account_id: String,
    pub(crate) reset_remote_context: bool,
    pub(crate) auth_session: Arc<dyn LeaseScopedAuthSession>,
}

impl AccountPoolManager {
    fn new(
        state_db: StateDbHandle,
        accounts: AccountsConfigToml,
        codex_home: PathBuf,
        holder_instance_id: String,
    ) -> Self {
        let default_pool_id = accounts.default_pool.clone();
        let allow_context_reuse_by_pool_id = accounts
            .pools
            .unwrap_or_default()
            .into_iter()
            .map(|(pool_id, pool)| (pool_id, pool.allow_context_reuse.unwrap_or(true)))
            .collect();
        let proactive_switch_threshold_percent =
            accounts.proactive_switch_threshold_percent.unwrap_or(85);
        let min_switch_interval_secs = accounts.min_switch_interval_secs.unwrap_or(0);
        let lease_ttl_secs = accounts.lease_ttl_secs.unwrap_or(300);
        let default_heartbeat_interval_secs = (lease_ttl_secs / 3).max(1);
        let max_heartbeat_interval_secs = lease_ttl_secs.saturating_sub(1).max(1);
        let heartbeat_interval_secs = accounts
            .heartbeat_interval_secs
            .unwrap_or(default_heartbeat_interval_secs)
            .clamp(1, max_heartbeat_interval_secs);

        Self {
            state_db,
            codex_home,
            default_pool_id,
            proactive_switch_threshold_percent,
            min_switch_interval: Duration::seconds(min_switch_interval_secs as i64),
            allow_context_reuse_by_pool_id,
            lease_ttl: Duration::seconds(lease_ttl_secs as i64),
            heartbeat_interval: StdDuration::from_secs(heartbeat_interval_secs),
            holder_instance_id,
            active_lease: None,
            next_health_event_sequence: 0,
            previous_turn_account_id: None,
            proactive_switch_state: ProactiveSwitchState::default(),
            pending_rotation: None,
            last_proactively_replaced_account_id: None,
            switch_reason: None,
            suppression_reason: None,
            transport_reset_generation: 0,
            last_remote_context_reset_turn_id: None,
        }
    }

    pub(crate) async fn prepare_turn(&mut self) -> anyhow::Result<Option<TurnAccountSelection>> {
        let now = Utc::now();
        let _ = self.proactive_switch_state.revalidate_before_turn(now);
        self.switch_reason = None;
        let pending_rotation = self.pending_rotation.take();
        let rotation_context = pending_rotation.and_then(|pending_rotation| {
            self.active_lease.as_ref().map(|active_lease| {
                (
                    pending_rotation,
                    active_lease.record.pool_id.clone(),
                    active_lease.record.account_id.clone(),
                )
            })
        });
        if let Some(pending_rotation) = pending_rotation {
            self.release_active_lease().await?;
            if pending_rotation == PendingRotation::HardFailure {
                self.last_proactively_replaced_account_id = None;
            }
        }

        self.renew_active_lease().await?;
        if self.active_lease.is_some() {
            self.suppression_reason = None;
        }

        if self.active_lease.is_none() {
            let startup_preview = self
                .state_db
                .preview_account_startup_selection(self.default_pool_id.as_deref())
                .await?;
            let Some(pool_id) = startup_preview.effective_pool_id.clone() else {
                self.suppression_reason = Some(AccountLeaseRuntimeReason::MissingPool);
                return Ok(None);
            };
            let selection_reason = AccountLeaseRuntimeReason::from(&startup_preview.eligibility);
            let lease_result = match rotation_context.as_ref() {
                Some((_, _, _))
                    if matches!(
                        startup_preview.eligibility,
                        AccountStartupEligibility::Suppressed
                    ) =>
                {
                    self.suppression_reason = Some(selection_reason);
                    return Ok(None);
                }
                Some((PendingRotation::HardFailure, _, _)) => {
                    self.state_db
                        .acquire_account_lease(&pool_id, &self.holder_instance_id, self.lease_ttl)
                        .await
                }
                Some((PendingRotation::SoftProactive, _, current_account_id)) => {
                    self.acquire_proactively_rotated_lease(&pool_id, current_account_id)
                        .await
                }
                None => match (
                    startup_preview.predicted_account_id.as_deref(),
                    &startup_preview.eligibility,
                ) {
                    (Some(account_id), AccountStartupEligibility::PreferredAccountSelected) => {
                        self.state_db
                            .acquire_preferred_account_lease(
                                &pool_id,
                                account_id,
                                &self.holder_instance_id,
                                self.lease_ttl,
                            )
                            .await
                    }
                    (Some(_), AccountStartupEligibility::AutomaticAccountSelected) => {
                        self.state_db
                            .acquire_account_lease(
                                &pool_id,
                                &self.holder_instance_id,
                                self.lease_ttl,
                            )
                            .await
                    }
                    (
                        None,
                        AccountStartupEligibility::Suppressed
                        | AccountStartupEligibility::MissingPool
                        | AccountStartupEligibility::PreferredAccountMissing
                        | AccountStartupEligibility::PreferredAccountInOtherPool { .. }
                        | AccountStartupEligibility::PreferredAccountDisabled
                        | AccountStartupEligibility::PreferredAccountUnhealthy
                        | AccountStartupEligibility::PreferredAccountBusy
                        | AccountStartupEligibility::NoEligibleAccount,
                    ) => {
                        self.suppression_reason = Some(selection_reason);
                        return Ok(None);
                    }
                    (Some(account_id), eligibility) => {
                        return Err(anyhow::anyhow!(
                            "unexpected startup-selection preview {eligibility:?} for predicted account {account_id}"
                        ));
                    }
                    (None, AccountStartupEligibility::PreferredAccountSelected)
                    | (None, AccountStartupEligibility::AutomaticAccountSelected) => {
                        return Err(anyhow::anyhow!(
                            "startup-selection preview did not include a predicted account"
                        ));
                    }
                },
            };
            match lease_result {
                Ok(lease) => {
                    match rotation_context.clone() {
                        Some((PendingRotation::SoftProactive, pool_id, current_account_id))
                            if lease.account_id != current_account_id =>
                        {
                            self.last_proactively_replaced_account_id = Some(current_account_id);
                            self.append_runtime_account_pool_event(RuntimeAccountPoolEvent {
                                occurred_at: lease.acquired_at,
                                pool_id: &pool_id,
                                account_id: Some(lease.account_id.as_str()),
                                lease_id: Some(lease.lease_id.as_str()),
                                event_type: AccountPoolEventType::ProactiveSwitchSelected,
                                reason_code: Some(AccountPoolReasonCode::QuotaNearExhausted),
                                message: format!(
                                    "proactive switch selected {} after quota pressure",
                                    lease.account_id
                                ),
                            })
                            .await?;
                        }
                        Some((PendingRotation::SoftProactive, pool_id, current_account_id)) => {
                            self.append_runtime_account_pool_event(RuntimeAccountPoolEvent {
                                occurred_at: lease.acquired_at,
                                pool_id: &pool_id,
                                account_id: None,
                                lease_id: None,
                                event_type: AccountPoolEventType::LeaseAcquireFailed,
                                reason_code: Some(AccountPoolReasonCode::NoEligibleAccount),
                                message: format!(
                                    "proactive switch could not select an alternate eligible account for {current_account_id}"
                                ),
                            })
                            .await?;
                        }
                        _ => {}
                    }
                    let auth_session = self.create_auth_session(&lease).await?;
                    self.next_health_event_sequence = self
                        .state_db
                        .read_account_health_event_sequence(&lease.account_id)
                        .await?
                        .unwrap_or(0);
                    self.suppression_reason = None;
                    if self
                        .previous_turn_account_id
                        .as_deref()
                        .is_some_and(|previous| previous != lease.account_id)
                    {
                        self.switch_reason =
                            Some(AccountLeaseRuntimeReason::AutomaticAccountSelected);
                    }
                    self.active_lease = Some(ActiveAccountLease {
                        record: lease,
                        auth_session,
                    });
                }
                Err(AccountLeaseError::NoEligibleAccount) => {
                    self.suppression_reason = Some(AccountLeaseRuntimeReason::NoEligibleAccount);
                    return Ok(None);
                }
                Err(AccountLeaseError::Storage(message)) => return Err(anyhow::anyhow!(message)),
            }
        }

        let active_lease = self
            .active_lease
            .as_ref()
            .context("active lease missing after account pool acquisition")?;
        let account_id = active_lease.record.account_id.clone();
        let allow_context_reuse = self
            .allow_context_reuse_by_pool_id
            .get(&active_lease.record.pool_id)
            .copied()
            .unwrap_or(true);
        let reset_remote_context = self
            .previous_turn_account_id
            .as_deref()
            .is_some_and(|previous| previous != account_id)
            && !allow_context_reuse;
        self.previous_turn_account_id = Some(account_id.clone());
        Ok(Some(TurnAccountSelection {
            account_id,
            reset_remote_context,
            auth_session: Arc::clone(&active_lease.auth_session),
        }))
    }

    pub(crate) fn snapshot_seed(&self) -> AccountPoolManagerSnapshotSeed {
        let proactive_switch_snapshot = self.active_lease.as_ref().map(|_| {
            let mut proactive_switch_state = self.proactive_switch_state.clone();
            proactive_switch_state.snapshot(Utc::now())
        });
        AccountPoolManagerSnapshotSeed {
            state_db: Arc::clone(&self.state_db),
            holder_instance_id: self.holder_instance_id.clone(),
            active_lease: self.active_lease.as_ref().map(|lease| lease.record.clone()),
            min_switch_interval_secs: self.min_switch_interval.num_seconds().max(0) as u64,
            proactive_switch_snapshot,
            switch_reason: self.switch_reason,
            suppression_reason: self.suppression_reason,
            transport_reset_generation: self.transport_reset_generation,
            last_remote_context_reset_turn_id: self.last_remote_context_reset_turn_id.clone(),
        }
    }

    pub(crate) fn record_remote_context_reset(&mut self, turn_id: &str) {
        self.transport_reset_generation += 1;
        self.last_remote_context_reset_turn_id = Some(turn_id.to_string());
    }

    pub(crate) fn heartbeat_interval(&self) -> StdDuration {
        self.heartbeat_interval
    }

    pub(crate) async fn renew_active_lease(&mut self) -> anyhow::Result<()> {
        if let Some(active_lease) = self.active_lease.clone() {
            match self
                .state_db
                .renew_account_lease(&active_lease.record.lease_key(), Utc::now(), self.lease_ttl)
                .await?
            {
                LeaseRenewal::Renewed(record) => {
                    self.active_lease = Some(ActiveAccountLease {
                        record,
                        auth_session: active_lease.auth_session,
                    });
                }
                LeaseRenewal::Missing => {
                    self.clear_auth_marker(&active_lease.record).await?;
                    self.active_lease = None;
                    self.next_health_event_sequence = 0;
                    self.proactive_switch_state.reset();
                    self.pending_rotation = None;
                    self.last_proactively_replaced_account_id = None;
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn release_for_shutdown(&mut self) -> anyhow::Result<()> {
        self.release_active_lease().await?;
        self.pending_rotation = None;
        self.last_proactively_replaced_account_id = None;
        Ok(())
    }

    pub(crate) async fn report_rate_limits(
        &mut self,
        snapshot: &RateLimitSnapshot,
    ) -> anyhow::Result<()> {
        let Some(window) = snapshot.primary.as_ref() else {
            return Ok(());
        };
        if window.used_percent < f64::from(self.proactive_switch_threshold_percent) {
            self.proactive_switch_state.reset();
            if matches!(self.pending_rotation, Some(PendingRotation::SoftProactive)) {
                self.pending_rotation = None;
            }
            return Ok(());
        }
        let Some(active_lease) = self.active_lease.as_ref() else {
            return Ok(());
        };
        let observed_at = Utc::now();
        let pool_id = active_lease.record.pool_id.clone();
        let account_id = active_lease.record.account_id.clone();
        let lease_id = active_lease.record.lease_id.clone();
        let lease_acquired_at = active_lease.record.acquired_at;
        let was_suppressed = self
            .proactive_switch_state
            .clone()
            .snapshot(observed_at)
            .suppressed;
        match self
            .proactive_switch_state
            .observe_soft_pressure(ProactiveSwitchObservation {
                lease_acquired_at,
                observed_at,
                min_switch_interval: self.min_switch_interval,
            }) {
            ProactiveSwitchOutcome::NoAction => {}
            ProactiveSwitchOutcome::Suppressed { .. } => {
                if !was_suppressed {
                    self.append_runtime_account_pool_event(RuntimeAccountPoolEvent {
                        occurred_at: observed_at,
                        pool_id: &pool_id,
                        account_id: Some(account_id.as_str()),
                        lease_id: Some(lease_id.as_str()),
                        event_type: AccountPoolEventType::ProactiveSwitchSuppressed,
                        reason_code: Some(AccountPoolReasonCode::MinimumSwitchInterval),
                        message: format!(
                            "proactive switch suppressed for {account_id} until the minimum switch interval elapses"
                        ),
                    })
                    .await?;
                }
            }
            ProactiveSwitchOutcome::RotateOnNextTurn => {
                if self.pending_rotation != Some(PendingRotation::HardFailure) {
                    self.pending_rotation = Some(PendingRotation::SoftProactive);
                }
            }
        }

        Ok(())
    }

    pub(crate) async fn report_usage_limit_reached(&mut self) -> anyhow::Result<()> {
        self.record_health_event(AccountHealthState::RateLimited, Utc::now())
            .await
    }

    pub(crate) async fn report_unauthorized(&mut self) -> anyhow::Result<()> {
        self.record_health_event(AccountHealthState::Unauthorized, Utc::now())
            .await
    }
    async fn release_active_lease(&mut self) -> anyhow::Result<()> {
        if let Some(lease) = self.active_lease.as_ref() {
            let _ = self
                .state_db
                .release_account_lease(&lease.record.lease_key(), Utc::now())
                .await?;
            self.clear_auth_marker(&lease.record).await?;
        }
        self.active_lease = None;
        self.next_health_event_sequence = 0;
        self.proactive_switch_state.reset();
        Ok(())
    }

    async fn record_health_event(
        &mut self,
        health_state: AccountHealthState,
        observed_at: chrono::DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let Some(active_lease) = self.active_lease.as_ref() else {
            return Ok(());
        };

        self.next_health_event_sequence += 1;
        self.state_db
            .record_account_health_event(AccountHealthEvent {
                account_id: active_lease.record.account_id.clone(),
                pool_id: active_lease.record.pool_id.clone(),
                health_state,
                sequence_number: self.next_health_event_sequence,
                observed_at,
            })
            .await?;
        self.proactive_switch_state.reset();
        self.pending_rotation = Some(PendingRotation::HardFailure);
        self.switch_reason = Some(AccountLeaseRuntimeReason::NonReplayableTurn);
        self.suppression_reason = None;
        Ok(())
    }

    async fn acquire_proactively_rotated_lease(
        &self,
        pool_id: &str,
        current_account_id: &str,
    ) -> Result<AccountLeaseRecord, AccountLeaseError> {
        let current_only = vec![current_account_id.to_string()];
        let mut excluded_account_ids = current_only.clone();
        if let Some(last_replaced_account_id) = self.last_proactively_replaced_account_id.as_ref()
            && last_replaced_account_id != current_account_id
        {
            excluded_account_ids.push(last_replaced_account_id.clone());
        }

        match self
            .state_db
            .acquire_account_lease_excluding(
                pool_id,
                &self.holder_instance_id,
                self.lease_ttl,
                &excluded_account_ids,
            )
            .await
        {
            Ok(lease) => Ok(lease),
            Err(AccountLeaseError::NoEligibleAccount) if excluded_account_ids.len() > 1 => {
                match self
                    .state_db
                    .acquire_account_lease_excluding(
                        pool_id,
                        &self.holder_instance_id,
                        self.lease_ttl,
                        &current_only,
                    )
                    .await
                {
                    Ok(lease) => Ok(lease),
                    Err(AccountLeaseError::NoEligibleAccount) => {
                        self.state_db
                            .acquire_account_lease(
                                pool_id,
                                &self.holder_instance_id,
                                self.lease_ttl,
                            )
                            .await
                    }
                    Err(err) => Err(err),
                }
            }
            Err(AccountLeaseError::NoEligibleAccount) => {
                self.state_db
                    .acquire_account_lease(pool_id, &self.holder_instance_id, self.lease_ttl)
                    .await
            }
            Err(err) => Err(err),
        }
    }

    async fn create_auth_session(
        &self,
        lease: &AccountLeaseRecord,
    ) -> anyhow::Result<Arc<dyn LeaseScopedAuthSession>> {
        let registered_account = self
            .state_db
            .read_registered_account(&lease.account_id)
            .await?
            .context("registered account missing for acquired lease")?;
        let auth_home = self.backend_private_auth_home(&registered_account.backend_account_handle);
        let binding = LeaseAuthBinding {
            account_id: lease.account_id.clone(),
            backend_account_handle: registered_account.backend_account_handle,
            lease_epoch: lease.lease_epoch as u64,
        };
        LocalLeaseScopedAuthSession::write_lease_epoch_marker(
            auth_home.as_path(),
            binding.lease_epoch,
        )?;
        Ok(Arc::new(LocalLeaseScopedAuthSession::new(
            binding, auth_home,
        )))
    }

    async fn clear_auth_marker(&self, lease: &AccountLeaseRecord) -> anyhow::Result<()> {
        let Some(registered_account) = self
            .state_db
            .read_registered_account(&lease.account_id)
            .await?
        else {
            return Ok(());
        };
        LocalLeaseScopedAuthSession::clear_lease_epoch_marker(
            self.backend_private_auth_home(&registered_account.backend_account_handle)
                .as_path(),
        )?;
        Ok(())
    }

    async fn append_runtime_account_pool_event(
        &self,
        event: RuntimeAccountPoolEvent<'_>,
    ) -> anyhow::Result<()> {
        self.state_db
            .append_account_pool_event(AccountPoolEventRecord {
                event_id: Uuid::new_v4().to_string(),
                occurred_at: event.occurred_at,
                pool_id: event.pool_id.to_string(),
                account_id: event.account_id.map(ToOwned::to_owned),
                lease_id: event.lease_id.map(ToOwned::to_owned),
                holder_instance_id: Some(self.holder_instance_id.clone()),
                event_type: serialized_protocol_enum_name(&event.event_type)?,
                reason_code: event
                    .reason_code
                    .as_ref()
                    .map(serialized_protocol_enum_name)
                    .transpose()?,
                message: event.message,
                details_json: None,
            })
            .await
    }

    fn backend_private_auth_home(&self, backend_account_handle: &str) -> PathBuf {
        self.codex_home
            .join(".pooled-auth/backends/local/accounts")
            .join(backend_account_handle)
    }
}

struct RuntimeAccountPoolEvent<'a> {
    occurred_at: DateTime<Utc>,
    pool_id: &'a str,
    account_id: Option<&'a str>,
    lease_id: Option<&'a str>,
    event_type: AccountPoolEventType,
    reason_code: Option<AccountPoolReasonCode>,
    message: String,
}

fn serialized_protocol_enum_name<T: serde::Serialize>(value: &T) -> anyhow::Result<String> {
    match serde_json::to_value(value)? {
        serde_json::Value::String(name) => Ok(name),
        other => Err(anyhow::anyhow!(
            "protocol enum serialized to a non-string value: {other:?}"
        )),
    }
}

impl AccountPoolManagerSnapshotSeed {
    pub(crate) async fn snapshot(self) -> AccountLeaseRuntimeSnapshot {
        let mut health_state = None;
        let mut next_eligible_at = None;
        let active_lease = if self.active_lease.is_some() {
            self.state_db
                .read_active_holder_lease(&self.holder_instance_id)
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        if let Some(diagnostic_lease) = active_lease.as_ref()
            && let Ok(diagnostic) = self
                .state_db
                .read_account_pool_diagnostic(
                    &diagnostic_lease.pool_id,
                    Some(&diagnostic_lease.account_id),
                )
                .await
        {
            next_eligible_at = diagnostic.next_eligible_at;
            if let Some(account_diagnostic) = diagnostic
                .accounts
                .into_iter()
                .find(|account| account.account_id == diagnostic_lease.account_id)
            {
                health_state = account_diagnostic.health_state;
                next_eligible_at = account_diagnostic.next_eligible_at.or(next_eligible_at);
            }
        }
        let active_lease = active_lease.as_ref();
        let proactive_switch_snapshot = active_lease.and(self.proactive_switch_snapshot.as_ref());
        AccountLeaseRuntimeSnapshot {
            active: active_lease.is_some(),
            suppressed: active_lease.is_none()
                && self.suppression_reason == Some(AccountLeaseRuntimeReason::StartupSuppressed),
            account_id: active_lease.map(|lease| lease.account_id.clone()),
            pool_id: active_lease.map(|lease| lease.pool_id.clone()),
            lease_id: active_lease.map(|lease| lease.lease_id.clone()),
            lease_epoch: active_lease.map(|lease| lease.lease_epoch),
            lease_acquired_at: active_lease.map(|lease| lease.acquired_at),
            health_state,
            switch_reason: self.switch_reason,
            suppression_reason: self.suppression_reason,
            transport_reset_generation: (self.transport_reset_generation != 0)
                .then_some(self.transport_reset_generation),
            last_remote_context_reset_turn_id: self.last_remote_context_reset_turn_id.clone(),
            min_switch_interval_secs: active_lease.map(|_| self.min_switch_interval_secs),
            proactive_switch_pending: proactive_switch_snapshot.map(|snapshot| snapshot.pending),
            proactive_switch_suppressed: proactive_switch_snapshot
                .map(|snapshot| snapshot.suppressed),
            proactive_switch_allowed_at: proactive_switch_snapshot
                .and_then(|snapshot| snapshot.allowed_at),
            next_eligible_at,
        }
    }
}
