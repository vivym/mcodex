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
use base64::Engine;
use chrono::Utc;
use codex_app_server_protocol::AuthMode;
use codex_config::types::AccountPoolDefinitionToml;
use codex_config::types::AccountsConfigToml;
use codex_config::types::McpServerConfig;
use codex_config::types::McpServerTransportConfig;
use codex_exec_server::EnvironmentManager;
use codex_login::AuthCredentialsStoreMode;
use codex_login::AuthDotJson;
use codex_login::CodexAuth;
use codex_login::TokenData;
use codex_login::save_auth;
use codex_models_manager::collaboration_mode_presets::CollaborationModesConfig;
use codex_models_manager::manager::ModelsManager;
use codex_protocol::ThreadId;
use codex_protocol::protocol::InitialHistory;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_state::AccountRegistryEntryUpdate;
use codex_state::AccountStartupSelectionUpdate;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

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
    let config_codex_home = config.codex_home.clone();
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
    assert!(Arc::ptr_eq(
        root.session
            .services
            .account_pool_manager
            .as_ref()
            .expect("root account pool manager"),
        &root
            .session
            .services
            .runtime_lease_host
            .as_ref()
            .expect("root runtime lease host")
            .legacy_manager_bridge_for_test()
            .expect("registered legacy manager bridge"),
    ));
    assert!(child.session.services.runtime_lease_host.is_some());
    assert!(child.session.services.account_pool_manager.is_none());
    assert!(
        child.session.take_session_startup_prewarm().await.is_none(),
        "pooled-host children must not prewarm before lease-backed auth is selected"
    );

    let account_id = "acct-pooled-child";
    let state_db = root
        .session
        .services
        .state_db
        .as_ref()
        .expect("root state db");
    state_db
        .upsert_account_registry_entry(AccountRegistryEntryUpdate {
            account_id: account_id.to_string(),
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
            preferred_account_id: Some(account_id.to_string()),
            suppressed: false,
        })
        .await?;
    save_auth(
        &pooled_auth_home(config_codex_home.as_path(), account_id),
        &auth_dot_json_for_account(account_id),
        AuthCredentialsStoreMode::File,
    )?;

    assert!(
        child
            .session
            .services
            .auth_manager
            .auth()
            .await
            .is_some_and(|auth| auth.is_api_key_auth()),
        "child fallback AuthManager should remain non-pooled"
    );
    let child_turn_manager = child
        .session
        .services
        .account_pool_manager_for_turn()
        .expect("pooled child should use root bridge manager for turns");
    let root_bridge_manager = root
        .session
        .services
        .runtime_lease_host
        .as_ref()
        .expect("root runtime lease host")
        .legacy_manager_bridge()
        .expect("registered root legacy bridge manager");
    assert!(Arc::ptr_eq(&child_turn_manager, &root_bridge_manager));
    let selection = {
        let mut manager = child_turn_manager.lock().await;
        manager.prepare_turn().await?
    }
    .expect("bridge manager should select a pooled account");
    let leased_auth = selection.auth_session.leased_turn_auth()?;
    assert_eq!(selection.account_id, account_id);
    assert_eq!(
        leased_auth.auth().get_account_id().as_deref(),
        Some(account_id)
    );
    child
        .session
        .services
        .lease_auth
        .replace_current(Some(Arc::clone(&selection.auth_session)));
    let child_model_auth = child
        .session
        .services
        .model_client
        .new_session()
        .current_auth_provider_for_test()
        .await?;
    assert_eq!(child_model_auth.account_id.as_deref(), Some(account_id));
    assert_eq!(
        child_model_auth.token.as_deref(),
        Some(fake_access_token(account_id).as_str())
    );

    child.shutdown_and_wait().await?;
    root.shutdown_and_wait().await?;
    Ok(())
}

