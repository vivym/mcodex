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
use crate::state::SessionLeaseContinuity;
use crate::state::SessionServices;
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
async fn runtime_lease_host_reuse_after_successful_shutdown_binds_children_to_new_bridge_manager()
-> anyhow::Result<()> {
    let codex_home = tempfile::tempdir()?;
    let state_db =
        codex_state::StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into())
            .await?;
    let runtime_lease_host =
        RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new("runtime-reuse".to_string()));
    let first_manager = SessionServices::build_account_pool_manager(
        Some(state_db.clone()),
        Some(test_accounts_config()),
        codex_home.path().to_path_buf(),
        "holder-old".to_string(),
    )
    .await?
    .expect("test manager should build");

    runtime_lease_host.attach_legacy_manager_bridge(Arc::clone(&first_manager))?;
    runtime_lease_host.attach_session("session-old").await;
    runtime_lease_host.detach_session("session-old").await?;

    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    assert!(
        !runtime_lease_host.has_legacy_manager_bridge_for_test(),
        "final successful detach should clear the old bridge"
    );

    let second_manager = SessionServices::build_account_pool_manager(
        Some(state_db),
        Some(test_accounts_config()),
        codex_home.path().to_path_buf(),
        "holder-new".to_string(),
    )
    .await?
    .expect("replacement manager should build");

    runtime_lease_host.attach_legacy_manager_bridge(Arc::clone(&second_manager))?;
    runtime_lease_host.attach_session("session-new").await;

    let rebound_manager = runtime_lease_host
        .legacy_manager_bridge_for_test()
        .expect("replacement manager should be visible to child sessions");
    assert!(Arc::ptr_eq(&rebound_manager, &second_manager));
    assert!(!Arc::ptr_eq(&rebound_manager, &first_manager));

    Ok(())
}

#[tokio::test]
async fn runtime_lease_host_rejects_different_bridge_manager_until_stale_sessions_detach()
-> anyhow::Result<()> {
    let codex_home = tempfile::tempdir()?;
    let state_db =
        codex_state::StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into())
            .await?;
    let runtime_lease_host =
        RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new("runtime-reuse".to_string()));
    let first_manager = SessionServices::build_account_pool_manager(
        Some(state_db.clone()),
        Some(test_accounts_config()),
        codex_home.path().to_path_buf(),
        "holder-old".to_string(),
    )
    .await?
    .expect("test manager should build");
    let second_manager = SessionServices::build_account_pool_manager(
        Some(state_db),
        Some(test_accounts_config()),
        codex_home.path().to_path_buf(),
        "holder-new".to_string(),
    )
    .await?
    .expect("replacement manager should build");

    runtime_lease_host.attach_legacy_manager_bridge(Arc::clone(&first_manager))?;
    runtime_lease_host.attach_session("session-root-old").await;
    runtime_lease_host.attach_session("session-child-old").await;

    let err = runtime_lease_host
        .attach_legacy_manager_bridge(Arc::clone(&second_manager))
        .expect_err("a later root must not silently keep using a stale bridge");
    assert!(
        err.to_string().contains("different legacy manager bridge"),
        "unexpected error: {err:#}"
    );
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        2
    );
    assert!(Arc::ptr_eq(
        &runtime_lease_host
            .legacy_manager_bridge_for_test()
            .expect("stale bridge should still point at the old manager"),
        &first_manager,
    ));

    runtime_lease_host
        .detach_session("session-root-old")
        .await?;
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        1
    );
    assert!(runtime_lease_host.has_legacy_manager_bridge_for_test());

    runtime_lease_host
        .detach_session("session-child-old")
        .await?;
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    assert!(
        !runtime_lease_host.has_legacy_manager_bridge_for_test(),
        "the stale bridge must clear once the old sessions are gone"
    );

    runtime_lease_host.attach_legacy_manager_bridge(Arc::clone(&second_manager))?;
    runtime_lease_host.attach_session("session-root-new").await;

    assert!(Arc::ptr_eq(
        &runtime_lease_host
            .legacy_manager_bridge_for_test()
            .expect("successful reuse should install the new bridge"),
        &second_manager,
    ));

    Ok(())
}

