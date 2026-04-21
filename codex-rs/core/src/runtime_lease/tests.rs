use super::CollaborationTreeId;
use super::LeaseAdmissionError;
use super::LeaseRequestContext;
use super::LeaseSnapshot;
use super::RemoteContextResetRecord;
use super::RequestBoundaryKind;
use super::RuntimeLeaseAuthority;
use super::RuntimeLeaseHost;
use super::RuntimeLeaseHostId;
use super::RuntimeLeaseHostMode;
use super::session_view::SessionLeaseView;
use super::session_view::SessionLeaseViewDecision;
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
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::RateLimitWindow;
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
fn session_view_resets_only_when_account_changes_and_pool_disallows_reuse() {
    let mut view = SessionLeaseView::for_test();
    let first = LeaseSnapshot::for_test(
        "pool-main",
        "acct-a",
        "codex",
        1,
        /*allow_context_reuse*/ false,
    );
    let second = LeaseSnapshot::for_test(
        "pool-main",
        "acct-a",
        "codex",
        1,
        /*allow_context_reuse*/ false,
    );
    let third = LeaseSnapshot::for_test(
        "pool-main",
        "acct-b",
        "codex",
        2,
        /*allow_context_reuse*/ false,
    );

    assert_eq!(
        view.before_request_for_test(&first),
        SessionLeaseViewDecision::Continue
    );
    assert_eq!(
        view.before_request_for_test(&second),
        SessionLeaseViewDecision::Continue
    );
    assert_eq!(
        view.before_request_for_test(&third),
        SessionLeaseViewDecision::ResetRemoteContext
    );
}

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
async fn pooled_host_snapshot_uses_authority_state_without_session_local_manager() {
    let host = RuntimeLeaseHost::pooled_with_authority_for_test(
        RuntimeLeaseHostId::new("runtime-snapshot".to_string()),
        RuntimeLeaseAuthority::for_test_accepting("acct-a", 11),
    );
    host.record_remote_context_reset(RemoteContextResetRecord {
        session_id: "session-a".to_string(),
        turn_id: Some("turn-1".to_string()),
        request_id: "req-1".to_string(),
        lease_generation: 11,
        transport_reset_generation: 7,
    });

    let snapshot = host
        .account_lease_snapshot()
        .await
        .expect("pooled host should expose a live authority snapshot");

    assert!(snapshot.active);
    assert_eq!(snapshot.account_id.as_deref(), Some("acct-a"));
    assert_eq!(snapshot.pool_id.as_deref(), Some("pool-main"));
    assert_eq!(snapshot.lease_epoch, Some(11));
    assert_eq!(snapshot.transport_reset_generation, Some(7));
    assert_eq!(
        snapshot.last_remote_context_reset_turn_id.as_deref(),
        Some("turn-1")
    );
}

#[tokio::test]
async fn pooled_host_release_for_shutdown_releases_manager_owner_lease() -> anyhow::Result<()> {
    let codex_home = tempfile::tempdir()?;
    let state_db =
        codex_state::StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into())
            .await?;
    let account_id = "acct-shutdown";
    state_db
        .import_legacy_default_account(codex_state::LegacyAccountImport {
            account_id: account_id.to_string(),
        })
        .await?;
    save_auth(
        &pooled_auth_home(codex_home.path(), account_id),
        &auth_dot_json_for_account(account_id),
        AuthCredentialsStoreMode::File,
    )?;

    let host =
        RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new("runtime-shutdown".to_string()));
    let mut pools = HashMap::new();
    pools.insert(
        "legacy-default".to_string(),
        AccountPoolDefinitionToml {
            allow_context_reuse: Some(false),
            account_kinds: None,
        },
    );
    let manager = SessionServices::build_account_pool_manager(
        Some(state_db),
        Some(AccountsConfigToml {
            backend: None,
            default_pool: Some("legacy-default".to_string()),
            proactive_switch_threshold_percent: None,
            lease_ttl_secs: None,
            heartbeat_interval_secs: None,
            min_switch_interval_secs: None,
            allocation_mode: None,
            pools: Some(pools),
        }),
        codex_home.path().to_path_buf(),
        "holder-shutdown".to_string(),
    )
    .await?
    .expect("test manager should build");
    host.install_manager_owner(Arc::clone(&manager))?;

    {
        let mut manager = manager.lock().await;
        manager
            .prepare_turn()
            .await?
            .expect("manager should acquire an active lease before shutdown");
    }

    host.release_for_shutdown().await?;

    let snapshot = host
        .account_lease_snapshot()
        .await
        .expect("pooled host should still expose an inactive snapshot after shutdown release");
    assert!(!snapshot.active);

    Ok(())
}