#[tokio::test]
async fn pooled_host_child_keeps_bridge_manager_until_last_session_shutdown() -> anyhow::Result<()>
{
    let codex_home = tempfile::tempdir()?;
    let config = build_test_config_with_pool(codex_home.path()).await;
    let config_codex_home = config.codex_home.clone();
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
        RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new("runtime-b".to_string()));

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
        config: config.clone(),
        auth_manager: auth_manager.clone(),
        models_manager: Arc::clone(&models_manager),
        environment_manager: Arc::new(EnvironmentManager::new(/*exec_server_url*/ None)),
        skills_manager: Arc::clone(&skills_manager),
        plugins_manager: Arc::clone(&plugins_manager),
        mcp_manager: Arc::clone(&mcp_manager),
        skills_watcher: Arc::clone(&skills_watcher),
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

    let account_id = "acct-pooled-lifecycle";
    let state_db = root
        .session
        .services
        .state_db
        .as_ref()
        .expect("root state db");
    state_db
        .upsert_account_registry_entry(AccountRegistryEntryUpdate {
            account_id: account_id.to_string(),
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
            preferred_account_id: Some(account_id.to_string()),
            suppressed: false,
        })
        .await?;
    save_auth(
        &pooled_auth_home(config_codex_home.as_path(), account_id),
        &auth_dot_json_for_account(account_id),
        AuthCredentialsStoreMode::File,
    )?;

    let child_turn_manager = child
        .session
        .services
        .account_pool_manager_for_turn()
        .expect("pooled child should resolve the bridge manager");
    {
        let mut manager = child_turn_manager.lock().await;
        let selection = manager
            .prepare_turn()
            .await?
            .expect("pooled child should acquire a bridge-backed lease");
        assert_eq!(selection.account_id, account_id);
    }

    let turn_cancellation = CancellationToken::new();
    let _child_heartbeat = crate::codex::start_account_pool_lease_heartbeat(
        &child.session,
        /*lease_selected_for_turn*/ true,
        &turn_cancellation,
    )
    .await
    .expect("pooled child should start a heartbeat from the bridge manager");

    let snapshot_before_parent_shutdown = child
        .account_lease_snapshot()
        .await
        .expect("child should expose bridge-backed lease snapshot before parent shutdown");
    assert!(snapshot_before_parent_shutdown.active);
    assert_eq!(
        snapshot_before_parent_shutdown.account_id.as_deref(),
        Some(account_id)
    );

    root.shutdown_and_wait().await?;

    let snapshot_after_parent_shutdown = child
        .account_lease_snapshot()
        .await
        .expect("child should keep the bridge-backed lease after parent shutdown");
    assert!(snapshot_after_parent_shutdown.active);
    assert_eq!(
        snapshot_after_parent_shutdown.account_id.as_deref(),
        Some(account_id)
    );

    child.shutdown_and_wait().await?;

    let contender = Codex::spawn(CodexSpawnArgs {
        config,
        auth_manager,
        models_manager,
        environment_manager: Arc::new(EnvironmentManager::new(/*exec_server_url*/ None)),
        skills_manager,
        plugins_manager,
        mcp_manager,
        skills_watcher,
        conversation_history: InitialHistory::New,
        session_source: SessionSource::Exec,
        agent_control: AgentControl::default(),
        dynamic_tools: Vec::new(),
        persist_extended_history: false,
        metrics_service_name: None,
        inherited_shell_snapshot: None,
        inherited_exec_policy: Some(Arc::new(crate::exec_policy::ExecPolicyManager::default())),
        inherited_lease_auth_session: None,
        runtime_lease_host: Some(RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new(
            "runtime-c".to_string(),
        ))),
        user_shell_override: None,
        parent_trace: None,
        analytics_events_client: None,
    })
    .await?
    .codex;
    let contender_turn_manager = contender
        .session
        .services
        .account_pool_manager_for_turn()
        .expect("contender root should build a local pooled manager");
    let contender_selection = {
        let mut manager = contender_turn_manager.lock().await;
        manager.prepare_turn().await?
    }
    .expect("last pooled child shutdown should release the bridge lease for immediate reuse");
    assert_eq!(contender_selection.account_id, account_id);

    contender.shutdown_and_wait().await?;
    Ok(())
}

