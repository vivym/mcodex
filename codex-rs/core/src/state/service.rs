use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use crate::RolloutRecorder;
use crate::SkillsManager;
use crate::agent::AgentControl;
use crate::agent_identity::AgentIdentityManager;
use crate::client::ModelClient;
use crate::config::StartedNetworkProxy;
use crate::exec_policy::ExecPolicyManager;
use crate::guardian::GuardianRejection;
use crate::mcp::McpManager;
use crate::plugins::PluginsManager;
use crate::runtime_lease::CollaborationTreeBinding;
use crate::runtime_lease::CollaborationTreeId;
use crate::skills_watcher::SkillsWatcher;
use crate::tools::code_mode::CodeModeService;
use crate::tools::network_approval::NetworkApprovalService;
use crate::tools::sandboxing::ApprovalStore;
use crate::unified_exec::UnifiedExecProcessManager;
use anyhow::Context;
use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use codex_account_pool::AccountKind;
use codex_account_pool::AccountRecord;
use codex_account_pool::LocalAccountPoolBackend;
use codex_account_pool::ProactiveSwitchObservation;
use codex_account_pool::ProactiveSwitchOutcome;
use codex_account_pool::ProactiveSwitchSnapshot;
use codex_account_pool::ProactiveSwitchState;
use codex_account_pool::QuotaFamilyView;
use codex_account_pool::SelectionAction;
use codex_account_pool::SelectionIntent;
use codex_account_pool::SelectionRequest;
use codex_account_pool::build_selection_plan;
use codex_account_pool::read_shared_startup_status;
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
use codex_state::AccountQuotaProbeBackoff;
use codex_state::AccountQuotaProbeObservation;
use codex_state::AccountQuotaProbeStillBlocked;
use codex_state::AccountQuotaStateRecord;
use codex_state::AccountStartupEligibility;
use codex_state::LeaseRenewal;
use codex_state::QuotaExhaustedWindows;
use codex_state::QuotaProbeResult;
use codex_thread_store::LocalThreadStore;
use serde_json::Value;
use serde_json::json;
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
    pub(crate) agent_identity_manager: Arc<AgentIdentityManager>,
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
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) runtime_lease_host: Option<crate::runtime_lease::RuntimeLeaseHost>,
    pub(crate) lease_auth: Arc<crate::lease_auth::SessionLeaseAuth>,
    pub(crate) thread_store: LocalThreadStore,
    /// Session-scoped model client shared across turns.
    pub(crate) model_client: ModelClient,
    pub(crate) code_mode_service: CodeModeService,
    pub(crate) environment: Option<Arc<Environment>>,
}

impl SessionServices {
    pub(crate) fn pooled_runtime_active(&self) -> bool {
        self.account_pool_manager.is_some()
            || self
                .runtime_lease_host
                .as_ref()
                .is_some_and(crate::runtime_lease::RuntimeLeaseHost::is_pooled)
    }

    pub(crate) async fn attach_runtime_lease_session(&self, session_id: &str) {
        if let Some(runtime_lease_host) = self.runtime_lease_host.as_ref()
            && runtime_lease_host.is_pooled()
        {
            runtime_lease_host.attach_session(session_id).await;
        }
    }

    pub(crate) async fn release_runtime_lease_session_for_shutdown(
        &self,
        session_id: &str,
    ) -> anyhow::Result<()> {
        if let Some(runtime_lease_host) = self.runtime_lease_host.as_ref()
            && runtime_lease_host.is_pooled()
        {
            runtime_lease_host
                .detach_session_with_retry(session_id)
                .await?;
            self.lease_auth.clear();
            return Ok(());
        }
        if let Some(account_pool_manager) = self.account_pool_manager.as_ref() {
            crate::runtime_lease::retry_shutdown_release(
                &format!("session {session_id} local account-pool manager"),
                || async {
                    let mut account_pool_manager = account_pool_manager.lock().await;
                    account_pool_manager.release_for_shutdown().await
                },
            )
            .await?;
            self.lease_auth.clear();
        }
        Ok(())
    }

    pub(crate) fn bind_collaboration_tree(
        &self,
        tree_id: CollaborationTreeId,
        member_id: String,
        cancellation_token: CancellationToken,
    ) -> CollaborationTreeBinding {
        if let Some(runtime_lease_host) = self
            .runtime_lease_host
            .as_ref()
            .filter(|host| host.is_pooled())
        {
            let membership = runtime_lease_host.register_collaboration_member(
                tree_id,
                member_id,
                cancellation_token,
            );
            return self.model_client.bind_collaboration_tree(membership);
        }
        self.model_client.bind_collaboration_tree_id(tree_id)
    }

    pub(crate) fn bind_synthetic_background_collaboration_tree(
        &self,
        member_id: String,
        cancellation_token: CancellationToken,
    ) -> CollaborationTreeBinding {
        if let Some(runtime_lease_host) = self
            .runtime_lease_host
            .as_ref()
            .filter(|host| host.is_pooled())
        {
            let tree_id = CollaborationTreeId::synthetic_background_tree_id(
                &runtime_lease_host.id(),
                Uuid::now_v7(),
            );
            let membership = runtime_lease_host.register_collaboration_member(
                tree_id,
                member_id,
                cancellation_token,
            );
            return self.model_client.bind_collaboration_tree(membership);
        }
        self.model_client.bind_collaboration_tree_id(
            CollaborationTreeId::synthetic_local_background_tree_id(Uuid::now_v7()),
        )
    }

    pub(crate) async fn build_root_runtime_lease_host(
        state_db: Option<StateDbHandle>,
        accounts: Option<AccountsConfigToml>,
        holder_instance_id: &str,
    ) -> anyhow::Result<Option<crate::runtime_lease::RuntimeLeaseHost>> {
        let Some(state_db) = state_db else {
            return Ok(None);
        };
        let shared_status = read_shared_startup_status(
            &LocalAccountPoolBackend::new(
                Arc::clone(&state_db),
                Duration::seconds(
                    accounts
                        .as_ref()
                        .and_then(|config| config.lease_ttl_secs)
                        .unwrap_or(300) as i64,
                ),
            ),
            accounts
                .as_ref()
                .and_then(|config| config.default_pool.as_deref()),
            None,
        )
        .await?;
        // Top-level sessions keep one runtime control plane even when pooled
        // mode can only activate after startup.
        if accounts.is_none() && !shared_status.pooled_applicable {
            return Ok(None);
        }
        Ok(Some(crate::runtime_lease::RuntimeLeaseHost::pooled(
            crate::runtime_lease::RuntimeLeaseHostId::new(holder_instance_id.to_string()),
        )))
    }

    pub(crate) async fn build_root_runtime_lease_authority(
        state_db: Option<StateDbHandle>,
        accounts: Option<AccountsConfigToml>,
        codex_home: PathBuf,
        holder_instance_id: String,
    ) -> anyhow::Result<Option<crate::runtime_lease::RuntimeLeaseAuthority>> {
        Ok(
            Self::build_account_pool_manager(state_db, accounts, codex_home, holder_instance_id)
                .await?
                .map(crate::runtime_lease::RuntimeLeaseAuthority::owned_manager),
        )
    }

