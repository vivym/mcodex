use super::RuntimeLeaseHost;
use super::RuntimeLeaseHostId;
use super::RuntimeLeaseHostMode;
use crate::SkillsManager;
use crate::agent::AgentControl;
use crate::codex::Codex;
use crate::codex::CodexSpawnArgs;
use crate::config::Config;
use crate::config::ConfigBuilder;
use crate::mcp::McpManager;
use crate::plugins::PluginsManager;
use crate::skills_watcher::SkillsWatcher;
use codex_config::types::AccountPoolDefinitionToml;
use codex_config::types::AccountsConfigToml;
use codex_exec_server::EnvironmentManager;
use codex_login::CodexAuth;
use codex_models_manager::collaboration_mode_presets::CollaborationModesConfig;
use codex_models_manager::manager::ModelsManager;
use codex_protocol::ThreadId;
use codex_protocol::protocol::InitialHistory;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

#[test]
fn pooled_host_id_is_stable_for_one_runtime() {
    let host = RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new("runtime-a".to_string()));

    assert_eq!(host.id(), RuntimeLeaseHostId::new("runtime-a".to_string()));
    assert_eq!(host.mode(), RuntimeLeaseHostMode::Pooled);
}

#[test]
fn non_pooled_host_never_reports_pooled_authority() {
    let host =
        RuntimeLeaseHost::non_pooled_for_test(RuntimeLeaseHostId::new("runtime-a".to_string()));

    assert_eq!(host.mode(), RuntimeLeaseHostMode::NonPooled);
    assert!(host.authority_for_test().is_none());
}

#[test]
fn cloned_host_shares_one_runtime_handle() {
    let host = RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new("runtime-a".to_string()));
    let cloned = host.clone();

    assert!(host.ptr_eq_for_test(&cloned));
}

#[tokio::test]
async fn child_session_with_inherited_runtime_host_skips_session_local_account_pool_manager()
-> anyhow::Result<()> {
    let codex_home = tempfile::tempdir()?;
    let config = build_test_config_with_pool(codex_home.path()).await;
    let auth_manager =
        codex_login::AuthManager::from_auth_for_testing(CodexAuth::from_api_key("Test API Key"));
    let models_manager = Arc::new(ModelsManager::new(
        config.codex_home.to_path_buf(),
        auth_manager.clone(),
        /*model_catalog*/ None,
        CollaborationModesConfig::default(),
    ));
    let plugins_manager = Arc::new(PluginsManager::new(config.codex_home.to_path_buf()));
    let mcp_manager = Arc::new(McpManager::new(Arc::clone(&plugins_manager)));
    let skills_manager = Arc::new(SkillsManager::new(
        config.codex_home.clone(),
        /*bundled_skills_enabled*/ true,
    ));
    let skills_watcher = Arc::new(SkillsWatcher::noop());
    let runtime_lease_host =
        RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new("runtime-a".to_string()));

    let root = Codex::spawn(CodexSpawnArgs {
        config: config.clone(),
        auth_manager: auth_manager.clone(),
        models_manager: Arc::clone(&models_manager),
        environment_manager: Arc::new(EnvironmentManager::new(/*exec_server_url*/ None)),
        skills_manager: Arc::clone(&skills_manager),
        plugins_manager: Arc::clone(&plugins_manager),
        mcp_manager: Arc::clone(&mcp_manager),
        skills_watcher: Arc::clone(&skills_watcher),
        conversation_history: InitialHistory::New,
        session_source: SessionSource::Exec,
        agent_control: AgentControl::default(),
        dynamic_tools: Vec::new(),
        persist_extended_history: false,
        metrics_service_name: None,
        inherited_shell_snapshot: None,
        inherited_exec_policy: Some(Arc::new(crate::exec_policy::ExecPolicyManager::default())),
        inherited_lease_auth_session: None,
        runtime_lease_host: Some(runtime_lease_host.clone()),
        user_shell_override: None,
        parent_trace: None,
        analytics_events_client: None,
    })
    .await?
    .codex;
    let child = Codex::spawn(CodexSpawnArgs {
        config,
        auth_manager,
        models_manager,
        environment_manager: Arc::new(EnvironmentManager::new(/*exec_server_url*/ None)),
        skills_manager,
        plugins_manager,
        mcp_manager,
        skills_watcher,
        conversation_history: InitialHistory::New,
        session_source: SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
            parent_thread_id: ThreadId::default(),
            depth: 1,
            agent_path: None,
            agent_nickname: None,
            agent_role: None,
        }),
        agent_control: AgentControl::default(),
        dynamic_tools: Vec::new(),
        persist_extended_history: false,
        metrics_service_name: None,
        inherited_shell_snapshot: None,
        inherited_exec_policy: Some(Arc::new(crate::exec_policy::ExecPolicyManager::default())),
        inherited_lease_auth_session: None,
        runtime_lease_host: Some(runtime_lease_host),
        user_shell_override: None,
        parent_trace: None,
        analytics_events_client: None,
    })
    .await?
    .codex;

    assert!(root.session.services.runtime_lease_host.is_some());
    assert!(root.session.services.account_pool_manager.is_some());
    assert!(
        root.session
            .services
            .runtime_lease_host
            .as_ref()
            .expect("root runtime lease host")
            .has_legacy_manager_bridge_for_test()
    );
    assert!(child.session.services.runtime_lease_host.is_some());
    assert!(child.session.services.account_pool_manager.is_none());

    child.shutdown_and_wait().await?;
    root.shutdown_and_wait().await?;
    Ok(())
}

async fn build_test_config_with_pool(codex_home: &Path) -> Config {
    let mut config = ConfigBuilder::without_managed_config_for_tests()
        .codex_home(codex_home.to_path_buf())
        .build()
        .await
        .expect("load default test config");
    let mut pools = HashMap::new();
    pools.insert(
        "pool-main".to_string(),
        AccountPoolDefinitionToml {
            allow_context_reuse: Some(false),
            account_kinds: None,
        },
    );
    config.accounts = Some(AccountsConfigToml {
        backend: None,
        default_pool: None,
        proactive_switch_threshold_percent: None,
        lease_ttl_secs: None,
        heartbeat_interval_secs: None,
        min_switch_interval_secs: None,
        allocation_mode: None,
        pools: Some(pools),
    });
    config
}