#[tokio::test]
async fn runtime_lease_host_reuse_after_successful_shutdown_republishes_authority()
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

    runtime_lease_host.install_manager_owner(Arc::clone(&first_manager))?;
    runtime_lease_host.attach_session("session-old").await;
    runtime_lease_host.detach_session("session-old").await?;

    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    assert!(runtime_lease_host.pooled_authority().is_none());

    let second_manager = SessionServices::build_account_pool_manager(
        Some(state_db),
        Some(test_accounts_config()),
        codex_home.path().to_path_buf(),
        "holder-new".to_string(),
    )
    .await?
    .expect("replacement manager should build");

    runtime_lease_host.install_manager_owner(Arc::clone(&second_manager))?;
    runtime_lease_host.attach_session("session-new").await;

    assert!(runtime_lease_host.pooled_authority().is_some());
    let _ = first_manager;
    let _ = second_manager;

    Ok(())
}

#[tokio::test]
async fn admission_guard_releases_exactly_once() {
    let authority = RuntimeLeaseAuthority::for_test_accepting("acct-a", 11);
    let request_context = LeaseRequestContext::for_test(
        RequestBoundaryKind::ResponsesHttp,
        "session-a",
        CollaborationTreeId::for_test("tree-a"),
    );

    let admission = authority
        .acquire_request_lease_for_test(request_context)
        .await
        .unwrap();
    assert_eq!(authority.admitted_count_for_test(), 1);

    drop(admission.guard);
    assert_eq!(authority.admitted_count_for_test(), 0);
}

#[tokio::test]
async fn draining_acquire_waits_until_replacement_generation() {
    let authority = RuntimeLeaseAuthority::for_test_accepting("acct-a", 11);
    let request_context = LeaseRequestContext::for_test(
        RequestBoundaryKind::ResponsesHttp,
        "session-a",
        CollaborationTreeId::for_test("tree-a"),
    );
    let first = authority
        .acquire_request_lease_for_test(request_context.clone())
        .await
        .unwrap();

    authority.close_current_generation_for_test().await;
    let waiter = tokio::spawn({
        let authority = authority.clone();
        async move {
            authority
                .acquire_request_lease_for_test(request_context)
                .await
                .unwrap()
        }
    });

    tokio::task::yield_now().await;
    assert!(!waiter.is_finished());

    drop(first.guard);
    authority.install_replacement_for_test("acct-b", 12).await;
    let second = waiter.await.unwrap();

    assert_eq!(second.snapshot.account_id(), "acct-b");
    assert_eq!(second.snapshot.generation(), 12);
}

#[tokio::test]
async fn manager_owner_rotations_produce_new_generation_and_gate_replacement_admission()
-> anyhow::Result<()> {
    let codex_home = tempfile::tempdir()?;
    let state_db =
        codex_state::StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into())
            .await?;
    for account_id in ["acct-legacy-a", "acct-legacy-b"] {
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
        save_auth(
            &pooled_auth_home(codex_home.path(), account_id),
            &auth_dot_json_for_account(account_id),
            AuthCredentialsStoreMode::File,
        )?;
    }
    state_db
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("pool-main".to_string()),
            preferred_account_id: Some("acct-legacy-a".to_string()),
            suppressed: false,
        })
        .await?;
    let manager = SessionServices::build_account_pool_manager(
        Some(state_db),
        Some(test_accounts_config()),
        codex_home.path().to_path_buf(),
        "holder-legacy-generation".to_string(),
    )
    .await?
    .expect("test manager should build");
    let authority = RuntimeLeaseAuthority::manager_owner(Arc::clone(&manager));
    let request_context = LeaseRequestContext::for_test(
        RequestBoundaryKind::ResponsesHttp,
        "session-a",
        CollaborationTreeId::for_test("tree-a"),
    );

    let first = authority
        .acquire_request_lease_for_test(request_context.clone())
        .await?;
    assert_eq!(first.snapshot.account_id(), "acct-legacy-a");
    assert_ne!(first.snapshot.generation(), 0);

    {
        let mut manager = manager.lock().await;
        manager.report_unauthorized().await?;
    }

    let err = authority
        .acquire_request_lease_for_test(request_context.clone())
        .await
        .expect_err(
            "rotated replacement lease must not admit while the prior generation is active",
        );
    assert_eq!(err, LeaseAdmissionError::UnsupportedPooledPath);

    let first_generation = first.snapshot.generation();
    drop(first.guard);

    let second = authority
        .acquire_request_lease_for_test(request_context)
        .await?;
    assert_eq!(second.snapshot.account_id(), "acct-legacy-b");
    assert!(second.snapshot.generation() > first_generation);

    Ok(())
}