#[tokio::test]
async fn runtime_lease_host_failed_final_release_stays_retryable_until_bridge_clears()
-> anyhow::Result<()> {
    let codex_home = tempfile::tempdir()?;
    let config = build_test_config_with_pool(codex_home.path()).await;
    let state_db =
        codex_state::StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into())
            .await?;
    let account_id = "acct-release-retry";
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
        &pooled_auth_home(codex_home.path(), account_id),
        &auth_dot_json_for_account(account_id),
        AuthCredentialsStoreMode::File,
    )?;

    let manager = SessionServices::build_account_pool_manager(
        Some(state_db.clone()),
        config.accounts.clone(),
        codex_home.path().to_path_buf(),
        "holder-release-retry".to_string(),
    )
    .await?
    .expect("test manager should build");
    {
        let mut manager = manager.lock().await;
        let selection = manager
            .prepare_turn()
            .await?
            .expect("test manager should acquire a lease before shutdown");
        assert_eq!(selection.account_id, account_id);
    }

    let lease_epoch_marker = pooled_auth_home(codex_home.path(), account_id).join("lease_epoch");
    std::fs::remove_file(&lease_epoch_marker)?;
    std::fs::create_dir(&lease_epoch_marker)?;

    let runtime_lease_host =
        RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new("runtime-retry".to_string()));
    runtime_lease_host.attach_legacy_manager_bridge(Arc::clone(&manager))?;
    runtime_lease_host.attach_session("session-retry").await;

    let first_release = runtime_lease_host.detach_session("session-retry").await;
    assert!(first_release.is_err());
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        1,
        "the final session must remain attached when release fails"
    );
    assert!(
        runtime_lease_host.has_legacy_manager_bridge_for_test(),
        "the bridge must remain installed for a later retry"
    );

    std::fs::remove_dir(&lease_epoch_marker)?;

    runtime_lease_host.detach_session("session-retry").await?;

    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    assert!(
        !runtime_lease_host.has_legacy_manager_bridge_for_test(),
        "a later successful retry should clear the bridge"
    );
    assert!(
        state_db
            .read_active_holder_lease("holder-release-retry")
            .await?
            .is_none(),
        "the retried shutdown should finish releasing the lease"
    );

    Ok(())
}

#[tokio::test]
async fn runtime_lease_host_retries_transient_final_release_failure_before_shutdown_completes()
-> anyhow::Result<()> {
    let codex_home = tempfile::tempdir()?;
    let config = build_test_config_with_pool(codex_home.path()).await;
    let state_db =
        codex_state::StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into())
            .await?;
    let account_id = "acct-release-retry-auto";
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
        &pooled_auth_home(codex_home.path(), account_id),
        &auth_dot_json_for_account(account_id),
        AuthCredentialsStoreMode::File,
    )?;

    let manager = SessionServices::build_account_pool_manager(
        Some(state_db.clone()),
        config.accounts.clone(),
        codex_home.path().to_path_buf(),
        "holder-release-retry-auto".to_string(),
    )
    .await?
    .expect("test manager should build");
    {
        let mut manager = manager.lock().await;
        let selection = manager
            .prepare_turn()
            .await?
            .expect("test manager should acquire a lease before shutdown");
        assert_eq!(selection.account_id, account_id);
    }

    let lease_epoch_marker = pooled_auth_home(codex_home.path(), account_id).join("lease_epoch");
    std::fs::remove_file(&lease_epoch_marker)?;
    std::fs::create_dir(&lease_epoch_marker)?;

    let runtime_lease_host = RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new(
        "runtime-retry-auto".to_string(),
    ));
    runtime_lease_host.attach_legacy_manager_bridge(Arc::clone(&manager))?;
    runtime_lease_host
        .attach_session("session-retry-auto")
        .await;

    let release_blocker = lease_epoch_marker.clone();
    let unblock_release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        std::fs::remove_dir(&release_blocker)
    });

    runtime_lease_host
        .detach_session_with_retry("session-retry-auto")
        .await?;
    unblock_release.await??;

    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    assert!(
        !runtime_lease_host.has_legacy_manager_bridge_for_test(),
        "shutdown retries should clear the bridge before returning"
    );
    assert!(
        state_db
            .read_active_holder_lease("holder-release-retry-auto")
            .await?
            .is_none(),
        "shutdown retries should eventually release the pooled lease"
    );

    Ok(())
}