#[tokio::test]
async fn failed_startup_does_not_leak_runtime_host_attachment() -> anyhow::Result<()> {
    let codex_home = tempfile::tempdir()?;
    let mut config = build_test_config_with_pool(codex_home.path()).await;
    let config_codex_home = config.codex_home.clone();
    let auth_manager =
        codex_login::AuthManager::from_auth_for_testing(CodexAuth::from_api_key("Test API Key"));
    let plugins_manager = Arc::new(PluginsManager::new(config_codex_home.to_path_buf()));
    let mut mcp_servers = config.mcp_servers.get().clone();
    mcp_servers.insert(
        "required-bad-server".to_string(),
        McpServerConfig {
            transport: McpServerTransportConfig::Stdio {
                command: "definitely-not-a-real-command".to_string(),
                args: Vec::new(),
                env: None,
                env_vars: Vec::new(),
                cwd: None,
            },
            enabled: true,
            required: true,
            supports_parallel_tool_calls: false,
            disabled_reason: None,
            startup_timeout_sec: Some(Duration::from_secs(1)),
            tool_timeout_sec: None,
            enabled_tools: None,
            disabled_tools: None,
            scopes: None,
            oauth_resource: None,
            tools: HashMap::new(),
        },
    );
    config
        .mcp_servers
        .set(mcp_servers)
        .expect("test mcp config should accept required server");
    let runtime_lease_host =
        RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new("runtime-fail".to_string()));

    let spawn_result = Codex::spawn(CodexSpawnArgs {
        config,
        auth_manager: auth_manager.clone(),
        models_manager: Arc::new(ModelsManager::new(
            config_codex_home.to_path_buf(),
            auth_manager,
            /*model_catalog*/ None,
            CollaborationModesConfig::default(),
        )),
        environment_manager: Arc::new(EnvironmentManager::new(/*exec_server_url*/ None)),
        skills_manager: Arc::new(SkillsManager::new(
            config_codex_home,
            /*bundled_skills_enabled*/ true,
        )),
        plugins_manager: Arc::clone(&plugins_manager),
        mcp_manager: Arc::new(McpManager::new(plugins_manager)),
        skills_watcher: Arc::new(SkillsWatcher::noop()),
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
    .await;

    let err = match spawn_result {
        Ok(_) => panic!("required MCP startup should fail"),
        Err(err) => err,
    };
    assert!(
        err.to_string()
            .contains("required MCP servers failed to initialize")
    );
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    Ok(())
}

#[tokio::test]
async fn root_with_config_only_pool_installs_runtime_host_for_future_threadspawn_children()
-> anyhow::Result<()> {
    let codex_home = tempfile::tempdir()?;
    let config = build_test_config_with_pool_without_default(codex_home.path()).await;
    let config_codex_home = config.codex_home.clone();
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
        runtime_lease_host: None,
        user_shell_override: None,
        parent_trace: None,
        analytics_events_client: None,
    })
    .await?
    .codex;
    let root_runtime_lease_host = root
        .session
        .services
        .runtime_lease_host
        .as_ref()
        .expect("config-only pooled roots should still install a runtime host")
        .clone();
    assert!(
        root_runtime_lease_host.has_legacy_manager_bridge_for_test(),
        "top-level root should bridge its local manager into the runtime host"
    );

    let account_id = "acct-config-only-root";
    let state_db = root
        .session
        .services
        .state_db
        .as_ref()
        .expect("root state db");
    state_db
        .upsert_account_registry_entry(AccountRegistryEntryUpdate {
            account_id: account_id.to_string(),
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
            preferred_account_id: Some(account_id.to_string()),
            suppressed: false,
        })
        .await?;
    save_auth(
        &pooled_auth_home(config_codex_home.as_path(), account_id),
        &auth_dot_json_for_account(account_id),
        AuthCredentialsStoreMode::File,
    )?;

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
        runtime_lease_host: Some(root_runtime_lease_host.clone()),
        user_shell_override: None,
        parent_trace: None,
        analytics_events_client: None,
    })
    .await?
    .codex;

    assert!(child.session.services.account_pool_manager.is_none());
    let child_turn_manager = child
        .session
        .services
        .account_pool_manager_for_turn()
        .expect("threadspawn child should resolve the root bridge manager");
    let root_bridge_manager = root_runtime_lease_host
        .legacy_manager_bridge_for_test()
        .expect("root runtime host should expose its bridged manager");
    assert!(Arc::ptr_eq(&child_turn_manager, &root_bridge_manager));
    let selection = {
        let mut manager = child_turn_manager.lock().await;
        manager.prepare_turn().await?
    }
    .expect("child should use the bridged root manager for future pooled activation");
    assert_eq!(selection.account_id, account_id);

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

async fn build_test_config_with_pool_without_default(codex_home: &Path) -> Config {
    let mut config = build_test_config_with_pool(codex_home).await;
    if let Some(accounts) = config.accounts.as_mut() {
        accounts.default_pool = None;
    }
    config
}

fn pooled_auth_home(codex_home: &Path, account_id: &str) -> std::path::PathBuf {
    codex_home
        .join(".pooled-auth/backends/local/accounts")
        .join(account_id)
}

fn auth_dot_json_for_account(account_id: &str) -> AuthDotJson {
    let access_token = fake_access_token(account_id);
    AuthDotJson {
        auth_mode: Some(AuthMode::Chatgpt),
        openai_api_key: None,
        tokens: Some(TokenData {
            id_token: codex_login::token_data::parse_chatgpt_jwt_claims(&access_token)
                .expect("fake access token should parse"),
            access_token,
            refresh_token: "refresh-token".to_string(),
            account_id: Some(account_id.to_string()),
        }),
        last_refresh: Some(Utc::now()),
    }
}

fn fake_access_token(chatgpt_account_id: &str) -> String {
    let header = serde_json::json!({
        "alg": "none",
        "typ": "JWT",
    });
    let payload = serde_json::json!({
        "email": "user@example.com",
        "email_verified": true,
        "https://api.openai.com/auth": {
            "chatgpt_plan_type": "pro",
            "chatgpt_user_id": "user-12345",
            "chatgpt_account_id": chatgpt_account_id,
        },
    });
    let b64 = |value: serde_json::Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&value).expect("serialize fake JWT part"))
    };
    format!("{}.{}.sig", b64(header), b64(payload))
}