#[tokio::test]
async fn manager_owner_admission_heartbeat_renews_active_lease_until_guard_drop()
-> anyhow::Result<()> {
    let codex_home = tempfile::tempdir()?;
    let state_db =
        codex_state::StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into())
            .await?;
    let account_id = "acct-legacy-heartbeat";
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
    save_auth(
        &pooled_auth_home(codex_home.path(), account_id),
        &auth_dot_json_for_account(account_id),
        AuthCredentialsStoreMode::File,
    )?;
    state_db
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("pool-main".to_string()),
            preferred_account_id: Some(account_id.to_string()),
            suppressed: false,
        })
        .await?;
    let holder_instance_id = "holder-legacy-heartbeat";
    let mut accounts = test_accounts_config();
    accounts.lease_ttl_secs = Some(4);
    accounts.heartbeat_interval_secs = Some(1);
    let manager = SessionServices::build_account_pool_manager(
        Some(state_db.clone()),
        Some(accounts),
        codex_home.path().to_path_buf(),
        holder_instance_id.to_string(),
    )
    .await?
    .expect("test manager should build");
    let authority = RuntimeLeaseAuthority::manager_owner(manager);
    let request_context = LeaseRequestContext::for_test(
        RequestBoundaryKind::ResponsesCompact,
        "session-heartbeat",
        CollaborationTreeId::for_test("tree-heartbeat"),
    );

    let admission = authority
        .acquire_request_lease_for_test(request_context)
        .await?;
    let initial_lease = state_db
        .read_active_holder_lease(holder_instance_id)
        .await?
        .expect("admission should acquire an active holder lease");

    let renewed_lease = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let active_lease = state_db
                .read_active_holder_lease(holder_instance_id)
                .await?
                .expect("heartbeat should keep the holder lease active");
            if active_lease.expires_at > initial_lease.expires_at {
                return Ok::<_, anyhow::Error>(active_lease);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await??;
    assert!(renewed_lease.expires_at > initial_lease.expires_at);

    drop(admission.guard);
    assert_eq!(authority.admitted_count_for_test(), 0);

    Ok(())
}

#[tokio::test]
async fn manager_owner_rejected_replacement_acquire_preserves_first_snapshot_until_guard_drop()
-> anyhow::Result<()> {
    let codex_home = tempfile::tempdir()?;
    let state_db =
        codex_state::StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into())
            .await?;
    for account_id in ["acct-preserve-a", "acct-preserve-b"] {
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
        save_auth(
            &pooled_auth_home(codex_home.path(), account_id),
            &auth_dot_json_for_account(account_id),
            AuthCredentialsStoreMode::File,
        )?;
    }
    state_db
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("pool-main".to_string()),
            preferred_account_id: Some("acct-preserve-a".to_string()),
            suppressed: false,
        })
        .await?;
    let manager = SessionServices::build_account_pool_manager(
        Some(state_db.clone()),
        Some(test_accounts_config()),
        codex_home.path().to_path_buf(),
        "holder-preserve-first-snapshot".to_string(),
    )
    .await?
    .expect("test manager should build");
    let authority = RuntimeLeaseAuthority::manager_owner(Arc::clone(&manager));
    let request_context = LeaseRequestContext::for_test(
        RequestBoundaryKind::ResponsesHttp,
        "session-a",
        CollaborationTreeId::for_test("tree-a"),
    );

    let first = authority
        .acquire_request_lease_for_test(request_context.clone())
        .await?;
    let first_snapshot = first.snapshot.clone();

    {
        let mut manager = manager.lock().await;
        manager.report_unauthorized().await?;
    }

    let err = authority
        .acquire_request_lease_for_test(request_context.clone())
        .await
        .expect_err(
            "rotated replacement lease must not mutate the active bridged generation while the prior admission is active",
        );
    assert_eq!(err, LeaseAdmissionError::UnsupportedPooledPath);

    let active_holder_lease = state_db
        .read_active_holder_lease("holder-preserve-first-snapshot")
        .await?
        .expect("the first bridged lease should remain active until its admission guard drops");
    assert_eq!(active_holder_lease.account_id, "acct-preserve-a");

    let leased_auth = first_snapshot
        .auth_handle
        .auth_session()
        .leased_turn_auth()?;
    assert_eq!(
        leased_auth.auth().get_account_id().as_deref(),
        Some("acct-preserve-a")
    );

    authority
        .report_rate_limits(
            &first_snapshot,
            &RateLimitSnapshot {
                limit_id: Some("codex".to_string()),
                limit_name: Some("Codex".to_string()),
                primary: Some(RateLimitWindow {
                    used_percent: 99.0,
                    window_minutes: Some(60),
                    resets_at: None,
                }),
                secondary: None,
                credits: None,
                plan_type: None,
            },
        )
        .await?;

    drop(first.guard);

    let second = authority
        .acquire_request_lease_for_test(request_context)
        .await?;
    assert_eq!(second.snapshot.account_id(), "acct-preserve-b");

    Ok(())
}