#[tokio::test]
async fn pending_inherited_child_startup_keeps_bridge_until_success_or_rollback()
-> anyhow::Result<()> {
    let codex_home = tempfile::tempdir()?;
    let state_db =
        codex_state::StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into())
            .await?;
    let runtime_lease_host = RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new(
        "runtime-pending-child".to_string(),
    ));
    let manager = SessionServices::build_account_pool_manager(
        Some(state_db),
        Some(test_accounts_config()),
        codex_home.path().to_path_buf(),
        "holder-pending-child".to_string(),
    )
    .await?
    .expect("test manager should build");

    runtime_lease_host.attach_legacy_manager_bridge(Arc::clone(&manager))?;
    runtime_lease_host.attach_session("session-parent").await;
    let child_startup = runtime_lease_host
        .reserve_startup("session-child-startup")
        .await;

    runtime_lease_host.detach_session("session-parent").await?;

    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    assert_eq!(runtime_lease_host.pending_startup_count_for_test().await, 1);
    assert!(
        runtime_lease_host.has_legacy_manager_bridge_for_test(),
        "parent shutdown must not clear the bridge while child startup is reserved"
    );

    child_startup
        .promote_to_session("session-child")
        .await
        .expect("successful child startup should attach the child session");

    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        1
    );
    assert_eq!(runtime_lease_host.pending_startup_count_for_test().await, 0);
    assert!(runtime_lease_host.has_legacy_manager_bridge_for_test());

    runtime_lease_host.detach_session("session-child").await?;
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    assert!(
        !runtime_lease_host.has_legacy_manager_bridge_for_test(),
        "final child detach should release the bridge after successful startup"
    );

    runtime_lease_host.attach_legacy_manager_bridge(Arc::clone(&manager))?;
    runtime_lease_host
        .attach_session("session-parent-rollback")
        .await;
    let child_startup = runtime_lease_host
        .reserve_startup("session-child-rollback")
        .await;

    runtime_lease_host
        .detach_session("session-parent-rollback")
        .await?;

    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    assert_eq!(runtime_lease_host.pending_startup_count_for_test().await, 1);
    assert!(runtime_lease_host.has_legacy_manager_bridge_for_test());

    child_startup
        .rollback()
        .await
        .expect("failed child startup should release the last reserved bridge");

    assert_eq!(runtime_lease_host.pending_startup_count_for_test().await, 0);
    assert!(
        !runtime_lease_host.has_legacy_manager_bridge_for_test(),
        "rollback of the last pending startup should release the bridge"
    );

    Ok(())
}

#[tokio::test]
async fn bridged_sessions_keep_remote_context_continuity_independent() -> anyhow::Result<()> {
    let codex_home = tempfile::tempdir()?;
    let state_db =
        codex_state::StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into())
            .await?;
    let config = test_accounts_config();
    for (position, account_id) in ["acct-session-a", "acct-session-b"].into_iter().enumerate() {
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
        save_auth(
            &pooled_auth_home(codex_home.path(), account_id),
            &auth_dot_json_for_account(account_id),
            AuthCredentialsStoreMode::File,
        )?;
    }
    state_db
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("pool-main".to_string()),
            preferred_account_id: None,
            suppressed: false,
        })
        .await?;
    let manager = SessionServices::build_account_pool_manager(
        Some(state_db),
        Some(config),
        codex_home.path().to_path_buf(),
        "holder-shared-continuity".to_string(),
    )
    .await?
    .expect("test manager should build");

    let mut manager = manager.lock().await;
    let first_selection = manager
        .prepare_turn()
        .await?
        .expect("first turn should select the preferred account");
    assert_eq!(first_selection.account_id, "acct-session-a");
    assert!(
        !first_selection.allow_context_reuse,
        "test pool must require reset on account changes"
    );
    let mut session_a = SessionLeaseContinuity::default();
    let mut session_b = SessionLeaseContinuity::default();
    assert!(
        !session_a.reset_remote_context_for_selection(&first_selection),
        "first turn in a session has no prior remote context to reset"
    );
    assert!(
        !session_b.reset_remote_context_for_selection(&first_selection),
        "each session tracks its own first selected account"
    );

    manager.report_unauthorized().await?;
    let second_selection = manager
        .prepare_turn()
        .await?
        .expect("hard failure should rotate to another healthy account");
    assert_eq!(second_selection.account_id, "acct-session-b");
    assert!(
        session_b.reset_remote_context_for_selection(&second_selection),
        "session B must reset when its selected account changes"
    );
    assert!(
        session_a.reset_remote_context_for_selection(&second_selection),
        "session A must still reset even after session B observed the new account"
    );

    Ok(())
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
            auth_manager.clone(),
            /*model_catalog*/ None,
            CollaborationModesConfig::default(),
        )),
        environment_manager: Arc::new(EnvironmentManager::new(/*exec_server_url*/ None)),
        skills_manager: Arc::new(SkillsManager::new(
            config_codex_home.clone(),
            /*bundled_skills_enabled*/ true,
        )),
        plugins_manager: Arc::clone(&plugins_manager),
        mcp_manager: Arc::new(McpManager::new(Arc::clone(&plugins_manager))),
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
    assert!(
        !runtime_lease_host.has_legacy_manager_bridge_for_test(),
        "failed startup must not leave a stale legacy bridge on the supplied host"
    );

    let recovered = Codex::spawn(CodexSpawnArgs {
        config: build_test_config_with_pool(codex_home.path()).await,
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
    .await?
    .codex;

    assert!(
        runtime_lease_host.has_legacy_manager_bridge_for_test(),
        "a later successful startup should still attach the legacy bridge"
    );
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        1
    );
    assert!(Arc::ptr_eq(
        &runtime_lease_host
            .legacy_manager_bridge_for_test()
            .expect("successful startup should register a legacy bridge"),
        recovered
            .session
            .services
            .account_pool_manager
            .as_ref()
            .expect("successful pooled root should keep a local manager"),
    ));

    recovered.shutdown_and_wait().await?;
    Ok(())
}