    pub(crate) async fn build_account_pool_manager(
        state_db: Option<StateDbHandle>,
        accounts: Option<AccountsConfigToml>,
        codex_home: PathBuf,
        holder_instance_id: String,
    ) -> anyhow::Result<Option<Arc<Mutex<AccountPoolManager>>>> {
        let Some(state_db) = state_db else {
            return Ok(None);
        };
        let lease_ttl_secs = accounts
            .as_ref()
            .and_then(|accounts| accounts.lease_ttl_secs)
            .unwrap_or(300);
        let backend = LocalAccountPoolBackend::new(
            Arc::clone(&state_db),
            Duration::seconds(lease_ttl_secs as i64),
        );
        let shared_status = read_shared_startup_status(
            &backend,
            accounts
                .as_ref()
                .and_then(|accounts| accounts.default_pool.as_deref()),
            None,
        )
        .await?;
        // Keep the manager available whenever local account-pool config exists so
        // state-backed startup selection written before the first turn can still
        // activate pooled mode after startup.
        if accounts.is_none() && !shared_status.pooled_applicable {
            return Ok(None);
        }
        let accounts = accounts.unwrap_or(AccountsConfigToml {
            backend: None,
            default_pool: None,
            proactive_switch_threshold_percent: None,
            lease_ttl_secs: None,
            heartbeat_interval_secs: None,
            min_switch_interval_secs: None,
            allocation_mode: None,
            pools: None,
        });
        Ok(Some(Arc::new(Mutex::new(AccountPoolManager::new(
            state_db,
            accounts,
            codex_home,
            holder_instance_id,
        )))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_config::types::AccountPoolDefinitionToml;
    use codex_protocol::protocol::RateLimitWindow;
    use codex_state::AccountQuotaStateRecord;
    use codex_state::AccountRegistryEntryUpdate;
    use codex_state::AccountStartupSelectionUpdate;
    use codex_state::LegacyAccountImport;
    use codex_state::QuotaExhaustedWindows;
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;
    use tempfile::TempDir;

    #[tokio::test]
    async fn build_account_pool_manager_uses_state_only_persisted_selection() -> anyhow::Result<()>
    {
        let home = TempDir::new()?;
        let state_db =
            codex_state::StateRuntime::init(home.path().to_path_buf(), "mock_provider".to_string())
                .await?;
        state_db
            .import_legacy_default_account(LegacyAccountImport {
                account_id: "acct-state-only".to_string(),
            })
            .await?;

        let manager = SessionServices::build_account_pool_manager(
            Some(state_db),
            None,
            home.path().to_path_buf(),
            "holder-state-only".to_string(),
        )
        .await?;

        assert!(manager.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn build_account_pool_manager_keeps_local_accounts_config_available_without_immediate_pool()
    -> anyhow::Result<()> {
        let home = TempDir::new()?;
        let state_db =
            codex_state::StateRuntime::init(home.path().to_path_buf(), "mock_provider".to_string())
                .await?;
        let mut pools = HashMap::new();
        pools.insert(
            "pool-main".to_string(),
            AccountPoolDefinitionToml {
                allow_context_reuse: Some(false),
                account_kinds: None,
            },
        );

        let manager = SessionServices::build_account_pool_manager(
            Some(state_db),
            Some(AccountsConfigToml {
                backend: None,
                default_pool: None,
                proactive_switch_threshold_percent: None,
                lease_ttl_secs: None,
                heartbeat_interval_secs: None,
                min_switch_interval_secs: None,
                allocation_mode: None,
                pools: Some(pools),
            }),
            home.path().to_path_buf(),
            "holder-config-only".to_string(),
        )
        .await?;

        assert!(manager.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn build_root_runtime_lease_host_keeps_local_accounts_config_available_without_immediate_pool()
    -> anyhow::Result<()> {
        let home = TempDir::new()?;
        let state_db =
            codex_state::StateRuntime::init(home.path().to_path_buf(), "mock_provider".to_string())
                .await?;
        let mut pools = HashMap::new();
        pools.insert(
            "pool-main".to_string(),
            AccountPoolDefinitionToml {
                allow_context_reuse: Some(false),
                account_kinds: None,
            },
        );

        let runtime_lease_host = SessionServices::build_root_runtime_lease_host(
            Some(state_db),
            Some(AccountsConfigToml {
                backend: None,
                default_pool: None,
                proactive_switch_threshold_percent: None,
                lease_ttl_secs: None,
                heartbeat_interval_secs: None,
                min_switch_interval_secs: None,
                allocation_mode: None,
                pools: Some(pools),
            }),
            "holder-config-only",
        )
        .await?;

        assert!(runtime_lease_host.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn build_root_runtime_lease_host_uses_config_default_pool() -> anyhow::Result<()> {
        let home = TempDir::new()?;
        let state_db =
            codex_state::StateRuntime::init(home.path().to_path_buf(), "mock_provider".to_string())
                .await?;
        let mut pools = HashMap::new();
        pools.insert(
            "pool-main".to_string(),
            AccountPoolDefinitionToml {
                allow_context_reuse: Some(false),
                account_kinds: None,
            },
        );

        let runtime_lease_host = SessionServices::build_root_runtime_lease_host(
            Some(state_db),
            Some(AccountsConfigToml {
                backend: None,
                default_pool: Some("pool-main".to_string()),
                proactive_switch_threshold_percent: None,
                lease_ttl_secs: None,
                heartbeat_interval_secs: None,
                min_switch_interval_secs: None,
                allocation_mode: None,
                pools: Some(pools),
            }),
            "holder-config-default",
        )
        .await?;

        assert!(runtime_lease_host.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn prepare_turn_reports_unhealthy_startup_preferred_account() -> anyhow::Result<()> {
        let home = TempDir::new()?;
        let state_db =
            codex_state::StateRuntime::init(home.path().to_path_buf(), "mock_provider".to_string())
                .await?;
        state_db
            .upsert_account_registry_entry(AccountRegistryEntryUpdate {
                account_id: "acct-preferred".to_string(),
                pool_id: "pool-main".to_string(),
                position: 0,
                account_kind: "chatgpt".to_string(),
                backend_family: "local".to_string(),
                workspace_id: Some("workspace-main".to_string()),
                enabled: true,
                healthy: true,
            })
            .await?;
        state_db
            .write_account_startup_selection(AccountStartupSelectionUpdate {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-preferred".to_string()),
                suppressed: false,
            })
            .await?;
        let now = Utc::now();
        state_db
            .upsert_account_quota_state(AccountQuotaStateRecord {
                account_id: "acct-preferred".to_string(),
                limit_id: "codex".to_string(),
                primary_used_percent: Some(99.0),
                primary_resets_at: Some(now + Duration::seconds(60)),
                secondary_used_percent: None,
                secondary_resets_at: None,
                observed_at: now,
                exhausted_windows: QuotaExhaustedWindows::Primary,
                predicted_blocked_until: Some(now + Duration::seconds(60)),
                next_probe_after: Some(now + Duration::seconds(30)),
                probe_backoff_level: 0,
                last_probe_result: None,
            })
            .await?;

        let mut manager = AccountPoolManager::new(
            state_db,
            AccountsConfigToml {
                backend: None,
                default_pool: Some("pool-main".to_string()),
                proactive_switch_threshold_percent: None,
                lease_ttl_secs: None,
                heartbeat_interval_secs: None,
                min_switch_interval_secs: None,
                allocation_mode: None,
                pools: None,
            },
            home.path().to_path_buf(),
            "holder-startup-preferred".to_string(),
        );

        let selection = manager.prepare_turn().await?;

        assert!(selection.is_none());
        assert_eq!(
            manager.snapshot_seed().await.snapshot().await,
            AccountLeaseRuntimeSnapshot {
                active: false,
                suppressed: false,
                account_id: None,
                pool_id: None,
                lease_id: None,
                lease_epoch: None,
                runtime_generation: None,
                lease_acquired_at: None,
                health_state: None,
                switch_reason: None,
                suppression_reason: Some(AccountLeaseRuntimeReason::PreferredAccountUnhealthy),
                transport_reset_generation: None,
                last_remote_context_reset_turn_id: None,
                min_switch_interval_secs: None,
                proactive_switch_pending: None,
                proactive_switch_suppressed: None,
                proactive_switch_allowed_at: None,
                next_eligible_at: None,
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn report_usage_limit_reached_records_exhausted_quota_for_rotation() -> anyhow::Result<()>
    {
        let home = TempDir::new()?;
        let state_db =
            codex_state::StateRuntime::init(home.path().to_path_buf(), "mock_provider".to_string())
                .await?;
        state_db
            .import_legacy_default_account(LegacyAccountImport {
                account_id: "acct-primary".to_string(),
            })
            .await?;
        state_db
            .import_legacy_default_account(LegacyAccountImport {
                account_id: "acct-secondary".to_string(),
            })
            .await?;
        let mut manager = AccountPoolManager::new(
            Arc::clone(&state_db),
            AccountsConfigToml {
                backend: None,
                default_pool: Some("legacy-default".to_string()),
                proactive_switch_threshold_percent: None,
                lease_ttl_secs: None,
                heartbeat_interval_secs: None,
                min_switch_interval_secs: None,
                allocation_mode: None,
                pools: None,
            },
            home.path().to_path_buf(),
            "holder-usage-limit".to_string(),
        );

        let first_selection = manager
            .prepare_turn()
            .await?
            .expect("first turn should acquire primary account");
        assert_eq!(first_selection.account_id, "acct-primary");

        let reset_at = DateTime::<Utc>::from_timestamp(1_704_067_242, 0)
            .expect("fixed reset timestamp should be valid");
        let snapshot = RateLimitSnapshot {
            limit_id: Some("codex".to_string()),
            limit_name: None,
            primary: Some(RateLimitWindow {
                used_percent: 100.0,
                window_minutes: Some(15),
                resets_at: Some(reset_at.timestamp()),
            }),
            secondary: None,
            credits: None,
            plan_type: None,
            rate_limit_reached_type: None,
        };
        manager
            .report_usage_limit_reached(Some(&snapshot), Some(reset_at))
            .await?;

        let quota = state_db
            .read_account_quota_state("acct-primary", "codex")
            .await?
            .expect("usage-limit should persist exhausted quota state");
        assert_eq!(
            quota,
            AccountQuotaStateRecord {
                account_id: "acct-primary".to_string(),
                limit_id: "codex".to_string(),
                primary_used_percent: Some(100.0),
                primary_resets_at: Some(reset_at),
                secondary_used_percent: None,
                secondary_resets_at: None,
                observed_at: quota.observed_at,
                exhausted_windows: QuotaExhaustedWindows::Primary,
                predicted_blocked_until: Some(reset_at),
                next_probe_after: Some(reset_at),
                probe_backoff_level: 0,
                last_probe_result: None,
            }
        );

        let second_selection = manager
            .prepare_turn()
            .await?
            .expect("second turn should rotate to the next eligible account");
        assert_eq!(second_selection.account_id, "acct-secondary");
        Ok(())
    }

    #[tokio::test]
    async fn snapshot_seed_keeps_runtime_generation_bound_to_seed_lease_identity()
    -> anyhow::Result<()> {
        let home = TempDir::new()?;
        let state_db =
            codex_state::StateRuntime::init(home.path().to_path_buf(), "mock_provider".to_string())
                .await?;
        for (position, account_id) in ["acct-seed-a", "acct-seed-b"].into_iter().enumerate() {
            state_db
                .upsert_account_registry_entry(AccountRegistryEntryUpdate {
                    account_id: account_id.to_string(),
                    pool_id: "pool-main".to_string(),
                    position: position as i64,
                    account_kind: "chatgpt".to_string(),
                    backend_family: "local".to_string(),
                    workspace_id: Some("workspace-main".to_string()),
                    enabled: true,
                    healthy: true,
                })
                .await?;
        }
        let lease_a = state_db
            .acquire_account_lease("pool-main", "holder-seed-snapshot", Duration::seconds(300))
            .await?;
        let stale_seed = AccountPoolManagerSnapshotSeed {
            state_db: Arc::clone(&state_db),
            active_lease: Some(lease_a.clone()),
            min_switch_interval_secs: 0,
            proactive_switch_snapshot: None,
            switch_reason: None,
            suppression_reason: None,
            transport_reset_generation: 0,
            last_remote_context_reset_turn_id: None,
            active_lease_generation: 1,
        };

        state_db
            .release_account_lease(&lease_a.lease_key(), Utc::now())
            .await?;
        let lease_b = state_db
            .acquire_account_lease_excluding(
                "pool-main",
                "holder-seed-snapshot",
                Duration::seconds(300),
                std::slice::from_ref(&lease_a.account_id),
            )
            .await?;
        assert_eq!(lease_b.account_id, "acct-seed-b");

        let snapshot = stale_seed.snapshot().await;

        assert_eq!(snapshot.account_id.as_deref(), Some("acct-seed-a"));
        assert_eq!(snapshot.runtime_generation, Some(1));
        Ok(())
    }

    #[tokio::test]
    async fn snapshot_seed_preserves_cached_lease_when_holder_read_fails() -> anyhow::Result<()> {
        let home = TempDir::new()?;
        let state_db =
            codex_state::StateRuntime::init(home.path().to_path_buf(), "mock_provider".to_string())
                .await?;
        state_db
            .upsert_account_registry_entry(AccountRegistryEntryUpdate {
                account_id: "acct-seed-error".to_string(),
                pool_id: "pool-main".to_string(),
                position: 0,
                account_kind: "chatgpt".to_string(),
                backend_family: "local".to_string(),
                workspace_id: Some("workspace-main".to_string()),
                enabled: true,
                healthy: true,
            })
            .await?;
        let lease = state_db
            .acquire_account_lease("pool-main", "holder-seed-error", Duration::seconds(300))
            .await?;

        assert_eq!(
            validated_active_snapshot_lease(
                Some(&lease),
                Err(anyhow::anyhow!("injected holder read failure"))
            ),
            Some(lease)
        );
        Ok(())
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
    pub runtime_generation: Option<u64>,
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
    active_lease_generation: u64,
    active_lease_identity: Option<ActiveLeaseIdentity>,
}

pub(crate) struct AccountPoolManagerSnapshotSeed {
    state_db: StateDbHandle,
    active_lease: Option<AccountLeaseRecord>,
    min_switch_interval_secs: u64,
    proactive_switch_snapshot: Option<ProactiveSwitchSnapshot>,
    switch_reason: Option<AccountLeaseRuntimeReason>,
    suppression_reason: Option<AccountLeaseRuntimeReason>,
    transport_reset_generation: u64,
    last_remote_context_reset_turn_id: Option<String>,
    active_lease_generation: u64,
}

#[derive(Clone)]
struct ActiveAccountLease {
    record: AccountLeaseRecord,
    selection_family: String,
    auth_session: Arc<dyn LeaseScopedAuthSession>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActiveLeaseIdentity {
    lease_id: String,
    account_id: String,
    lease_epoch: i64,
}

pub(crate) struct TurnAccountSelection {
    pub(crate) pool_id: String,
    pub(crate) account_id: String,
    pub(crate) selection_family: String,
    pub(crate) generation: u64,
    pub(crate) allow_context_reuse: bool,
    pub(crate) reset_remote_context: bool,
    pub(crate) auth_session: Arc<dyn LeaseScopedAuthSession>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BridgedTurnPreview {
    ReuseCurrent(BridgedTurnSelectionPreview),
    Rotate(BridgedTurnSelectionPreview),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BridgedTurnSelectionPreview {
    pub(crate) pool_id: String,
    pub(crate) account_id: Option<String>,
    pub(crate) generation: u64,
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct SessionLeaseContinuity {
    previous_turn_account_id: Option<String>,
}

#[cfg(test)]
impl SessionLeaseContinuity {
    pub(crate) fn reset_remote_context_for_selection(
        &mut self,
        selection: &TurnAccountSelection,
    ) -> bool {
        let reset_remote_context = self
            .previous_turn_account_id
            .as_deref()
            .is_some_and(|previous| previous != selection.account_id)
            && !selection.allow_context_reuse;
        self.previous_turn_account_id = Some(selection.account_id.clone());
        reset_remote_context
    }
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
            active_lease_generation: 0,
            active_lease_identity: None,
        }
    }

    pub(crate) async fn preview_next_bridged_turn(
        &self,
    ) -> anyhow::Result<Option<BridgedTurnPreview>> {
        if self.pending_rotation.is_none()
            && let Some(active_lease) = self.active_lease.as_ref()
        {
            let current_holder_lease = self
                .state_db
                .read_active_holder_lease(&self.holder_instance_id)
                .await?;
            if let Some(current_holder_lease) = current_holder_lease {
                let current_identity = ActiveLeaseIdentity::from_record(&current_holder_lease);
                let cached_identity = ActiveLeaseIdentity::from_record(&active_lease.record);
                if current_identity == cached_identity {
                    let preview = BridgedTurnSelectionPreview {
                        pool_id: current_holder_lease.pool_id,
                        account_id: Some(current_holder_lease.account_id),
                        generation: if self.active_lease_identity.as_ref()
                            == Some(&current_identity)
                        {
                            self.active_lease_generation
                        } else {
                            self.active_lease_generation.saturating_add(1)
                        },
                    };
                    return Ok(Some(
                        if self.active_lease_identity.as_ref() == Some(&current_identity) {
                            BridgedTurnPreview::ReuseCurrent(preview)
                        } else {
                            BridgedTurnPreview::Rotate(preview)
                        },
                    ));
                }
            }
        }

        let now = Utc::now();
        if let Some(active_lease) = self.active_lease.as_ref() {
            let intent = match self.pending_rotation {
                Some(PendingRotation::HardFailure) => SelectionIntent::HardFailover,
                Some(PendingRotation::SoftProactive) => SelectionIntent::SoftRotation,
                None => SelectionIntent::Startup,
            };
            let mut request = SelectionRequest::for_intent(intent);
            request.now = Some(now);
            request.pool_id = Some(active_lease.record.pool_id.clone());
            request.selection_family = Some(active_lease.selection_family.clone());
            request.current_account_id = Some(active_lease.record.account_id.clone());
            request.just_replaced_account_id = self.last_proactively_replaced_account_id.clone();
            request.proactive_threshold_percent = self.proactive_switch_threshold_percent;
            let (_, plan) = self.build_runtime_selection_plan(&request).await?;
            let preview = match plan.terminal_action {
                SelectionAction::Select(account_id) | SelectionAction::Probe(account_id) => {
                    BridgedTurnPreview::Rotate(BridgedTurnSelectionPreview {
                        pool_id: active_lease.record.pool_id.clone(),
                        account_id: Some(account_id),
                        generation: self.active_lease_generation.saturating_add(1),
                    })
                }
                SelectionAction::StayOnCurrent => {
                    BridgedTurnPreview::ReuseCurrent(BridgedTurnSelectionPreview {
                        pool_id: active_lease.record.pool_id.clone(),
                        account_id: Some(active_lease.record.account_id.clone()),
                        generation: self.active_lease_generation,
                    })
                }
                SelectionAction::NoCandidate => return Ok(None),
            };
            return Ok(Some(preview));
        }

        let startup_preview = self
            .state_db
            .preview_account_startup_selection(self.default_pool_id.as_deref())
            .await?;
        let Some(pool_id) = startup_preview.effective_pool_id.clone() else {
            return Ok(None);
        };
        if matches!(
            startup_preview.eligibility,
            AccountStartupEligibility::Suppressed
                | AccountStartupEligibility::MissingPool
                | AccountStartupEligibility::PreferredAccountMissing
                | AccountStartupEligibility::PreferredAccountInOtherPool { .. }
                | AccountStartupEligibility::PreferredAccountDisabled
                | AccountStartupEligibility::PreferredAccountUnhealthy
                | AccountStartupEligibility::PreferredAccountBusy
                | AccountStartupEligibility::NoEligibleAccount
        ) {
            return Ok(None);
        }

        let mut request = SelectionRequest::for_intent(SelectionIntent::Startup);
        request.now = Some(now);
        request.pool_id = Some(pool_id.clone());
        if matches!(
            startup_preview.eligibility,
            AccountStartupEligibility::PreferredAccountSelected
        ) {
            request.preferred_account_id = startup_preview.predicted_account_id.clone();
        }
        request.proactive_threshold_percent = self.proactive_switch_threshold_percent;
        let (_, plan) = self.build_runtime_selection_plan(&request).await?;
        let account_id = match plan.terminal_action {
            SelectionAction::Select(account_id) | SelectionAction::Probe(account_id) => {
                Some(account_id)
            }
            SelectionAction::StayOnCurrent | SelectionAction::NoCandidate => None,
        };
        Ok(account_id.map(|account_id| {
            BridgedTurnPreview::Rotate(BridgedTurnSelectionPreview {
                pool_id,
                account_id: Some(account_id),
                generation: self.active_lease_generation.saturating_add(1),
            })
        }))
    }

    pub(crate) async fn prepare_turn(&mut self) -> anyhow::Result<Option<TurnAccountSelection>> {
        let now = Utc::now();
        let _ = self.proactive_switch_state.revalidate_before_turn(now);
        self.switch_reason = None;
        if let Some(active_lease) = self.active_lease.clone() {
            if let Some(pending_rotation) = self.pending_rotation.take() {
                let current_account_id = active_lease.record.account_id.clone();
                let current_pool_id = active_lease.record.pool_id.clone();
                let current_selection_family = active_lease.selection_family.clone();
                let mut request = SelectionRequest::for_intent(match pending_rotation {
                    PendingRotation::HardFailure => SelectionIntent::HardFailover,
                    PendingRotation::SoftProactive => SelectionIntent::SoftRotation,
                });
                request.now = Some(now);
                request.pool_id = Some(current_pool_id.clone());
                request.selection_family = Some(current_selection_family.clone());
                request.current_account_id = Some(current_account_id.clone());
                request.just_replaced_account_id =
                    self.last_proactively_replaced_account_id.clone();
                request.proactive_threshold_percent = self.proactive_switch_threshold_percent;
                match self
                    .acquire_selected_rotation_lease(request, active_lease)
                    .await
                {
                    Ok(next_lease) => {
                        if pending_rotation == PendingRotation::HardFailure {
                            self.last_proactively_replaced_account_id = None;
                            self.append_runtime_account_pool_event(RuntimeAccountPoolEvent {
                                occurred_at: next_lease.record.acquired_at,
                                pool_id: &current_pool_id,
                                account_id: Some(next_lease.record.account_id.as_str()),
                                lease_id: Some(next_lease.record.lease_id.as_str()),
                                event_type: AccountPoolEventType::LeaseAcquired,
                                reason_code: Some(AccountPoolReasonCode::NonReplayableTurn),
                                message: format!(
                                    "hard failover selected {} after non-replayable turn on {current_account_id}",
                                    next_lease.record.account_id
                                ),
                                details_json: Some(json!({
                                    "source": "rotation",
                                    "outcome": "selected",
                                    "intent": "hardFailover",
                                    "fromAccountId": current_account_id,
                                    "toAccountId": next_lease.record.account_id,
                                    "selectionFamily": next_lease.selection_family,
                                })),
                            })
                            .await?;
                        }
                        if pending_rotation == PendingRotation::SoftProactive
                            && next_lease.record.account_id != current_account_id
                        {
                            self.last_proactively_replaced_account_id =
                                Some(current_account_id.clone());
                            self.append_runtime_account_pool_event(RuntimeAccountPoolEvent {
                                occurred_at: next_lease.record.acquired_at,
                                pool_id: &current_pool_id,
                                account_id: Some(next_lease.record.account_id.as_str()),
                                lease_id: Some(next_lease.record.lease_id.as_str()),
                                event_type: AccountPoolEventType::ProactiveSwitchSelected,
                                reason_code: Some(AccountPoolReasonCode::QuotaNearExhausted),
                                message: format!(
                                    "proactive switch selected {} after quota pressure",
                                    next_lease.record.account_id
                                ),
                                details_json: Some(json!({
                                    "source": "rotation",
                                    "outcome": "selected",
                                    "intent": "softRotation",
                                    "fromAccountId": current_account_id,
                                    "toAccountId": next_lease.record.account_id,
                                    "selectionFamily": next_lease.selection_family,
                                })),
                            })
                            .await?;
                        } else if pending_rotation == PendingRotation::SoftProactive {
                            self.append_runtime_account_pool_event(RuntimeAccountPoolEvent {
                                occurred_at: now,
                                pool_id: &current_pool_id,
                                account_id: None,
                                lease_id: None,
                                event_type: AccountPoolEventType::LeaseAcquireFailed,
                                reason_code: Some(AccountPoolReasonCode::NoEligibleAccount),
                                message: format!(
                                    "proactive switch could not select an alternate eligible account for {current_account_id}"
                                ),
                                details_json: Some(json!({
                                    "source": "rotation",
                                    "outcome": "noEligibleAccount",
                                    "intent": "softRotation",
                                    "fromAccountId": current_account_id,
                                    "selectionFamily": current_selection_family,
                                })),
                            })
                            .await?;
                        }
                        self.adopt_active_lease(next_lease).await?;
                    }
                    Err(AccountLeaseError::NoEligibleAccount) => {
                        if pending_rotation == PendingRotation::HardFailure {
                            self.last_proactively_replaced_account_id = None;
                            self.append_runtime_account_pool_event(RuntimeAccountPoolEvent {
                                occurred_at: now,
                                pool_id: &current_pool_id,
                                account_id: None,
                                lease_id: None,
                                event_type: AccountPoolEventType::LeaseAcquireFailed,
                                reason_code: Some(AccountPoolReasonCode::NoEligibleAccount),
                                message: format!(
                                    "hard failover could not select an alternate eligible account for {current_account_id}"
                                ),
                                details_json: Some(json!({
                                    "source": "rotation",
                                    "outcome": "noEligibleAccount",
                                    "intent": "hardFailover",
                                    "fromAccountId": current_account_id,
                                    "selectionFamily": current_selection_family,
                                })),
                            })
                            .await?;
                        }
                        self.suppression_reason =
                            Some(AccountLeaseRuntimeReason::NoEligibleAccount);
                        return Ok(None);
                    }
                    Err(AccountLeaseError::Storage(message)) => {
                        return Err(anyhow::anyhow!(message));
                    }
                }
            } else {
                self.renew_active_lease().await?;
            }
        } else {
            self.renew_active_lease().await?;
        }
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
            if matches!(
                startup_preview.eligibility,
                AccountStartupEligibility::Suppressed
            ) {
                self.suppression_reason = Some(selection_reason);
                return Ok(None);
            }
            let mut request = SelectionRequest::for_intent(SelectionIntent::Startup);
            request.now = Some(now);
            request.pool_id = Some(pool_id);
            if matches!(
                startup_preview.eligibility,
                AccountStartupEligibility::PreferredAccountSelected
            ) {
                request.preferred_account_id = startup_preview.predicted_account_id.clone();
            }
            request.proactive_threshold_percent = self.proactive_switch_threshold_percent;
            match self.acquire_selected_lease(request).await {
                Ok(lease) => {
                    self.suppression_reason = None;
                    self.adopt_active_lease(lease).await?;
                }
                Err(AccountLeaseError::NoEligibleAccount) => {
                    self.suppression_reason = Some(match startup_preview.eligibility {
                        AccountStartupEligibility::PreferredAccountMissing
                        | AccountStartupEligibility::PreferredAccountInOtherPool { .. }
                        | AccountStartupEligibility::PreferredAccountDisabled
                        | AccountStartupEligibility::PreferredAccountUnhealthy
                        | AccountStartupEligibility::PreferredAccountBusy
                        | AccountStartupEligibility::NoEligibleAccount => selection_reason,
                        AccountStartupEligibility::Suppressed => {
                            AccountLeaseRuntimeReason::StartupSuppressed
                        }
                        AccountStartupEligibility::MissingPool
                        | AccountStartupEligibility::PreferredAccountSelected
                        | AccountStartupEligibility::AutomaticAccountSelected => {
                            AccountLeaseRuntimeReason::NoEligibleAccount
                        }
                    });
                    return Ok(None);
                }
                Err(AccountLeaseError::Storage(message)) => return Err(anyhow::anyhow!(message)),
            }
        }

        let (record, selection_family, auth_session) = {
            let active_lease = self
                .active_lease
                .as_ref()
                .context("active lease missing after account pool acquisition")?;
            (
                active_lease.record.clone(),
                active_lease.selection_family.clone(),
                Arc::clone(&active_lease.auth_session),
            )
        };
        let generation = self.observe_active_lease_generation(&record);
        let pool_id = record.pool_id.clone();
        let account_id = record.account_id;
        let allow_context_reuse = self
            .allow_context_reuse_by_pool_id
            .get(&pool_id)
            .copied()
            .unwrap_or(true);
        let reset_remote_context = self
            .previous_turn_account_id
            .as_deref()
            .is_some_and(|previous| previous != account_id)
            && !allow_context_reuse;
        self.previous_turn_account_id = Some(account_id.clone());
        Ok(Some(TurnAccountSelection {
            pool_id,
            account_id,
            selection_family,
            generation,
            allow_context_reuse,
            reset_remote_context,
            auth_session,
        }))
    }

    pub(crate) fn record_remote_context_reset(&mut self, turn_id: &str) {
        self.transport_reset_generation = self.transport_reset_generation.saturating_add(1);
        self.last_remote_context_reset_turn_id = Some(turn_id.to_string());
    }

    pub(crate) async fn snapshot_seed(&self) -> AccountPoolManagerSnapshotSeed {
        let active_lease = if let Some(active_lease) = self.active_lease.as_ref() {
            validated_active_snapshot_lease(
                Some(&active_lease.record),
                self.state_db
                    .read_active_holder_lease(&self.holder_instance_id)
                    .await,
            )
        } else {
            None
        };
        let proactive_switch_snapshot = active_lease.as_ref().map(|_| {
            let mut proactive_switch_state = self.proactive_switch_state.clone();
            proactive_switch_state.snapshot(Utc::now())
        });
        AccountPoolManagerSnapshotSeed {
            state_db: Arc::clone(&self.state_db),
            active_lease,
            min_switch_interval_secs: self.min_switch_interval.num_seconds().max(0) as u64,
            proactive_switch_snapshot,
            switch_reason: self.switch_reason,
            suppression_reason: self.suppression_reason,
            transport_reset_generation: self.transport_reset_generation,
            last_remote_context_reset_turn_id: self.last_remote_context_reset_turn_id.clone(),
            active_lease_generation: self.active_lease_generation,
        }
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
                        selection_family: active_lease.selection_family,
                        auth_session: active_lease.auth_session,
                    });
                }
                LeaseRenewal::Missing => {
                    self.clear_auth_marker(&active_lease.record).await?;
                    self.active_lease = None;
                    self.active_lease_identity = None;
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

    async fn observe_rate_limits_for_rotation(
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
                        details_json: None,
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

    pub(crate) async fn report_rate_limits(
        &mut self,
        snapshot: &RateLimitSnapshot,
    ) -> anyhow::Result<()> {
        let Some(active_lease) = self.active_lease.as_ref() else {
            return Ok(());
        };
        let account_id = active_lease.record.account_id.clone();
        let pool_id = active_lease.record.pool_id.clone();
        let selection_family = active_lease.selection_family.clone();
        self.record_live_quota_state_for_family(
            account_id.as_str(),
            pool_id.as_str(),
            selection_family.as_str(),
            snapshot,
        )
        .await?;
        self.observe_rate_limits_for_rotation(snapshot).await
    }

    pub(crate) async fn report_usage_limit_reached(
        &mut self,
        rate_limits: Option<&RateLimitSnapshot>,
        resets_at: Option<DateTime<Utc>>,
    ) -> anyhow::Result<()> {
        let observed_at = Utc::now();
        let active_account_id = self
            .active_lease
            .as_ref()
            .map(|lease| lease.record.account_id.clone());
        self.record_health_event(AccountHealthState::RateLimited, observed_at)
            .await?;
        if let Some(account_id) = active_account_id {
            self.record_usage_limit_quota_state(&account_id, rate_limits, resets_at, observed_at)
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn report_rate_limits_for_generation(
        &mut self,
        generation: u64,
        account_id: &str,
        pool_id: &str,
        selection_family: &str,
        snapshot: &RateLimitSnapshot,
    ) -> anyhow::Result<()> {
        if !self.generation_matches(generation, account_id, pool_id) {
            return Ok(());
        }
        self.record_live_quota_state_for_family(account_id, pool_id, selection_family, snapshot)
            .await?;
        self.observe_rate_limits_for_rotation(snapshot).await
    }

    pub(crate) async fn report_usage_limit_reached_for_generation(
        &mut self,
        generation: u64,
        account_id: &str,
        pool_id: &str,
        selection_family: &str,
    ) -> anyhow::Result<()> {
        if !self.generation_matches(generation, account_id, pool_id) {
            return Ok(());
        }
        self.record_ambiguous_usage_limit_quota_for_family(
            account_id,
            pool_id,
            selection_family,
            Utc::now(),
        )
        .await?;
        self.mark_non_replayable_turn_rotation();
        Ok(())
    }

    pub(crate) async fn report_unauthorized(&mut self) -> anyhow::Result<()> {
        self.record_health_event(AccountHealthState::Unauthorized, Utc::now())
            .await
    }

    pub(crate) async fn report_unauthorized_for_generation(
        &mut self,
        generation: u64,
        account_id: &str,
        pool_id: &str,
    ) -> anyhow::Result<bool> {
        if !self.generation_matches(generation, account_id, pool_id) {
            return Ok(false);
        }
        self.report_unauthorized().await?;
        Ok(true)
    }

    async fn release_active_lease(&mut self) -> anyhow::Result<()> {
        if let Some(lease) = self.active_lease.as_ref() {
            self.release_lease_record(&lease.record).await?;
        }
        self.active_lease = None;
        self.active_lease_identity = None;
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
        self.mark_non_replayable_turn_rotation();
        Ok(())
    }

    fn mark_non_replayable_turn_rotation(&mut self) {
        self.proactive_switch_state.reset();
        self.pending_rotation = Some(PendingRotation::HardFailure);
        self.switch_reason = Some(AccountLeaseRuntimeReason::NonReplayableTurn);
        self.suppression_reason = None;
    }

    async fn record_usage_limit_quota_state(
        &self,
        account_id: &str,
        rate_limits: Option<&RateLimitSnapshot>,
        resets_at: Option<DateTime<Utc>>,
        observed_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let to_datetime =
            |seconds| DateTime::<Utc>::from_timestamp(seconds, 0).filter(|_| seconds >= 0);
        let primary = rate_limits.and_then(|snapshot| snapshot.primary.as_ref());
        let secondary = rate_limits.and_then(|snapshot| snapshot.secondary.as_ref());
        let primary_resets_at = primary
            .and_then(|window| window.resets_at)
            .and_then(to_datetime);
        let secondary_resets_at = secondary
            .and_then(|window| window.resets_at)
            .and_then(to_datetime);
        let primary_exhausted = primary.is_some_and(|window| window.used_percent >= 100.0);
        let secondary_exhausted = secondary.is_some_and(|window| window.used_percent >= 100.0);
        let exhausted_windows = match (primary_exhausted, secondary_exhausted) {
            (true, true) => QuotaExhaustedWindows::Both,
            (true, false) => QuotaExhaustedWindows::Primary,
            (false, true) => QuotaExhaustedWindows::Secondary,
            (false, false) => QuotaExhaustedWindows::Unknown,
        };
        let window_reset = match exhausted_windows {
            QuotaExhaustedWindows::Primary => primary_resets_at,
            QuotaExhaustedWindows::Secondary => secondary_resets_at,
            QuotaExhaustedWindows::Both => match (primary_resets_at, secondary_resets_at) {
                (Some(primary), Some(secondary)) => Some(primary.max(secondary)),
                (Some(primary), None) => Some(primary),
                (None, Some(secondary)) => Some(secondary),
                (None, None) => None,
            },
            QuotaExhaustedWindows::Unknown => primary_resets_at.or(secondary_resets_at),
            QuotaExhaustedWindows::None => None,
        };
        let predicted_blocked_until = resets_at.or(window_reset);
        let limit_id = rate_limits
            .and_then(|snapshot| snapshot.limit_id.as_deref())
            .map(str::trim)
            .filter(|limit_id| !limit_id.is_empty())
            .unwrap_or("codex")
            .to_string();

        self.state_db
            .upsert_account_quota_state(AccountQuotaStateRecord {
                account_id: account_id.to_string(),
                limit_id,
                primary_used_percent: primary.map(|window| window.used_percent),
                primary_resets_at,
                secondary_used_percent: secondary.map(|window| window.used_percent),
                secondary_resets_at,
                observed_at,
                exhausted_windows,
                predicted_blocked_until,
                next_probe_after: predicted_blocked_until,
                probe_backoff_level: 0,
                last_probe_result: None,
            })
            .await
    }

    async fn build_runtime_selection_plan(
        &self,
        request: &SelectionRequest,
    ) -> anyhow::Result<(String, codex_account_pool::SelectionPlan)> {
        let pool_id = request
            .pool_id
            .as_deref()
            .context("runtime selection requires pool id")?;
        let selection_family = self.resolve_selection_family(request).await?;
        let mut selector = build_selection_plan(SelectionRequest {
            selection_family: Some(selection_family.clone()),
            ..request.clone()
        });
        for candidate in self
            .load_runtime_selection_candidates(
                pool_id,
                self.holder_instance_id.as_str(),
                selection_family.as_str(),
            )
            .await?
        {
            selector = selector.with_candidate(candidate);
        }

        Ok((selection_family, selector.run()))
    }

    async fn resolve_selection_family(&self, request: &SelectionRequest) -> anyhow::Result<String> {
        if let Some(selection_family) = request.selection_family.clone() {
            return Ok(normalized_selection_family(&selection_family));
        }
        if matches!(request.intent, SelectionIntent::Startup) {
            return Ok("codex".to_string());
        }
        if let Some(current_account_id) = request.current_account_id.as_deref()
            && let Some(active_lease) = self.active_lease.as_ref()
            && active_lease.record.account_id == current_account_id
        {
            return Ok(normalized_selection_family(
                active_lease.selection_family.as_str(),
            ));
        }

        Ok("codex".to_string())
    }

    async fn load_runtime_selection_candidates(
        &self,
        pool_id: &str,
        holder_instance_id: &str,
        selection_family: &str,
    ) -> anyhow::Result<Vec<AccountRecord>> {
        let rows = self
            .state_db
            .read_account_lease_selection_candidates(pool_id)
            .await?;
        let mut candidates = Vec::with_capacity(rows.len());
        for (registered_account, health_state, active_lease, position) in rows {
            let selection_quota = self
                .read_candidate_quota(registered_account.account_id.as_str(), selection_family)
                .await?;
            let codex_fallback = if selection_family == "codex" || selection_quota.is_some() {
                None
            } else {
                self.state_db
                    .read_account_quota_state(registered_account.account_id.as_str(), "codex")
                    .await?
            };
            candidates.push(AccountRecord {
                account_id: registered_account.account_id.clone(),
                healthy: registered_account.healthy,
                kind: account_kind(&registered_account.account_kind),
                enabled: registered_account.enabled,
                selector_auth_eligible: !matches!(
                    health_state,
                    Some(AccountHealthState::Unauthorized)
                ),
                pool_position: usize::try_from(position).unwrap_or_default(),
                leased_to_other_holder: active_lease
                    .as_ref()
                    .is_some_and(|lease| lease.holder_instance_id != holder_instance_id),
                quota: QuotaFamilyView {
                    selection: selection_quota,
                    codex_fallback,
                },
            });
        }

        Ok(candidates)
    }

    async fn read_candidate_quota(
        &self,
        account_id: &str,
        selection_family: &str,
    ) -> anyhow::Result<Option<AccountQuotaStateRecord>> {
        self.state_db
            .read_account_quota_state(account_id, selection_family)
            .await
    }

    async fn acquire_selected_lease(
        &mut self,
        request: SelectionRequest,
    ) -> std::result::Result<ActiveAccountLease, AccountLeaseError> {
        let pool_id = request.pool_id.as_deref().ok_or_else(|| {
            AccountLeaseError::Storage("runtime selection requires pool id".to_string())
        })?;
        loop {
            let (selection_family, plan) = self
                .build_runtime_selection_plan(&request)
                .await
                .map_err(|err| AccountLeaseError::Storage(err.to_string()))?;

            match plan.terminal_action {
                SelectionAction::Select(account_id) => match self
                    .state_db
                    .acquire_preferred_account_lease(
                        pool_id,
                        account_id.as_str(),
                        selection_family.as_str(),
                        &self.holder_instance_id,
                        self.lease_ttl,
                    )
                    .await
                {
                    Ok(lease) => {
                        let auth_session = self
                            .create_auth_session(&lease)
                            .await
                            .map_err(|err| AccountLeaseError::Storage(err.to_string()))?;
                        let active_selection_family = self
                            .active_selection_family(&lease, selection_family.as_str())
                            .await
                            .map_err(|err| AccountLeaseError::Storage(err.to_string()))?;
                        return Ok(ActiveAccountLease {
                            record: lease,
                            selection_family: active_selection_family,
                            auth_session,
                        });
                    }
                    Err(AccountLeaseError::NoEligibleAccount) => continue,
                    Err(err) => return Err(err),
                },
                SelectionAction::Probe(account_id) => {
                    let now = request.now.unwrap_or_else(Utc::now);
                    let Some(reservation) = self
                        .reserve_quota_probe(
                            pool_id,
                            account_id.as_str(),
                            selection_family.as_str(),
                            now,
                        )
                        .await
                        .map_err(|err| AccountLeaseError::Storage(err.to_string()))?
                    else {
                        continue;
                    };
                    let probe_holder_instance_id = self.probe_holder_instance_id();
                    let verification_lease = match self
                        .state_db
                        .acquire_quota_probe_account_lease(
                            pool_id,
                            account_id.as_str(),
                            reservation.limit_id.as_str(),
                            reservation.reserved_until,
                            probe_holder_instance_id.as_str(),
                            self.lease_ttl,
                        )
                        .await
                    {
                        Ok(lease) => lease,
                        Err(AccountLeaseError::NoEligibleAccount) => continue,
                        Err(err) => return Err(err),
                    };
                    let probe_result = self
                        .refresh_quota_probe(&verification_lease, &reservation)
                        .await
                        .map_err(|err| AccountLeaseError::Storage(err.to_string()));
                    let release_result = self
                        .release_lease_record(&verification_lease)
                        .await
                        .map_err(|err| AccountLeaseError::Storage(err.to_string()));
                    probe_result?;
                    release_result?;
                }
                SelectionAction::StayOnCurrent | SelectionAction::NoCandidate => {
                    return Err(AccountLeaseError::NoEligibleAccount);
                }
            }
        }
    }

    async fn acquire_selected_rotation_lease(
        &mut self,
        mut request: SelectionRequest,
        active_lease: ActiveAccountLease,
    ) -> std::result::Result<ActiveAccountLease, AccountLeaseError> {
        let keep_current_on_no_candidate = matches!(request.intent, SelectionIntent::SoftRotation);
        let current_account_id = active_lease.record.account_id.clone();
        let mut current_released = false;
        let mut current_reacquire_exhausted = false;
        let pool_id = request.pool_id.as_deref().ok_or_else(|| {
            AccountLeaseError::Storage("runtime selection requires pool id".to_string())
        })?;
        loop {
            let (selection_family, plan) = self
                .build_runtime_selection_plan(&request)
                .await
                .map_err(|err| AccountLeaseError::Storage(err.to_string()))?;

            match plan.terminal_action {
                SelectionAction::Select(account_id) => {
                    if !current_released {
                        self.release_active_lease()
                            .await
                            .map_err(|err| AccountLeaseError::Storage(err.to_string()))?;
                        current_released = true;
                    }
                    match self
                        .state_db
                        .acquire_preferred_account_lease(
                            pool_id,
                            account_id.as_str(),
                            selection_family.as_str(),
                            &self.holder_instance_id,
                            self.lease_ttl,
                        )
                        .await
                    {
                        Ok(lease) => {
                            let auth_session = self
                                .create_auth_session(&lease)
                                .await
                                .map_err(|err| AccountLeaseError::Storage(err.to_string()))?;
                            let active_selection_family = self
                                .active_selection_family(&lease, selection_family.as_str())
                                .await
                                .map_err(|err| AccountLeaseError::Storage(err.to_string()))?;
                            return Ok(ActiveAccountLease {
                                record: lease,
                                selection_family: active_selection_family,
                                auth_session,
                            });
                        }
                        Err(AccountLeaseError::NoEligibleAccount) => continue,
                        Err(err) => return Err(err),
                    }
                }
                SelectionAction::Probe(account_id) => {
                    let now = request.now.unwrap_or_else(Utc::now);
                    let Some(reservation) = self
                        .reserve_quota_probe(
                            pool_id,
                            account_id.as_str(),
                            selection_family.as_str(),
                            now,
                        )
                        .await
                        .map_err(|err| AccountLeaseError::Storage(err.to_string()))?
                    else {
                        continue;
                    };
                    let probe_holder_instance_id = self.probe_holder_instance_id();
                    let verification_lease = match self
                        .state_db
                        .acquire_quota_probe_account_lease(
                            pool_id,
                            account_id.as_str(),
                            reservation.limit_id.as_str(),
                            reservation.reserved_until,
                            probe_holder_instance_id.as_str(),
                            self.lease_ttl,
                        )
                        .await
                    {
                        Ok(lease) => lease,
                        Err(AccountLeaseError::NoEligibleAccount) => continue,
                        Err(err) => return Err(err),
                    };
                    let probe_result = self
                        .refresh_quota_probe(&verification_lease, &reservation)
                        .await
                        .map_err(|err| AccountLeaseError::Storage(err.to_string()));
                    let release_result = self
                        .release_lease_record(&verification_lease)
                        .await
                        .map_err(|err| AccountLeaseError::Storage(err.to_string()));
                    probe_result?;
                    release_result?;
                    continue;
                }
                SelectionAction::StayOnCurrent | SelectionAction::NoCandidate => {
                    if keep_current_on_no_candidate {
                        if !current_released {
                            return Ok(active_lease);
                        }
                        if current_reacquire_exhausted {
                            return Err(AccountLeaseError::NoEligibleAccount);
                        }
                        current_reacquire_exhausted = true;
                        match self
                            .state_db
                            .acquire_preferred_account_lease(
                                pool_id,
                                current_account_id.as_str(),
                                selection_family.as_str(),
                                &self.holder_instance_id,
                                self.lease_ttl,
                            )
                            .await
                        {
                            Ok(lease) => {
                                let auth_session = self
                                    .create_auth_session(&lease)
                                    .await
                                    .map_err(|err| AccountLeaseError::Storage(err.to_string()))?;
                                let active_selection_family = self
                                    .active_selection_family(&lease, selection_family.as_str())
                                    .await
                                    .map_err(|err| AccountLeaseError::Storage(err.to_string()))?;
                                return Ok(ActiveAccountLease {
                                    record: lease,
                                    selection_family: active_selection_family,
                                    auth_session,
                                });
                            }
                            Err(AccountLeaseError::NoEligibleAccount) => {
                                request.current_account_id = None;
                                continue;
                            }
                            Err(err) => return Err(err),
                        }
                    }
                    if !current_released {
                        self.release_active_lease()
                            .await
                            .map_err(|err| AccountLeaseError::Storage(err.to_string()))?;
                    }
                    return Err(AccountLeaseError::NoEligibleAccount);
                }
            }
        }
    }

    async fn adopt_active_lease(&mut self, lease: ActiveAccountLease) -> anyhow::Result<()> {
        self.next_health_event_sequence = self
            .state_db
            .read_account_health_event_sequence(&lease.record.account_id)
            .await?
            .unwrap_or(0);
        self.suppression_reason = None;
        if self
            .previous_turn_account_id
            .as_deref()
            .is_some_and(|previous| previous != lease.record.account_id)
            && self.switch_reason.is_none()
        {
            self.switch_reason = Some(AccountLeaseRuntimeReason::AutomaticAccountSelected);
        }
        self.active_lease = Some(lease);
        Ok(())
    }

    async fn reserve_quota_probe(
        &self,
        pool_id: &str,
        account_id: &str,
        selection_family: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Option<codex_account_pool::ProbeReservation>> {
        let Some(quota_state) = self
            .state_db
            .read_selection_quota_state(account_id, selection_family)
            .await?
        else {
            return Ok(None);
        };
        let reserved_until = now + self.probe_reservation_duration();
        let reserved_limit_id = quota_state.limit_id;
        let reserved = self
            .state_db
            .reserve_account_quota_probe(
                account_id,
                reserved_limit_id.as_str(),
                now,
                reserved_until,
            )
            .await?;
        if !reserved {
            self.append_runtime_account_pool_event(RuntimeAccountPoolEvent {
                occurred_at: now,
                pool_id,
                account_id: Some(account_id),
                lease_id: None,
                event_type: AccountPoolEventType::QuotaObserved,
                reason_code: None,
                message: format!(
                    "quota probe reservation skipped for {account_id} because another runtime reserved the slot"
                ),
                details_json: Some(json!({
                    "source": "probeReservation",
                    "outcome": "alreadyReserved",
                    "limitId": reserved_limit_id,
                    "selectionFamily": selection_family,
                })),
            })
            .await?;
            return Ok(None);
        }

        self.append_runtime_account_pool_event(RuntimeAccountPoolEvent {
            occurred_at: now,
            pool_id,
            account_id: Some(account_id),
            lease_id: None,
            event_type: AccountPoolEventType::QuotaObserved,
            reason_code: None,
            message: format!("quota probe reserved for {account_id}"),
            details_json: Some(json!({
                "source": "probeReservation",
                "outcome": "reserved",
                "limitId": reserved_limit_id,
                "selectionFamily": selection_family,
                "reservedUntil": reserved_until,
            })),
        })
        .await?;
        Ok(Some(codex_account_pool::ProbeReservation {
            limit_id: reserved_limit_id,
            reserved_until,
        }))
    }

    async fn refresh_quota_probe(
        &self,
        lease: &AccountLeaseRecord,
        reservation: &codex_account_pool::ProbeReservation,
    ) -> anyhow::Result<()> {
        let observed_at = Utc::now();
        let quota_state = self
            .state_db
            .read_account_quota_state(&lease.account_id, reservation.limit_id.as_str())
            .await?;
        let auth_session = self.create_auth_session(lease).await?;
        if auth_session.ensure_current().is_err() {
            if quota_state.is_some() {
                let backoff_until = observed_at + Duration::seconds(30);
                let _ = self
                    .state_db
                    .record_account_quota_probe_ambiguous(
                        &lease.account_id,
                        reservation.limit_id.as_str(),
                        AccountQuotaProbeObservation {
                            observed_at,
                            reserved_until: reservation.reserved_until,
                        },
                        AccountQuotaProbeBackoff {
                            predicted_blocked_until: backoff_until,
                            next_probe_after: backoff_until,
                        },
                    )
                    .await?;
            }
            self.append_runtime_account_pool_event(RuntimeAccountPoolEvent {
                occurred_at: observed_at,
                pool_id: lease.pool_id.as_str(),
                account_id: Some(lease.account_id.as_str()),
                lease_id: Some(lease.lease_id.as_str()),
                event_type: AccountPoolEventType::QuotaExhausted,
                reason_code: Some(AccountPoolReasonCode::Unknown),
                message: format!(
                    "quota probe for {} could not refresh auth state",
                    lease.account_id
                ),
                details_json: Some(json!({
                    "source": "probeRefresh",
                    "outcome": "ambiguous",
                    "limitId": reservation.limit_id,
                    "reservedUntil": reservation.reserved_until,
                })),
            })
            .await?;
            return Ok(());
        }

        let Some(quota_state) = quota_state else {
            return Ok(());
        };
        if !quota_state.exhausted_windows.is_exhausted() {
            let _ = self
                .state_db
                .record_account_quota_probe_success(
                    &lease.account_id,
                    quota_state.limit_id.as_str(),
                    AccountQuotaProbeObservation {
                        observed_at,
                        reserved_until: reservation.reserved_until,
                    },
                )
                .await?;
            self.append_runtime_account_pool_event(RuntimeAccountPoolEvent {
                occurred_at: observed_at,
                pool_id: lease.pool_id.as_str(),
                account_id: Some(lease.account_id.as_str()),
                lease_id: Some(lease.lease_id.as_str()),
                event_type: AccountPoolEventType::QuotaObserved,
                reason_code: None,
                message: format!("quota probe recovered {}", lease.account_id),
                details_json: Some(json!({
                    "source": "probeRefresh",
                    "outcome": "success",
                    "limitId": quota_state.limit_id,
                    "previousExhaustedWindows": quota_exhausted_windows_wire(
                        quota_state.exhausted_windows
                    ),
                    "reservedUntil": reservation.reserved_until,
                })),
            })
            .await?;
            return Ok(());
        }

        let next_probe_after = observed_at + Duration::seconds(30);
        let _ = self
            .state_db
            .record_account_quota_probe_still_blocked(
                &lease.account_id,
                quota_state.limit_id.as_str(),
                AccountQuotaProbeObservation {
                    observed_at,
                    reserved_until: reservation.reserved_until,
                },
                AccountQuotaProbeStillBlocked {
                    exhausted_windows: quota_state.exhausted_windows,
                    predicted_blocked_until: quota_state.predicted_blocked_until,
                    next_probe_after,
                },
            )
            .await?;
        self.append_runtime_account_pool_event(RuntimeAccountPoolEvent {
            occurred_at: observed_at,
            pool_id: lease.pool_id.as_str(),
            account_id: Some(lease.account_id.as_str()),
            lease_id: Some(lease.lease_id.as_str()),
            event_type: AccountPoolEventType::QuotaExhausted,
            reason_code: Some(AccountPoolReasonCode::QuotaExhausted),
            message: format!("quota probe still blocked {}", lease.account_id),
            details_json: Some(json!({
                    "source": "probeRefresh",
                    "outcome": "stillBlocked",
                    "limitId": quota_state.limit_id,
                    "exhaustedWindows": quota_exhausted_windows_wire(
                        quota_state.exhausted_windows
                    ),
                    "predictedBlockedUntil": quota_state.predicted_blocked_until,
                    "nextProbeAfter": next_probe_after,
                    "reservedUntil": reservation.reserved_until,
            })),
        })
        .await?;
        Ok(())
    }

    async fn record_live_quota_state_for_family(
        &self,
        account_id: &str,
        pool_id: &str,
        selection_family: &str,
        snapshot: &RateLimitSnapshot,
    ) -> anyhow::Result<()> {
        if snapshot.primary.is_none() && snapshot.secondary.is_none() {
            return Ok(());
        }
        let observed_at = Utc::now();
        let existing = self
            .state_db
            .read_account_quota_state(account_id, selection_family)
            .await?;
        let quota_state =
            quota_state_from_rate_limits(account_id, selection_family, snapshot, observed_at);
        self.state_db
            .upsert_account_quota_state(quota_state.clone())
            .await?;
        if existing
            .as_ref()
            .is_some_and(|row| !quota_state_changed_for_event(row, &quota_state))
        {
            return Ok(());
        }
        self.append_runtime_account_pool_event(RuntimeAccountPoolEvent {
            occurred_at: observed_at,
            pool_id,
            account_id: Some(account_id),
            lease_id: self
                .active_lease
                .as_ref()
                .filter(|lease| lease.record.account_id == account_id)
                .map(|lease| lease.record.lease_id.as_str()),
            event_type: quota_event_type_for_state(
                &quota_state,
                self.proactive_switch_threshold_percent,
            ),
            reason_code: quota_event_reason_for_state(
                &quota_state,
                self.proactive_switch_threshold_percent,
            ),
            message: format!(
                "quota observed for account {account_id} limit family {selection_family}"
            ),
            details_json: Some(quota_event_details(
                "liveRateLimit",
                &quota_state,
                existing.as_ref().map(|row| row.exhausted_windows),
            )),
        })
        .await
    }

    async fn record_ambiguous_usage_limit_quota_for_family(
        &self,
        account_id: &str,
        pool_id: &str,
        selection_family: &str,
        observed_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let existing = self
            .state_db
            .read_account_quota_state(account_id, selection_family)
            .await?;
        if existing
            .as_ref()
            .is_some_and(|row| row.exhausted_windows.is_exhausted())
        {
            return Ok(());
        }
        let predicted_blocked_until = existing
            .as_ref()
            .and_then(quota_reset_prediction_from_existing);
        let quota_state = AccountQuotaStateRecord {
            account_id: account_id.to_string(),
            limit_id: selection_family.to_string(),
            primary_used_percent: existing.as_ref().and_then(|row| row.primary_used_percent),
            primary_resets_at: existing.as_ref().and_then(|row| row.primary_resets_at),
            secondary_used_percent: existing.as_ref().and_then(|row| row.secondary_used_percent),
            secondary_resets_at: existing.as_ref().and_then(|row| row.secondary_resets_at),
            observed_at,
            exhausted_windows: QuotaExhaustedWindows::Unknown,
            predicted_blocked_until,
            next_probe_after: Some(observed_at + self.probe_reservation_duration()),
            probe_backoff_level: 0,
            last_probe_result: None,
        };
        self.state_db
            .upsert_account_quota_state(quota_state.clone())
            .await?;
        self.append_runtime_account_pool_event(RuntimeAccountPoolEvent {
            occurred_at: observed_at,
            pool_id,
            account_id: Some(account_id),
            lease_id: self
                .active_lease
                .as_ref()
                .filter(|lease| lease.record.account_id == account_id)
                .map(|lease| lease.record.lease_id.as_str()),
            event_type: AccountPoolEventType::QuotaExhausted,
            reason_code: Some(AccountPoolReasonCode::QuotaExhausted),
            message: format!(
                "usage limit reached for account {account_id} limit family {selection_family}"
            ),
            details_json: Some(quota_event_details(
                "usageLimit",
                &quota_state,
                existing.as_ref().map(|row| row.exhausted_windows),
            )),
        })
        .await
    }

    async fn release_lease_record(&self, lease: &AccountLeaseRecord) -> anyhow::Result<()> {
        let _ = self
            .state_db
            .release_account_lease(&lease.lease_key(), Utc::now())
            .await?;
        self.clear_auth_marker(lease).await
    }

    fn probe_holder_instance_id(&self) -> String {
        format!("{}:probe", self.holder_instance_id)
    }

    fn probe_reservation_duration(&self) -> Duration {
        Duration::seconds(30)
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

    async fn active_selection_family(
        &self,
        lease: &AccountLeaseRecord,
        requested_selection_family: &str,
    ) -> anyhow::Result<String> {
        Ok(self
            .state_db
            .read_registered_account(&lease.account_id)
            .await?
            .map(|account| normalized_selection_family(account.backend_family.as_str()))
            .filter(|family| !family.is_empty())
            .unwrap_or_else(|| normalized_selection_family(requested_selection_family)))
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
                details_json: event.details_json,
            })
            .await
    }

    fn generation_matches(&self, generation: u64, account_id: &str, pool_id: &str) -> bool {
        let Some(active_lease) = self.active_lease.as_ref() else {
            return false;
        };
        self.active_lease_generation == generation
            && active_lease.record.account_id == account_id
            && active_lease.record.pool_id == pool_id
    }

    fn observe_active_lease_generation(&mut self, lease: &AccountLeaseRecord) -> u64 {
        let identity = ActiveLeaseIdentity::from_record(lease);
        if self.active_lease_identity.as_ref() != Some(&identity) {
            self.active_lease_generation = self.active_lease_generation.saturating_add(1);
            self.active_lease_identity = Some(identity);
        }
        self.active_lease_generation
    }

    fn backend_private_auth_home(&self, backend_account_handle: &str) -> PathBuf {
        self.codex_home
            .join(".pooled-auth/backends/local/accounts")
            .join(backend_account_handle)
    }
}

impl ActiveLeaseIdentity {
    fn from_record(lease: &AccountLeaseRecord) -> Self {
        Self {
            lease_id: lease.lease_id.clone(),
            account_id: lease.account_id.clone(),
            lease_epoch: lease.lease_epoch,
        }
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
    details_json: Option<Value>,
}

fn normalized_selection_family(selection_family: &str) -> String {
    if selection_family.is_empty() {
        "codex".to_string()
    } else {
        selection_family.to_string()
    }
}

fn account_kind(account_kind: &str) -> AccountKind {
    if account_kind == "chatgpt" {
        AccountKind::ChatGpt
    } else {
        AccountKind::ManualOnly
    }
}

fn quota_state_from_rate_limits(
    account_id: &str,
    limit_id: &str,
    snapshot: &RateLimitSnapshot,
    observed_at: DateTime<Utc>,
) -> AccountQuotaStateRecord {
    let primary_resets_at = rate_limit_window_resets_at(snapshot.primary.as_ref());
    let secondary_resets_at = rate_limit_window_resets_at(snapshot.secondary.as_ref());
    let exhausted_windows = quota_exhausted_windows(snapshot);
    AccountQuotaStateRecord {
        account_id: account_id.to_string(),
        limit_id: limit_id.to_string(),
        primary_used_percent: snapshot.primary.as_ref().map(|window| window.used_percent),
        primary_resets_at,
        secondary_used_percent: snapshot
            .secondary
            .as_ref()
            .map(|window| window.used_percent),
        secondary_resets_at,
        observed_at,
        exhausted_windows,
        predicted_blocked_until: quota_reset_prediction(
            exhausted_windows,
            primary_resets_at,
            secondary_resets_at,
        ),
        next_probe_after: exhausted_windows
            .is_exhausted()
            .then_some(observed_at + Duration::seconds(30)),
        probe_backoff_level: 0,
        last_probe_result: None,
    }
}

fn quota_event_type_for_state(
    row: &AccountQuotaStateRecord,
    proactive_switch_threshold_percent: u8,
) -> AccountPoolEventType {
    if row.exhausted_windows.is_exhausted() {
        AccountPoolEventType::QuotaExhausted
    } else if quota_state_near_threshold(row, proactive_switch_threshold_percent) {
        AccountPoolEventType::QuotaNearExhausted
    } else {
        AccountPoolEventType::QuotaObserved
    }
}

fn quota_state_changed_for_event(
    previous: &AccountQuotaStateRecord,
    current: &AccountQuotaStateRecord,
) -> bool {
    previous.limit_id != current.limit_id
        || previous.primary_used_percent != current.primary_used_percent
        || previous.primary_resets_at != current.primary_resets_at
        || previous.secondary_used_percent != current.secondary_used_percent
        || previous.secondary_resets_at != current.secondary_resets_at
        || previous.exhausted_windows != current.exhausted_windows
        || previous.predicted_blocked_until != current.predicted_blocked_until
        || previous.next_probe_after != current.next_probe_after
        || previous.probe_backoff_level != current.probe_backoff_level
        || previous.last_probe_result != current.last_probe_result
}

fn quota_event_reason_for_state(
    row: &AccountQuotaStateRecord,
    proactive_switch_threshold_percent: u8,
) -> Option<AccountPoolReasonCode> {
    if row.exhausted_windows.is_exhausted() {
        Some(AccountPoolReasonCode::QuotaExhausted)
    } else if quota_state_near_threshold(row, proactive_switch_threshold_percent) {
        Some(AccountPoolReasonCode::QuotaNearExhausted)
    } else {
        None
    }
}

fn quota_state_near_threshold(
    row: &AccountQuotaStateRecord,
    proactive_switch_threshold_percent: u8,
) -> bool {
    let threshold = f64::from(proactive_switch_threshold_percent);
    row.primary_used_percent
        .is_some_and(|used| used >= threshold)
        || row
            .secondary_used_percent
            .is_some_and(|used| used >= threshold)
}

fn quota_event_details(
    source: &str,
    row: &AccountQuotaStateRecord,
    previous_exhausted_windows: Option<QuotaExhaustedWindows>,
) -> Value {
    json!({
        "source": source,
        "limitId": row.limit_id,
        "primaryUsedPercent": row.primary_used_percent,
        "primaryResetsAt": row.primary_resets_at,
        "secondaryUsedPercent": row.secondary_used_percent,
        "secondaryResetsAt": row.secondary_resets_at,
        "exhaustedWindows": quota_exhausted_windows_wire(row.exhausted_windows),
        "previousExhaustedWindows": previous_exhausted_windows.map(quota_exhausted_windows_wire),
        "predictedBlockedUntil": row.predicted_blocked_until,
        "nextProbeAfter": row.next_probe_after,
        "probeBackoffLevel": row.probe_backoff_level,
        "lastProbeResult": row.last_probe_result.map(quota_probe_result_wire),
    })
}

fn quota_exhausted_windows(snapshot: &RateLimitSnapshot) -> QuotaExhaustedWindows {
    let primary_exhausted = snapshot
        .primary
        .as_ref()
        .is_some_and(|window| window.used_percent >= 100.0);
    let secondary_exhausted = snapshot
        .secondary
        .as_ref()
        .is_some_and(|window| window.used_percent >= 100.0);
    match (primary_exhausted, secondary_exhausted) {
        (true, true) => QuotaExhaustedWindows::Both,
        (true, false) => QuotaExhaustedWindows::Primary,
        (false, true) => QuotaExhaustedWindows::Secondary,
        (false, false) => QuotaExhaustedWindows::None,
    }
}

fn quota_exhausted_windows_wire(value: QuotaExhaustedWindows) -> &'static str {
    match value {
        QuotaExhaustedWindows::None => "none",
        QuotaExhaustedWindows::Primary => "primary",
        QuotaExhaustedWindows::Secondary => "secondary",
        QuotaExhaustedWindows::Both => "both",
        QuotaExhaustedWindows::Unknown => "unknown",
    }
}

fn quota_probe_result_wire(value: QuotaProbeResult) -> &'static str {
    match value {
        QuotaProbeResult::Success => "success",
        QuotaProbeResult::StillBlocked => "stillBlocked",
        QuotaProbeResult::Ambiguous => "ambiguous",
    }
}

fn quota_reset_prediction_from_existing(row: &AccountQuotaStateRecord) -> Option<DateTime<Utc>> {
    quota_reset_prediction(
        row.exhausted_windows,
        row.primary_resets_at,
        row.secondary_resets_at,
    )
}

fn quota_reset_prediction(
    exhausted_windows: QuotaExhaustedWindows,
    primary_resets_at: Option<DateTime<Utc>>,
    secondary_resets_at: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match exhausted_windows {
        QuotaExhaustedWindows::None => None,
        QuotaExhaustedWindows::Primary => primary_resets_at,
        QuotaExhaustedWindows::Secondary => secondary_resets_at,
        QuotaExhaustedWindows::Both => match (primary_resets_at, secondary_resets_at) {
            (Some(primary), Some(secondary)) => Some(primary.max(secondary)),
            (Some(primary), None) => Some(primary),
            (None, Some(secondary)) => Some(secondary),
            (None, None) => None,
        },
        QuotaExhaustedWindows::Unknown => primary_resets_at.or(secondary_resets_at),
    }
}

fn validated_active_snapshot_lease(
    cached_lease: Option<&AccountLeaseRecord>,
    holder_read: anyhow::Result<Option<AccountLeaseRecord>>,
) -> Option<AccountLeaseRecord> {
    let cached_lease = cached_lease?;
    match holder_read {
        Ok(Some(current_lease)) if current_lease.lease_key() == cached_lease.lease_key() => {
            Some(cached_lease.clone())
        }
        Ok(_) => None,
        Err(err) => {
            tracing::warn!(
                "failed to revalidate active account lease snapshot; preserving cached lease: {err}"
            );
            Some(cached_lease.clone())
        }
    }
}

fn rate_limit_window_resets_at(
    window: Option<&codex_protocol::protocol::RateLimitWindow>,
) -> Option<DateTime<Utc>> {
    window
        .and_then(|window| window.resets_at)
        .and_then(|resets_at| DateTime::<Utc>::from_timestamp(resets_at, 0))
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
        let active_lease = self.active_lease.as_ref();
        if let Some(diagnostic_lease) = active_lease
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
        let proactive_switch_snapshot = active_lease.and(self.proactive_switch_snapshot.as_ref());
        AccountLeaseRuntimeSnapshot {
            active: active_lease.is_some(),
            suppressed: active_lease.is_none()
                && self.suppression_reason == Some(AccountLeaseRuntimeReason::StartupSuppressed),
            account_id: active_lease.map(|lease| lease.account_id.clone()),
            pool_id: active_lease.map(|lease| lease.pool_id.clone()),
            lease_id: active_lease.map(|lease| lease.lease_id.clone()),
            lease_epoch: active_lease.map(|lease| lease.lease_epoch),
            runtime_generation: active_lease.map(|_| self.active_lease_generation),
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