#[tokio::test]
async fn manager_owner_rejects_stale_reporting_after_rotation() -> anyhow::Result<()> {
    let codex_home = tempfile::tempdir()?;
    let state_db =
        codex_state::StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into())
            .await?;
    for account_id in ["acct-stale-a", "acct-stale-b"] {
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
        save_auth(
            &pooled_auth_home(codex_home.path(), account_id),
            &auth_dot_json_for_account(account_id),
            AuthCredentialsStoreMode::File,
        )?;
    }
    state_db
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("pool-main".to_string()),
            preferred_account_id: Some("acct-stale-a".to_string()),
            suppressed: false,
        })
        .await?;
    let manager = SessionServices::build_account_pool_manager(
        Some(state_db),
        Some(test_accounts_config()),
        codex_home.path().to_path_buf(),
        "holder-stale-reporting".to_string(),
    )
    .await?
    .expect("test manager should build");
    let authority = RuntimeLeaseAuthority::manager_owner(Arc::clone(&manager));
    let request_context = LeaseRequestContext::for_test(
        RequestBoundaryKind::ResponsesHttp,
        "session-a",
        CollaborationTreeId::for_test("tree-a"),
    );

    let first = authority
        .acquire_request_lease_for_test(request_context.clone())
        .await?;
    let first_snapshot = first.snapshot.clone();

    {
        let mut manager = manager.lock().await;
        manager.report_unauthorized().await?;
    }
    drop(first.guard);

    let second = authority
        .acquire_request_lease_for_test(request_context.clone())
        .await?;
    assert_eq!(second.snapshot.account_id(), "acct-stale-b");

    let rate_limits = RateLimitSnapshot {
        limit_id: Some("codex".to_string()),
        limit_name: Some("Codex".to_string()),
        primary: Some(RateLimitWindow {
            used_percent: 99.0,
            window_minutes: Some(60),
            resets_at: None,
        }),
        secondary: None,
        credits: None,
        plan_type: None,
    };
    let rate_limit_err = authority
        .report_rate_limits(&first_snapshot, &rate_limits)
        .await
        .expect_err("stale rate-limit reports must be rejected");
    assert!(
        rate_limit_err.to_string().contains("stale"),
        "unexpected stale rate-limit error: {rate_limit_err:#}"
    );
    let usage_limit_err = authority
        .report_usage_limit_reached(&first_snapshot)
        .await
        .expect_err("stale usage-limit reports must be rejected");
    assert!(
        usage_limit_err.to_string().contains("stale"),
        "unexpected stale usage-limit error: {usage_limit_err:#}"
    );
    let unauthorized_err = authority
        .report_terminal_unauthorized(&first_snapshot)
        .await
        .expect_err("stale unauthorized reports must be rejected");
    assert!(
        unauthorized_err.to_string().contains("stale"),
        "unexpected stale unauthorized error: {unauthorized_err:#}"
    );

    let second_generation = second.snapshot.generation();
    drop(second.guard);

    let current = authority
        .acquire_request_lease_for_test(request_context)
        .await?;
    assert_eq!(current.snapshot.account_id(), "acct-stale-b");
    assert_eq!(current.snapshot.generation(), second_generation);

    Ok(())
}

#[tokio::test]
async fn manager_owner_usage_limit_report_rotates_legacy_default_turn() -> anyhow::Result<()> {
    const LEGACY_DEFAULT_POOL_ID: &str = "legacy-default";

    let codex_home = tempfile::tempdir()?;
    let state_db =
        codex_state::StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into())
            .await?;
    for account_id in ["acct-turn-a", "acct-turn-b"] {
        state_db
            .import_legacy_default_account(codex_state::LegacyAccountImport {
                account_id: account_id.to_string(),
            })
            .await?;
        save_auth(
            &pooled_auth_home(codex_home.path(), account_id),
            &auth_dot_json_for_account(account_id),
            AuthCredentialsStoreMode::File,
        )?;
    }
    let mut pools = HashMap::new();
    pools.insert(
        LEGACY_DEFAULT_POOL_ID.to_string(),
        AccountPoolDefinitionToml {
            allow_context_reuse: Some(false),
            account_kinds: None,
        },
    );
    let manager = SessionServices::build_account_pool_manager(
        Some(state_db),
        Some(AccountsConfigToml {
            backend: None,
            default_pool: Some(LEGACY_DEFAULT_POOL_ID.to_string()),
            proactive_switch_threshold_percent: Some(85),
            lease_ttl_secs: None,
            heartbeat_interval_secs: None,
            min_switch_interval_secs: None,
            allocation_mode: None,
            pools: Some(pools),
        }),
        codex_home.path().to_path_buf(),
        "holder-turn-report".to_string(),
    )
    .await?
    .expect("test manager should build");
    let authority = RuntimeLeaseAuthority::manager_owner(Arc::clone(&manager));
    let turn_selection = {
        let mut manager = manager.lock().await;
        manager
            .prepare_turn()
            .await?
            .expect("outer turn should acquire a pooled lease")
    };
    let request_context = LeaseRequestContext::for_test(
        RequestBoundaryKind::ResponsesCompact,
        "session-turn-report",
        CollaborationTreeId::for_test("tree-turn-report"),
    );

    let admission = authority
        .acquire_request_lease_for_test(request_context)
        .await?;
    assert_eq!(admission.snapshot.account_id(), "acct-turn-a");
    assert_eq!(admission.snapshot.generation(), turn_selection.generation);

    authority
        .report_usage_limit_reached(&admission.snapshot)
        .await?;

    drop(admission.guard);

    let next_selection = {
        let mut manager = manager.lock().await;
        manager
            .prepare_turn()
            .await?
            .expect("next turn should rotate after the reported usage limit")
    };
    assert_eq!(next_selection.account_id, "acct-turn-b");
    assert!(next_selection.generation > turn_selection.generation);

    Ok(())
}

