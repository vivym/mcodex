use std::collections::HashMap;
use std::sync::Arc;

use crate::RolloutRecorder;
use crate::SkillsManager;
use crate::agent::AgentControl;
use crate::client::ModelClient;
use crate::config::StartedNetworkProxy;
use crate::exec_policy::ExecPolicyManager;
use crate::mcp::McpManager;
use crate::plugins::PluginsManager;
use crate::skills_watcher::SkillsWatcher;
use crate::tools::code_mode::CodeModeService;
use crate::tools::network_approval::NetworkApprovalService;
use crate::tools::sandboxing::ApprovalStore;
use crate::unified_exec::UnifiedExecProcessManager;
use anyhow::Context;
use chrono::Duration;
use chrono::Utc;
use codex_analytics::AnalyticsEventsClient;
use codex_config::types::AccountsConfigToml;
use codex_exec_server::Environment;
use codex_hooks::Hooks;
use codex_login::AuthManager;
use codex_mcp::McpConnectionManager;
use codex_models_manager::manager::ModelsManager;
use codex_otel::SessionTelemetry;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_rollout::state_db::StateDbHandle;
use codex_state::AccountHealthEvent;
use codex_state::AccountHealthState;
use codex_state::AccountLeaseError;
use codex_state::AccountLeaseRecord;
use codex_state::LeaseRenewal;
use std::path::PathBuf;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

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
    pub(crate) guardian_rejection_rationales: Mutex<HashMap<String, String>>,
    pub(crate) skills_manager: Arc<SkillsManager>,
    pub(crate) plugins_manager: Arc<PluginsManager>,
    pub(crate) mcp_manager: Arc<McpManager>,
    pub(crate) skills_watcher: Arc<SkillsWatcher>,
    pub(crate) agent_control: AgentControl,
    pub(crate) network_proxy: Option<StartedNetworkProxy>,
    pub(crate) network_approval: Arc<NetworkApprovalService>,
    pub(crate) state_db: Option<StateDbHandle>,
    pub(crate) account_pool_manager: Option<Arc<Mutex<AccountPoolManager>>>,
    /// Session-scoped model client shared across turns.
    pub(crate) model_client: ModelClient,
    pub(crate) code_mode_service: CodeModeService,
    pub(crate) environment: Option<Arc<Environment>>,
}

impl SessionServices {
    pub(crate) fn build_account_pool_manager(
        state_db: Option<StateDbHandle>,
        accounts: Option<AccountsConfigToml>,
        holder_instance_id: String,
    ) -> Option<Arc<Mutex<AccountPoolManager>>> {
        let state_db = state_db?;
        let accounts = accounts?;
        Some(Arc::new(Mutex::new(AccountPoolManager::new(
            state_db,
            accounts,
            holder_instance_id,
        ))))
    }
}

pub(crate) struct AccountPoolManager {
    state_db: StateDbHandle,
    default_pool_id: Option<String>,
    proactive_switch_threshold_percent: u8,
    allow_context_reuse_by_pool_id: HashMap<String, bool>,
    lease_ttl: Duration,
    holder_instance_id: String,
    active_lease: Option<AccountLeaseRecord>,
    next_health_event_sequence: i64,
    previous_turn_account_id: Option<String>,
    rotate_on_next_turn: bool,
}

impl AccountPoolManager {
    fn new(
        state_db: StateDbHandle,
        accounts: AccountsConfigToml,
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
        let lease_ttl_secs = accounts.lease_ttl_secs.unwrap_or(300);

        Self {
            state_db,
            default_pool_id,
            proactive_switch_threshold_percent,
            allow_context_reuse_by_pool_id,
            lease_ttl: Duration::seconds(lease_ttl_secs as i64),
            holder_instance_id,
            active_lease: None,
            next_health_event_sequence: 0,
            previous_turn_account_id: None,
            rotate_on_next_turn: false,
        }
    }

    pub(crate) async fn prepare_turn(&mut self) -> anyhow::Result<Option<(String, bool)>> {
        if self.rotate_on_next_turn {
            self.release_active_lease().await?;
            self.rotate_on_next_turn = false;
        }

        if let Some(active_lease) = self.active_lease.clone() {
            match self
                .state_db
                .renew_account_lease(&active_lease.lease_key(), Utc::now(), self.lease_ttl)
                .await?
            {
                LeaseRenewal::Renewed(record) => {
                    self.active_lease = Some(record);
                }
                LeaseRenewal::Missing => {
                    self.active_lease = None;
                    self.next_health_event_sequence = 0;
                }
            }
        }

        if self.active_lease.is_none() {
            let Some(pool_id) = self.resolve_pool_id().await? else {
                return Ok(None);
            };
            match self
                .state_db
                .acquire_account_lease(&pool_id, &self.holder_instance_id, self.lease_ttl)
                .await
            {
                Ok(lease) => {
                    self.next_health_event_sequence = self
                        .state_db
                        .read_account_health_event_sequence(&lease.account_id)
                        .await?
                        .unwrap_or(0);
                    self.active_lease = Some(lease);
                }
                Err(AccountLeaseError::NoEligibleAccount) => return Ok(None),
                Err(AccountLeaseError::Storage(message)) => return Err(anyhow::anyhow!(message)),
            }
        }

        let active_lease = self
            .active_lease
            .as_ref()
            .context("active lease missing after account pool acquisition")?;
        let account_id = active_lease.account_id.clone();
        let allow_context_reuse = self
            .allow_context_reuse_by_pool_id
            .get(&active_lease.pool_id)
            .copied()
            .unwrap_or(true);
        let reset_remote_context = self
            .previous_turn_account_id
            .as_deref()
            .is_some_and(|previous| previous != account_id)
            && !allow_context_reuse;
        self.previous_turn_account_id = Some(account_id.clone());
        Ok(Some((account_id, reset_remote_context)))
    }

    pub(crate) async fn report_rate_limits(
        &mut self,
        snapshot: &RateLimitSnapshot,
    ) -> anyhow::Result<()> {
        if snapshot.primary.as_ref().is_some_and(|window| {
            window.used_percent >= f64::from(self.proactive_switch_threshold_percent)
        }) {
            self.record_health_event(AccountHealthState::RateLimited, Utc::now())
                .await?;
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

    async fn resolve_pool_id(&self) -> anyhow::Result<Option<String>> {
        if self.default_pool_id.is_some() {
            return Ok(self.default_pool_id.clone());
        }

        Ok(self
            .state_db
            .read_account_startup_selection()
            .await?
            .default_pool_id)
    }

    async fn release_active_lease(&mut self) -> anyhow::Result<()> {
        if let Some(lease) = self.active_lease.as_ref() {
            let _ = self
                .state_db
                .release_account_lease(&lease.lease_key(), Utc::now())
                .await?;
        }
        self.active_lease = None;
        self.next_health_event_sequence = 0;
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
                account_id: active_lease.account_id.clone(),
                pool_id: active_lease.pool_id.clone(),
                health_state,
                sequence_number: self.next_health_event_sequence,
                observed_at,
            })
            .await?;
        self.rotate_on_next_turn = true;
        Ok(())
    }
}