#[tokio::test]
async fn root_startup_rejects_stale_runtime_bridge_without_attaching_session() -> anyhow::Result<()>
{
    let codex_home = tempfile::tempdir()?;
    let config = build_test_config_with_pool(codex_home.path()).await;
    let config_codex_home = config.codex_home.clone();
    let auth_manager =
        codex_login::AuthManager::from_auth_for_testing(CodexAuth::from_api_key("Test API Key"));
    let models_manager = Arc::new(ModelsManager::new(
        config_codex_home.to_path_buf(),
        auth_manager.clone(),
        /*model_catalog*/ None,
        CollaborationModesConfig::default(),
    ));
    let plugins_manager = Arc::new(PluginsManager::new(config_codex_home.to_path_buf()));
    let mcp_manager = Arc::new(McpManager::new(Arc::clone(&plugins_manager)));
    let skills_manager = Arc::new(SkillsManager::new(
        config_codex_home.clone(),
        /*bundled_skills_enabled*/ true,
    ));
    let skills_watcher = Arc::new(SkillsWatcher::noop());
    let runtime_lease_host = RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new(
        "runtime-stale-startup".to_string(),
    ));

    let first_root = Codex::spawn(CodexSpawnArgs {
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
    let first_bridge = runtime_lease_host
        .legacy_manager_bridge_for_test()
        .expect("first root should install a bridge");
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        1
    );

    let second_root = Codex::spawn(CodexSpawnArgs {
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
    .await;
    let err = match second_root {
        Ok(spawned) => {
            spawned.codex.shutdown_and_wait().await?;
            panic!("startup with a stale runtime bridge should fail");
        }
        Err(err) => err,
    };
    assert!(
        err.to_string()
            .contains("failed to attach runtime lease bridge"),
        "unexpected error: {err:#}"
    );
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        1,
        "failed startup must not commit a new runtime attachment"
    );
    assert!(Arc::ptr_eq(
        &runtime_lease_host
            .legacy_manager_bridge_for_test()
            .expect("stale bridge should still point at the first root"),
        &first_bridge,
    ));

    first_root.shutdown_and_wait().await?;
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    assert!(
        !runtime_lease_host.has_legacy_manager_bridge_for_test(),
        "successful shutdown should clear the stale bridge before reuse"
    );

    let recovered_root = Codex::spawn(CodexSpawnArgs {
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
        runtime_lease_host: Some(runtime_lease_host.clone()),
        user_shell_override: None,
        parent_trace: None,
        analytics_events_client: None,
    })
    .await?
    .codex;
    let rebound_bridge = runtime_lease_host
        .legacy_manager_bridge_for_test()
        .expect("recovered root should install a fresh bridge");
    let recovered_manager = recovered_root
        .session
        .services
        .account_pool_manager
        .as_ref()
        .expect("recovered root should keep its local manager");
    assert!(Arc::ptr_eq(&rebound_bridge, recovered_manager));
    assert!(!Arc::ptr_eq(&rebound_bridge, &first_bridge));

    recovered_root.shutdown_and_wait().await?;
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

fn test_accounts_config() -> AccountsConfigToml {
    let mut pools = HashMap::new();
    pools.insert(
        "pool-main".to_string(),
        AccountPoolDefinitionToml {
            allow_context_reuse: Some(false),
            account_kinds: None,
        },
    );

    AccountsConfigToml {
        backend: None,
        default_pool: None,
        proactive_switch_threshold_percent: None,
        lease_ttl_secs: None,
        heartbeat_interval_secs: None,
        min_switch_interval_secs: None,
        allocation_mode: None,
        pools: Some(pools),
    }
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