#[tokio::test]
async fn manager_owner_terminal_unauthorized_report_rotates_legacy_default_turn()
-> anyhow::Result<()> {
    const LEGACY_DEFAULT_POOL_ID: &str = "legacy-default";

    let codex_home = tempfile::tempdir()?;
    let state_db =
        codex_state::StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into())
            .await?;
    for account_id in ["acct-turn-a", "acct-turn-b"] {
        state_db
            .import_legacy_default_account(codex_state::LegacyAccountImport {
                account_id: account_id.to_string(),
            })
            .await?;
        save_auth(
            &pooled_auth_home(codex_home.path(), account_id),
            &auth_dot_json_for_account(account_id),
            AuthCredentialsStoreMode::File,
        )?;
    }
    let mut pools = HashMap::new();
    pools.insert(
        LEGACY_DEFAULT_POOL_ID.to_string(),
        AccountPoolDefinitionToml {
            allow_context_reuse: Some(false),
            account_kinds: None,
        },
    );
    let manager = SessionServices::build_account_pool_manager(
        Some(state_db),
        Some(AccountsConfigToml {
            backend: None,
            default_pool: Some(LEGACY_DEFAULT_POOL_ID.to_string()),
            proactive_switch_threshold_percent: Some(85),
            lease_ttl_secs: None,
            heartbeat_interval_secs: None,
            min_switch_interval_secs: None,
            allocation_mode: None,
            pools: Some(pools),
        }),
        codex_home.path().to_path_buf(),
        "holder-turn-unauthorized-report".to_string(),
    )
    .await?
    .expect("test manager should build");
    let authority = RuntimeLeaseAuthority::manager_owner(Arc::clone(&manager));
    let turn_selection = {
        let mut manager = manager.lock().await;
        manager
            .prepare_turn()
            .await?
            .expect("outer turn should acquire a pooled lease")
    };
    let request_context = LeaseRequestContext::for_test(
        RequestBoundaryKind::ResponsesCompact,
        "session-turn-unauthorized-report",
        CollaborationTreeId::for_test("tree-turn-unauthorized-report"),
    );

    let admission = authority
        .acquire_request_lease_for_test(request_context)
        .await?;
    assert_eq!(admission.snapshot.account_id(), "acct-turn-a");
    assert_eq!(admission.snapshot.generation(), turn_selection.generation);

    authority
        .report_terminal_unauthorized(&admission.snapshot)
        .await?;

    drop(admission.guard);

    let next_selection = {
        let mut manager = manager.lock().await;
        manager
            .prepare_turn()
            .await?
            .expect("next turn should rotate after terminal unauthorized")
    };
    assert_eq!(next_selection.account_id, "acct-turn-b");
    assert!(next_selection.generation > turn_selection.generation);

    Ok(())
}

#[tokio::test]
async fn manager_owner_acquire_honors_cancellation_while_waiting_for_manager_lock()
-> anyhow::Result<()> {
    let codex_home = tempfile::tempdir()?;
    let state_db =
        codex_state::StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into())
            .await?;
    let manager = SessionServices::build_account_pool_manager(
        Some(state_db),
        Some(test_accounts_config()),
        codex_home.path().to_path_buf(),
        "holder-legacy-cancel".to_string(),
    )
    .await?
    .expect("test manager should build");
    let authority = RuntimeLeaseAuthority::manager_owner(Arc::clone(&manager));
    let token = CancellationToken::new();
    let request_context = LeaseRequestContext::for_test_with_cancel(
        RequestBoundaryKind::ResponsesHttp,
        "session-a",
        CollaborationTreeId::for_test("tree-a"),
        token.clone(),
    );
    let _held_manager_lock = manager.lock().await;

    let acquire = tokio::spawn({
        let authority = authority.clone();
        async move {
            authority
                .acquire_request_lease_for_test(request_context)
                .await
        }
    });

    tokio::task::yield_now().await;
    assert!(!acquire.is_finished());

    token.cancel();

    let err = tokio::time::timeout(Duration::from_secs(1), acquire)
        .await
        .expect("cancelled acquire should finish without waiting for the manager lock")?
        .expect_err("cancelled acquire should return a typed cancellation error");
    assert_eq!(err, LeaseAdmissionError::Cancelled);

    Ok(())
}

#[tokio::test]
async fn cancelled_draining_acquire_returns_typed_cancellation() {
    let authority = RuntimeLeaseAuthority::for_test_draining("acct-a", 11);
    let token = tokio_util::sync::CancellationToken::new();
    let request_context = LeaseRequestContext::for_test_with_cancel(
        RequestBoundaryKind::ResponsesHttp,
        "session-a",
        CollaborationTreeId::for_test("tree-a"),
        token.clone(),
    );
    token.cancel();

    let err = authority
        .acquire_request_lease_for_test(request_context)
        .await
        .unwrap_err();

    assert_eq!(err, LeaseAdmissionError::Cancelled);
}

#[tokio::test]
async fn runtime_lease_host_rejects_republishing_authority_until_stale_sessions_detach()
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

    runtime_lease_host.install_manager_owner(Arc::clone(&first_manager))?;
    runtime_lease_host.attach_session("session-root-old").await;
    runtime_lease_host.attach_session("session-child-old").await;

    let err = runtime_lease_host
        .install_manager_owner(Arc::clone(&second_manager))
        .expect_err("a later root must not silently replace published authority");
    assert!(
        err.to_string()
            .contains("already has published pooled authority"),
        "unexpected error: {err:#}"
    );
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        2
    );
    assert!(runtime_lease_host.pooled_authority().is_some());
    let _ = first_manager;

    runtime_lease_host
        .detach_session("session-root-old")
        .await?;
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        1
    );
    assert!(runtime_lease_host.pooled_authority().is_some());

    runtime_lease_host
        .detach_session("session-child-old")
        .await?;
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    assert!(runtime_lease_host.pooled_authority().is_none());

    runtime_lease_host.install_manager_owner(Arc::clone(&second_manager))?;
    runtime_lease_host.attach_session("session-root-new").await;

    assert!(runtime_lease_host.pooled_authority().is_some());

    Ok(())
}

#[tokio::test]
async fn runtime_lease_host_failed_final_release_stays_retryable_until_authority_clears()
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
    runtime_lease_host.install_manager_owner(Arc::clone(&manager))?;
    runtime_lease_host.attach_session("session-retry").await;

    let first_release = runtime_lease_host.detach_session("session-retry").await;
    assert!(first_release.is_err());
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        1,
        "the final session must remain attached when release fails"
    );
    assert!(runtime_lease_host.pooled_authority().is_some());

    std::fs::remove_dir(&lease_epoch_marker)?;

    runtime_lease_host.detach_session("session-retry").await?;

    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    assert!(runtime_lease_host.pooled_authority().is_none());
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
    runtime_lease_host.install_manager_owner(Arc::clone(&manager))?;
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
    assert!(runtime_lease_host.pooled_authority().is_none());
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
async fn pending_inherited_child_startup_keeps_authority_until_success_or_rollback()
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

    runtime_lease_host.install_manager_owner(Arc::clone(&manager))?;
    runtime_lease_host.attach_session("session-parent").await;
    let child_startup = runtime_lease_host
        .try_reserve_startup_for_child("session-child-startup")
        .await?;

    runtime_lease_host.detach_session("session-parent").await?;

    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    assert_eq!(runtime_lease_host.pending_startup_count_for_test().await, 1);
    assert!(runtime_lease_host.pooled_authority().is_some());

    child_startup
        .promote_to_session("session-child")
        .await
        .expect("successful child startup should attach the child session");

    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        1
    );
    assert_eq!(runtime_lease_host.pending_startup_count_for_test().await, 0);
    assert!(runtime_lease_host.pooled_authority().is_some());

    runtime_lease_host.detach_session("session-child").await?;
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    assert!(runtime_lease_host.pooled_authority().is_none());

    runtime_lease_host.install_manager_owner(Arc::clone(&manager))?;
    runtime_lease_host
        .attach_session("session-parent-rollback")
        .await;
    let child_startup = runtime_lease_host
        .try_reserve_startup_for_child("session-child-rollback")
        .await?;

    runtime_lease_host
        .detach_session("session-parent-rollback")
        .await?;

    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    assert_eq!(runtime_lease_host.pending_startup_count_for_test().await, 1);
    assert!(runtime_lease_host.pooled_authority().is_some());

    child_startup
        .rollback()
        .await
        .expect("failed child startup should release the last reserved authority");

    assert_eq!(runtime_lease_host.pending_startup_count_for_test().await, 0);
    assert!(runtime_lease_host.pooled_authority().is_none());

    Ok(())
}

#[tokio::test]
async fn child_startup_reservation_rejects_host_after_parent_final_detach_clears_authority()
-> anyhow::Result<()> {
    let codex_home = tempfile::tempdir()?;
    let state_db =
        codex_state::StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into())
            .await?;
    let runtime_lease_host = RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new(
        "runtime-stale-child-startup".to_string(),
    ));
    let manager = SessionServices::build_account_pool_manager(
        Some(state_db),
        Some(test_accounts_config()),
        codex_home.path().to_path_buf(),
        "holder-stale-child-startup".to_string(),
    )
    .await?
    .expect("test manager should build");

    runtime_lease_host.install_manager_owner(Arc::clone(&manager))?;
    runtime_lease_host.attach_session("session-parent").await;
    runtime_lease_host.detach_session("session-parent").await?;

    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    assert!(runtime_lease_host.pooled_authority().is_none());

    let err = runtime_lease_host
        .try_reserve_startup_for_child("session-child-after-parent-detach")
        .await
        .expect_err("stale pooled host must reject child startup reservation");

    assert!(
        err.to_string()
            .contains("has no published pooled authority"),
        "unexpected error: {err:#}"
    );
    assert_eq!(runtime_lease_host.pending_startup_count_for_test().await, 0);

    Ok(())
}

#[tokio::test]
async fn startup_rollback_retries_transient_final_release_failure_before_clearing_authority()
-> anyhow::Result<()> {
    let codex_home = tempfile::tempdir()?;
    let config = build_test_config_with_pool(codex_home.path()).await;
    let state_db =
        codex_state::StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into())
            .await?;
    let account_id = "acct-startup-rollback-retry";
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
        "holder-startup-rollback-retry".to_string(),
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
        "runtime-startup-rollback-retry".to_string(),
    ));
    runtime_lease_host.install_manager_owner(Arc::clone(&manager))?;
    let child_startup = runtime_lease_host
        .try_reserve_startup_for_child("session-child-rollback-retry")
        .await?;

    let release_blocker = lease_epoch_marker.clone();
    let unblock_release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        std::fs::remove_dir(&release_blocker)
    });

    child_startup.rollback().await?;
    unblock_release.await??;

    assert_eq!(runtime_lease_host.pending_startup_count_for_test().await, 0);
    assert!(runtime_lease_host.pooled_authority().is_none());
    assert!(
        state_db
            .read_active_holder_lease("holder-startup-rollback-retry")
            .await?
            .is_none(),
        "startup rollback retry should release the active holder lease"
    );

    Ok(())
}

#[tokio::test]
async fn dropped_startup_reservation_cleans_pending_startup_and_allows_authority_reuse()
-> anyhow::Result<()> {
    let codex_home = tempfile::tempdir()?;
    let state_db =
        codex_state::StateRuntime::init(codex_home.path().to_path_buf(), "mock_provider".into())
            .await?;
    let runtime_lease_host = RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new(
        "runtime-dropped-startup".to_string(),
    ));
    let first_manager = SessionServices::build_account_pool_manager(
        Some(state_db.clone()),
        Some(test_accounts_config()),
        codex_home.path().to_path_buf(),
        "holder-dropped-startup-old".to_string(),
    )
    .await?
    .expect("test manager should build");

    runtime_lease_host.install_manager_owner(Arc::clone(&first_manager))?;
    runtime_lease_host.attach_session("session-parent").await;
    let child_startup = runtime_lease_host
        .try_reserve_startup_for_child("session-child-dropped-startup")
        .await?;

    runtime_lease_host.detach_session("session-parent").await?;
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    assert_eq!(runtime_lease_host.pending_startup_count_for_test().await, 1);
    assert!(runtime_lease_host.pooled_authority().is_some());

    drop(child_startup);

    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if runtime_lease_host.pending_startup_count_for_test().await == 0
                && runtime_lease_host.pooled_authority().is_none()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("dropped startup reservation should schedule cleanup");

    let second_manager = SessionServices::build_account_pool_manager(
        Some(state_db),
        Some(test_accounts_config()),
        codex_home.path().to_path_buf(),
        "holder-dropped-startup-new".to_string(),
    )
    .await?
    .expect("replacement manager should build");
    runtime_lease_host.install_manager_owner(Arc::clone(&second_manager))?;
    runtime_lease_host.attach_session("session-reused").await;

    assert!(runtime_lease_host.pooled_authority().is_some());

    runtime_lease_host.detach_session("session-reused").await?;
    Ok(())
}

#[tokio::test]
async fn pooled_sessions_keep_remote_context_continuity_independent() -> anyhow::Result<()> {
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
    assert!(root.session.services.account_pool_manager.is_none());
    let root_runtime_lease_host = root
        .session
        .services
        .runtime_lease_host
        .as_ref()
        .expect("root runtime lease host");
    assert!(root_runtime_lease_host.pooled_authority().is_some());
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
    let child_admitted = child
        .session
        .services
        .model_client
        .admitted_client_setup(
            RequestBoundaryKind::ResponsesHttp,
            None,
            "runtime-lease-child-request",
            CancellationToken::new(),
        )
        .await?;
    assert_eq!(
        child_admitted
            .reporter
            .as_ref()
            .map(|reporter| reporter.snapshot().account_id().to_string()),
        Some(account_id.to_string())
    );
    assert_eq!(
        child_admitted.setup.api_auth.account_id.as_deref(),
        Some(account_id)
    );
    assert_eq!(
        child_admitted.setup.api_auth.token.as_deref(),
        Some(fake_access_token(account_id).as_str())
    );

    child.shutdown_and_wait().await?;
    root.shutdown_and_wait().await?;
    Ok(())
}

#[tokio::test]
async fn codex_spawn_rejects_inherited_pooled_runtime_host_without_published_authority()
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
        config_codex_home,
        /*bundled_skills_enabled*/ true,
    ));
    let skills_watcher = Arc::new(SkillsWatcher::noop());
    let runtime_lease_host = RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new(
        "runtime-missing-authority-child".to_string(),
    ));

    let spawn_result = Codex::spawn(CodexSpawnArgs {
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
        runtime_lease_host: Some(runtime_lease_host.clone()),
        user_shell_override: None,
        parent_trace: None,
        analytics_events_client: None,
    })
    .await;
    let err = match spawn_result {
        Ok(spawned) => {
            spawned.codex.shutdown_and_wait().await?;
            panic!("inherited pooled runtime host without authority should fail child spawn");
        }
        Err(err) => err,
    };

    assert!(
        err.to_string()
            .contains("inherited pooled runtime host is not usable for child session"),
        "unexpected error: {err:#}"
    );
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    assert_eq!(runtime_lease_host.pending_startup_count_for_test().await, 0);

    Ok(())
}

#[tokio::test]
async fn pooled_host_child_keeps_authority_owned_lease_until_last_session_shutdown()
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

    let child_admitted = child
        .session
        .services
        .model_client
        .admitted_client_setup(
            RequestBoundaryKind::ResponsesHttp,
            None,
            "runtime-lease-child-lifecycle-request",
            CancellationToken::new(),
        )
        .await?;
    assert_eq!(
        child_admitted
            .reporter
            .as_ref()
            .map(|reporter| reporter.snapshot().account_id().to_string()),
        Some(account_id.to_string())
    );

    let snapshot_before_parent_shutdown = child
        .account_lease_snapshot()
        .await
        .expect("child should expose pooled lease snapshot before parent shutdown");
    assert!(snapshot_before_parent_shutdown.active);
    assert_eq!(
        snapshot_before_parent_shutdown.account_id.as_deref(),
        Some(account_id)
    );

    root.shutdown_and_wait().await?;

    let snapshot_after_parent_shutdown = child
        .account_lease_snapshot()
        .await
        .expect("child should keep the pooled lease after parent shutdown");
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
    let contender_admitted = contender
        .session
        .services
        .model_client
        .admitted_client_setup(
            RequestBoundaryKind::ResponsesHttp,
            None,
            "runtime-lease-contender-request",
            CancellationToken::new(),
        )
        .await?;
    assert_eq!(
        contender_admitted
            .reporter
            .as_ref()
            .map(|reporter| reporter.snapshot().account_id().to_string()),
        Some(account_id.to_string())
    );

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
    assert!(runtime_lease_host.pooled_authority().is_none());

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

    assert!(runtime_lease_host.pooled_authority().is_some());
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        1
    );
    assert!(recovered.session.services.account_pool_manager.is_none());

    recovered.shutdown_and_wait().await?;
    Ok(())
}

#[tokio::test]
async fn root_startup_reuses_existing_runtime_authority_when_host_is_explicitly_shared()
-> anyhow::Result<()> {
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
    assert!(runtime_lease_host.pooled_authority().is_some());
    assert!(first_root.session.services.account_pool_manager.is_none());
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
    .await?
    .codex;
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        2,
        "shared pooled host should track both attached sessions"
    );
    assert!(runtime_lease_host.pooled_authority().is_some());
    assert!(second_root.session.services.account_pool_manager.is_none());

    first_root.shutdown_and_wait().await?;
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        1
    );
    assert!(runtime_lease_host.pooled_authority().is_some());

    second_root.shutdown_and_wait().await?;
    assert_eq!(
        runtime_lease_host.attached_session_count_for_test().await,
        0
    );
    assert!(runtime_lease_host.pooled_authority().is_none());
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
    assert!(root.session.services.account_pool_manager.is_none());
    assert!(root_runtime_lease_host.pooled_authority().is_some());

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

    assert!(root.session.services.account_pool_manager.is_none());
    assert!(root_runtime_lease_host.pooled_authority().is_some());
    assert!(child.session.services.account_pool_manager.is_none());
    let child_admitted = child
        .session
        .services
        .model_client
        .admitted_client_setup(
            RequestBoundaryKind::ResponsesHttp,
            None,
            "runtime-lease-config-only-child-request",
            CancellationToken::new(),
        )
        .await?;
    assert_eq!(
        child_admitted
            .reporter
            .as_ref()
            .map(|reporter| reporter.snapshot().account_id().to_string()),
        Some(account_id.to_string())
    );
    assert_eq!(
        child_admitted.setup.api_auth.account_id.as_deref(),
        Some(account_id)
    );

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
