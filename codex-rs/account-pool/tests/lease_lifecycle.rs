#![allow(clippy::expect_used)]

use base64::Engine;
use chrono::Duration;
use chrono::Utc;
use codex_account_pool::AccountPoolConfig;
use codex_account_pool::AccountPoolControlPlane;
use codex_account_pool::AccountPoolExecutionBackend;
use codex_account_pool::AccountPoolManager;
use codex_account_pool::HealthEventDisposition;
use codex_account_pool::LeaseGrant;
use codex_account_pool::LegacyAuthBootstrap;
use codex_account_pool::LocalAccountPoolBackend;
use codex_account_pool::NoLegacyAuthBootstrap;
use codex_account_pool::ProbeOutcome;
use codex_account_pool::ProbeReservation;
use codex_account_pool::RateLimitSnapshot;
use codex_account_pool::RegisteredAccountRegistration;
use codex_account_pool::SelectionAction;
use codex_account_pool::SelectionIntent;
use codex_account_pool::SelectionRequest;
use codex_account_pool::UsageLimitEvent;
use codex_account_pool::read_shared_startup_status;
use codex_login::AuthCredentialsStoreMode;
use codex_login::AuthDotJson;
use codex_login::ChatgptManagedRegistrationTokens;
use codex_login::CodexAuth;
use codex_login::TokenData;
use codex_login::save_auth;
use codex_login::token_data::parse_chatgpt_jwt_claims;
use codex_state::AccountHealthEvent;
use codex_state::AccountLeaseError;
use codex_state::AccountQuotaStateRecord;
use codex_state::AccountRegistryEntryUpdate;
use codex_state::AccountStartupAvailability;
use codex_state::AccountStartupResolutionIssueKind;
use codex_state::AccountStartupSelectionState;
use codex_state::AccountStartupStatus;
use codex_state::EffectivePoolResolutionSource;
use codex_state::LeaseRenewal;
use codex_state::LegacyAccountImport;
use codex_state::QuotaExhaustedWindows;
use codex_state::QuotaProbeResult;
use codex_state::RegisteredAccountMembership;
use codex_state::RegisteredAccountUpsert;
use codex_state::StateRuntime;
use pretty_assertions::assert_eq;
use std::fs;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use tempfile::TempDir;

#[tokio::test]
async fn ensure_active_lease_reuses_sticky_account_until_threshold() {
    let harness = fixture_with_registered_account("acct-legacy").await;
    let mut manager = harness
        .manager("test-holder", default_config())
        .expect("create manager");
    let first = manager
        .ensure_active_lease(SelectionRequest::default())
        .await
        .expect("acquire initial lease");

    manager
        .report_rate_limits(first.key(), snapshot(/* used_percent */ 70.0))
        .await
        .expect("record below-threshold snapshot");

    let second = manager
        .ensure_active_lease(SelectionRequest::default())
        .await
        .expect("reuse sticky lease");

    assert_eq!(first.account_id(), second.account_id());
}

#[tokio::test]
async fn stale_holder_health_event_is_ignored_after_epoch_bump() {
    let harness = fixture_with_registered_account("acct-legacy").await;
    let mut manager = harness
        .manager("test-holder", default_config())
        .expect("create manager");
    let lease = manager
        .ensure_active_lease(SelectionRequest::default())
        .await
        .expect("acquire initial lease");
    manager
        .force_epoch_bump_for_test(lease.account_id())
        .expect("bump lease epoch");

    let result = manager
        .report_usage_limit_reached(lease.key(), usage_limit_event())
        .await;

    assert_eq!(
        result.expect("report stale health event"),
        HealthEventDisposition::IgnoredAsStale
    );
}

#[tokio::test]
async fn shared_startup_status_treats_invisible_config_default_as_invalid() {
    let harness = fixture_with_legacy_auth("acct-legacy").await;
    let backend = LocalAccountPoolBackend::new(
        harness.runtime.clone(),
        default_config().lease_ttl_duration(),
    );

    let status = read_shared_startup_status(&backend, Some("configured-main"), None)
        .await
        .expect("read shared startup status");

    assert_eq!(status.pooled_applicable, false);
    assert_eq!(
        status.startup.effective_pool_resolution_source,
        EffectivePoolResolutionSource::ConfigDefault
    );
    assert_eq!(
        status.startup.configured_default_pool_id.as_deref(),
        Some("configured-main")
    );
    assert_eq!(status.startup.persisted_default_pool_id, None);
    assert_eq!(
        status.startup.startup_availability,
        AccountStartupAvailability::InvalidExplicitDefault
    );
    assert_eq!(
        status
            .startup
            .startup_resolution_issue
            .as_ref()
            .map(|issue| issue.kind),
        Some(AccountStartupResolutionIssueKind::ConfigDefaultPoolUnavailable)
    );
}

#[tokio::test]
async fn shared_startup_status_treats_explicit_override_as_pooled_applicable() {
    let harness = fixture_with_registered_account("acct-legacy").await;
    let backend = LocalAccountPoolBackend::new(
        harness.runtime.clone(),
        default_config().lease_ttl_duration(),
    );

    let status =
        read_shared_startup_status(&backend, Some("configured-main"), Some("legacy-default"))
            .await
            .expect("read shared startup status");

    assert_eq!(status.pooled_applicable, true);
    assert_eq!(
        status.startup.effective_pool_resolution_source,
        EffectivePoolResolutionSource::Override
    );
    assert_eq!(
        status.startup.configured_default_pool_id.as_deref(),
        Some("configured-main")
    );
    assert_eq!(
        status.startup.preview.effective_pool_id.as_deref(),
        Some("legacy-default")
    );
}

#[tokio::test]
async fn soft_pressure_before_min_interval_does_not_persist_rate_limited_health() {
    let harness = fixture_with_registered_accounts(&["acct-a", "acct-b"]).await;
    let mut manager = harness
        .manager("test-holder", damping_config())
        .expect("create manager");
    let first = manager
        .ensure_active_lease(SelectionRequest::default())
        .await
        .expect("acquire initial lease");

    manager
        .report_rate_limits(
            first.key(),
            snapshot_at(
                /* used_percent */ 95.0,
                first.acquired_at() + Duration::seconds(30),
            ),
        )
        .await
        .expect("record soft pressure");

    let second = manager
        .ensure_active_lease(SelectionRequest {
            now: Some(first.acquired_at() + Duration::seconds(60)),
            pool_id: None,
            ..SelectionRequest::default()
        })
        .await
        .expect("reuse sticky lease");

    assert_eq!(first.account_id(), second.account_id());
    assert_eq!(
        harness
            .runtime
            .read_account_health_event_sequence(first.account_id())
            .await
            .expect("read health sequence"),
        None
    );
}

#[tokio::test]
async fn stale_soft_pressure_does_not_force_delayed_rotation_after_window_opens() {
    let harness = fixture_with_registered_accounts(&["acct-a", "acct-b"]).await;
    let mut manager = harness
        .manager("test-holder", damping_config())
        .expect("create manager");
    let first = manager
        .ensure_active_lease(SelectionRequest::default())
        .await
        .expect("acquire initial lease");

    manager
        .report_rate_limits(
            first.key(),
            snapshot_at(
                /* used_percent */ 95.0,
                first.acquired_at() + Duration::seconds(30),
            ),
        )
        .await
        .expect("record soft pressure");

    let same = manager
        .ensure_active_lease(SelectionRequest {
            now: Some(first.acquired_at() + Duration::seconds(121)),
            pool_id: None,
            ..SelectionRequest::default()
        })
        .await
        .expect("stale soft pressure should not rotate");

    assert_eq!(same.account_id(), first.account_id());
    assert_eq!(
        harness
            .runtime
            .read_account_health_event_sequence(first.account_id())
            .await
            .expect("read health sequence"),
        None
    );
}

#[tokio::test]
async fn hard_usage_limit_bypasses_min_switch_interval() {
    let harness = fixture_with_registered_accounts(&["acct-a", "acct-b"]).await;
    let mut manager = harness
        .manager("test-holder", damping_config())
        .expect("create manager");
    let first = manager
        .ensure_active_lease(SelectionRequest::default())
        .await
        .expect("acquire initial lease");

    manager
        .report_usage_limit_reached(
            first.key(),
            usage_limit_event_at(first.acquired_at() + Duration::seconds(30)),
        )
        .await
        .expect("record hard limit");

    let rotated = manager
        .ensure_active_lease(SelectionRequest {
            now: Some(first.acquired_at() + Duration::seconds(31)),
            pool_id: None,
            ..SelectionRequest::default()
        })
        .await
        .expect("hard failure should rotate immediately");

    assert_ne!(rotated.account_id(), first.account_id());
    assert_eq!(
        harness
            .runtime
            .read_account_health_event_sequence(first.account_id())
            .await
            .expect("read health sequence"),
        Some(1)
    );
}

#[tokio::test]
async fn proactive_rotation_avoids_just_replaced_account_when_another_candidate_exists() {
    let harness = fixture_with_registered_accounts(&["acct-a", "acct-b", "acct-c"]).await;
    let mut manager = harness
        .manager("test-holder", damping_config())
        .expect("create manager");
    let first = manager
        .ensure_active_lease(SelectionRequest::default())
        .await
        .expect("acquire initial lease");
    assert_eq!(first.account_id(), "acct-a");

    manager
        .report_rate_limits(
            first.key(),
            snapshot_at(
                /* used_percent */ 95.0,
                first.acquired_at() + Duration::seconds(121),
            ),
        )
        .await
        .expect("record proactive pressure for first account");

    let second = manager
        .ensure_active_lease(SelectionRequest {
            now: Some(first.acquired_at() + Duration::seconds(122)),
            pool_id: None,
            ..SelectionRequest::default()
        })
        .await
        .expect("rotate to second account");
    assert_eq!(second.account_id(), "acct-b");

    manager
        .report_rate_limits(
            second.key(),
            snapshot_at(
                /* used_percent */ 95.0,
                second.acquired_at() + Duration::seconds(121),
            ),
        )
        .await
        .expect("record proactive pressure for second account");

    let third = manager
        .ensure_active_lease(SelectionRequest {
            now: Some(second.acquired_at() + Duration::seconds(122)),
            pool_id: None,
            ..SelectionRequest::default()
        })
        .await
        .expect("rotate to third account");

    assert_eq!(third.account_id(), "acct-c");
}

#[tokio::test]
async fn non_proactive_replacement_clears_just_replaced_exclusion() {
    let harness = fixture_with_registered_accounts(&["acct-a", "acct-b", "acct-c", "acct-d"]).await;
    let mut manager = harness
        .manager("test-holder", damping_config())
        .expect("create manager");
    let first = manager
        .ensure_active_lease(SelectionRequest::default())
        .await
        .expect("acquire initial lease");
    assert_eq!(first.account_id(), "acct-a");

    manager
        .report_rate_limits(
            first.key(),
            snapshot_at(
                /* used_percent */ 95.0,
                first.acquired_at() + Duration::seconds(121),
            ),
        )
        .await
        .expect("record proactive pressure for first account");

    let second = manager
        .ensure_active_lease(SelectionRequest {
            now: Some(first.acquired_at() + Duration::seconds(122)),
            pool_id: None,
            ..SelectionRequest::default()
        })
        .await
        .expect("rotate to second account");
    assert_eq!(second.account_id(), "acct-b");

    harness
        .runtime
        .upsert_account_registry_entry(registry_entry_update("acct-a", /* enabled */ false))
        .await
        .expect("disable first account before hard rotation");

    manager
        .report_usage_limit_reached(
            second.key(),
            usage_limit_event_at(second.acquired_at() + Duration::seconds(30)),
        )
        .await
        .expect("record hard failure on second account");

    let third = manager
        .ensure_active_lease(SelectionRequest {
            now: Some(second.acquired_at() + Duration::seconds(31)),
            pool_id: None,
            ..SelectionRequest::default()
        })
        .await
        .expect("hard failure should rotate to next eligible account");
    assert_eq!(third.account_id(), "acct-c");

    harness
        .runtime
        .upsert_account_registry_entry(registry_entry_update("acct-a", /* enabled */ true))
        .await
        .expect("re-enable first account");

    manager
        .report_rate_limits(
            third.key(),
            snapshot_at(
                /* used_percent */ 95.0,
                third.acquired_at() + Duration::seconds(121),
            ),
        )
        .await
        .expect("record proactive pressure for third account");

    let fourth = manager
        .ensure_active_lease(SelectionRequest {
            now: Some(third.acquired_at() + Duration::seconds(122)),
            pool_id: None,
            ..SelectionRequest::default()
        })
        .await
        .expect("proactive rotation should not retain stale exclusion");

    assert_eq!(fourth.account_id(), "acct-a");
}

mod lease_lifecycle {
    use pretty_assertions::assert_eq;

    use super::*;

    #[tokio::test]
    async fn ensure_active_lease_does_not_bootstrap_legacy_auth_when_startup_state_is_empty() {
        let harness = fixture_with_legacy_auth("acct-legacy").await;
        let mut manager = harness
            .manager("test-holder", default_config())
            .expect("create manager");

        let err = manager
            .ensure_active_lease(SelectionRequest::default())
            .await
            .expect_err("legacy auth should not bootstrap pooled state implicitly");

        assert!(
            err.to_string().contains("default is unavailable"),
            "unexpected error: {err}"
        );

        let selection = manager
            .read_startup_selection_for_test()
            .await
            .expect("read startup selection");

        assert_eq!(selection.default_pool_id, None);
        assert_eq!(selection.preferred_account_id, None);
        assert_eq!(selection.suppressed, false);
        assert_eq!(harness.bootstrap_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn startup_selection_facts_keep_any_eligible_account_true_when_preferred_is_busy() {
        let harness = fixture_with_registered_accounts(&["acct-a", "acct-b"]).await;
        harness
            .runtime
            .write_account_startup_selection(codex_state::AccountStartupSelectionUpdate {
                default_pool_id: Some("legacy-default".to_string()),
                preferred_account_id: Some("acct-a".to_string()),
                suppressed: false,
            })
            .await
            .expect("write startup selection");
        harness
            .runtime
            .acquire_preferred_account_lease(
                "legacy-default",
                "acct-a",
                "codex",
                "holder-1",
                Duration::seconds(300),
            )
            .await
            .expect("acquire preferred account lease");

        let backend = LocalAccountPoolBackend::new(
            harness.runtime.clone(),
            default_config().lease_ttl_duration(),
        );
        let facts = backend
            .read_startup_selection_facts("legacy-default")
            .await
            .expect("read startup selection facts");

        assert_eq!(facts.any_eligible_account, true);
        assert_eq!(facts.predicted_account_id.as_deref(), Some("acct-b"));
    }

    #[tokio::test]
    async fn lease_lifecycle_register_account_persists_backend_private_auth_for_pooled_accounts() {
        register_account_persists_backend_private_auth_for_pooled_accounts()
            .await
            .expect("register account should persist backend-private auth");
    }

    #[tokio::test]
    async fn lease_lifecycle_register_account_encodes_slash_account_id_for_backend_private_auth() {
        register_account_encodes_slash_account_id_for_backend_private_auth()
            .await
            .expect("slash account id should be encoded for backend-private auth");
    }

    #[tokio::test]
    async fn lease_lifecycle_register_account_preserves_existing_backend_private_auth_on_conflict()
    {
        register_account_preserves_existing_backend_private_auth_on_conflict()
            .await
            .expect("conflicting registration should not clear existing auth");
    }

    #[tokio::test]
    async fn lease_lifecycle_register_account_reuses_encoded_backend_handle_on_reregister() {
        register_account_reuses_encoded_backend_handle_on_reregister()
            .await
            .expect("re-registering the same provider identity should reuse the encoded handle");
    }

    #[tokio::test]
    async fn lease_lifecycle_register_account_removes_legacy_raw_backend_private_auth_on_reregister()
     {
        register_account_removes_legacy_raw_backend_private_auth_on_reregister()
            .await
            .expect("re-registering should remove the legacy raw backend-private auth namespace");
    }

    #[tokio::test]
    async fn lease_lifecycle_register_account_returns_actual_persisted_row_for_existing_account() {
        register_account_returns_actual_persisted_row_for_existing_account()
            .await
            .expect("register account should return the persisted row");
    }

    #[tokio::test]
    async fn lease_lifecycle_stale_session_fails_after_epoch_supersession() {
        stale_lease_scoped_session_fails_after_epoch_supersession()
            .await
            .expect("stale session should fail after epoch supersession");
    }

    #[tokio::test]
    async fn acquire_lease_releases_lease_when_marker_write_fails() {
        super::acquire_lease_releases_lease_when_marker_write_fails()
            .await
            .expect("failed acquisition should be compensated");
    }

    #[tokio::test]
    async fn lease_lifecycle_register_account_cleans_new_backend_private_auth_on_persistence_failure()
     {
        register_account_cleans_new_backend_private_auth_on_persistence_failure()
            .await
            .expect("failed registration should clean up new backend-private auth");
    }

    #[tokio::test]
    async fn acquire_lease_prefers_primary_safe_account_over_lower_position_blocked_account() {
        let harness = quota_fixture_with_three_accounts().await;
        harness
            .write_quota("acct-a", "codex", exhausted_secondary())
            .await
            .expect("write blocked quota");
        harness
            .write_quota("acct-b", "codex", healthy_primary(44.0))
            .await
            .expect("write healthy quota");

        let lease = harness
            .acquire_runtime_selected_lease(runtime_selection_request())
            .await
            .expect("acquire runtime-selected lease");

        assert_eq!(lease.account_id(), "acct-b");
    }

    #[tokio::test]
    async fn runtime_selection_uses_requested_family_before_consulting_codex_fallback() {
        let harness = quota_fixture_with_three_accounts().await;
        harness
            .write_quota("acct-a", "codex", healthy_primary(5.0))
            .await
            .expect("write codex fallback quota");
        harness
            .write_quota("acct-a", "chatgpt", exhausted_primary())
            .await
            .expect("write requested-family blocked quota");
        harness
            .write_quota("acct-b", "chatgpt", healthy_primary(42.0))
            .await
            .expect("write requested-family healthy quota");

        let lease = harness
            .acquire_runtime_selected_lease(
                runtime_selection_request().with_selection_family("chatgpt"),
            )
            .await
            .expect("acquire requested-family lease");

        assert_eq!(lease.account_id(), "acct-b");
    }

    #[tokio::test]
    async fn preferred_lease_acquisition_uses_requested_family_instead_of_backend_family_fallback()
    {
        let harness = quota_fixture_with_three_accounts().await;
        harness
            .write_quota("acct-a", "codex", exhausted_primary())
            .await
            .expect("write blocked codex fallback quota");
        harness
            .write_quota("acct-a", "chatgpt", healthy_primary(5.0))
            .await
            .expect("write requested-family healthy quota");
        harness
            .write_quota("acct-b", "chatgpt", healthy_primary(42.0))
            .await
            .expect("write worse requested-family quota");
        let backend = LocalAccountPoolBackend::new(
            harness.runtime.clone(),
            default_config().lease_ttl_duration(),
        );
        let request = runtime_selection_request().with_selection_family("chatgpt");
        let (selection_family, plan) = backend
            .plan_runtime_selection(&request, "test-holder")
            .await
            .expect("plan requested-family selection");

        assert_eq!(selection_family, "chatgpt");
        assert_eq!(
            plan.terminal_action,
            SelectionAction::Select("acct-a".to_string())
        );

        let lease = backend
            .acquire_preferred_lease(
                "legacy-default",
                "acct-a",
                selection_family.as_str(),
                "test-holder",
            )
            .await
            .expect("acquire preferred lease for requested-family winner");

        assert_eq!(lease.account_id(), "acct-a");
    }

    #[tokio::test]
    async fn runtime_selection_ignores_legacy_registry_health_flag() {
        let harness = quota_fixture_with_three_accounts().await;
        harness
            .runtime
            .upsert_account_registry_entry(AccountRegistryEntryUpdate {
                healthy: false,
                ..registry_entry_update("acct-a", /* enabled */ true)
            })
            .await
            .expect("mark account unhealthy in legacy registry field");
        harness
            .write_quota("acct-a", "codex", healthy_primary(5.0))
            .await
            .expect("write best quota");
        harness
            .write_quota("acct-b", "codex", healthy_primary(42.0))
            .await
            .expect("write fallback quota");

        let lease = harness
            .acquire_runtime_selected_lease(runtime_selection_request())
            .await
            .expect("acquire runtime-selected lease");

        assert_eq!(lease.account_id(), "acct-a");
    }

    #[tokio::test]
    async fn probe_reservation_uses_codex_fallback_when_requested_family_row_is_absent() {
        let harness = quota_fixture_with_three_accounts().await;
        harness
            .write_quota("acct-a", "codex", exhausted_primary())
            .await
            .expect("write codex fallback quota");
        let backend = LocalAccountPoolBackend::new(
            harness.runtime.clone(),
            default_config().lease_ttl_duration(),
        );
        let now = Utc::now();

        let reservation = backend
            .reserve_quota_probe("acct-a", "chatgpt", now, Duration::seconds(30))
            .await
            .expect("reserve fallback probe")
            .expect("probe reservation should succeed");

        let codex_quota = harness
            .runtime
            .read_account_quota_state("acct-a", "codex")
            .await
            .expect("read codex quota")
            .expect("codex quota should exist");
        assert_eq!(reservation.limit_id, "codex");
        assert_eq!(reservation.reserved_until, now + Duration::seconds(30));
        assert_eq!(
            harness
                .runtime
                .read_account_quota_state("acct-a", "chatgpt")
                .await
                .expect("read requested family quota"),
            None
        );
        assert!(
            codex_quota
                .next_probe_after
                .is_some_and(|next_probe_after| next_probe_after >= now),
            "codex fallback row should receive the reservation"
        );
    }

    #[tokio::test]
    async fn probe_acquire_uses_reserved_codex_fallback_when_requested_family_row_appears_later() {
        let harness = quota_fixture_with_three_accounts().await;
        harness
            .write_quota("acct-a", "codex", exhausted_primary())
            .await
            .expect("write codex fallback quota");
        let backend = LocalAccountPoolBackend::new(
            harness.runtime.clone(),
            default_config().lease_ttl_duration(),
        );
        let now = Utc::now();
        let reservation = backend
            .reserve_quota_probe("acct-a", "chatgpt", now, Duration::seconds(30))
            .await
            .expect("reserve fallback probe")
            .expect("probe reservation should succeed");
        harness
            .write_quota("acct-a", "chatgpt", exhausted_primary())
            .await
            .expect("write requested-family quota after reservation");

        let lease = backend
            .acquire_probe_lease("legacy-default", "acct-a", &reservation, "probe-holder")
            .await
            .expect("acquire probe lease through reserved fallback row");

        assert_eq!(lease.account_id(), "acct-a");
    }

    #[tokio::test]
    async fn probe_refresh_uses_reserved_codex_fallback_when_requested_family_row_appears_later() {
        let harness = quota_fixture_with_three_accounts().await;
        harness
            .write_quota("acct-a", "codex", exhausted_primary())
            .await
            .expect("write codex fallback quota");
        let backend = LocalAccountPoolBackend::new(
            harness.runtime.clone(),
            default_config().lease_ttl_duration(),
        );
        let now = Utc::now();
        let reservation = backend
            .reserve_quota_probe("acct-a", "chatgpt", now, Duration::seconds(30))
            .await
            .expect("reserve fallback probe")
            .expect("probe reservation should succeed");
        harness
            .write_quota("acct-a", "chatgpt", exhausted_primary())
            .await
            .expect("write requested-family quota after reservation");
        let lease = backend
            .acquire_probe_lease("legacy-default", "acct-a", &reservation, "probe-holder")
            .await
            .expect("acquire probe lease through reserved fallback row");
        let auth_home = harness
            .runtime
            .codex_home()
            .join(".pooled-auth/backends/local/accounts")
            .join(normalized_backend_account_handle("acct-a"));
        fs::remove_file(auth_home.join("lease_epoch")).expect("remove lease epoch marker");

        let outcome = backend
            .refresh_quota_probe(&lease, &reservation)
            .await
            .expect("refresh reserved fallback quota probe");

        let codex_quota = harness
            .runtime
            .read_account_quota_state("acct-a", "codex")
            .await
            .expect("read codex quota")
            .expect("codex quota should exist");
        let chatgpt_quota = harness
            .runtime
            .read_account_quota_state("acct-a", "chatgpt")
            .await
            .expect("read requested-family quota")
            .expect("requested-family quota should exist");
        assert_eq!(outcome, Some(ProbeOutcome::Ambiguous));
        assert_eq!(
            codex_quota.last_probe_result,
            Some(QuotaProbeResult::Ambiguous)
        );
        assert_eq!(chatgpt_quota.last_probe_result, None);
    }

    #[tokio::test]
    async fn probe_refresh_ignores_reserved_codex_fallback_after_row_is_replaced() {
        let harness = quota_fixture_with_three_accounts().await;
        harness
            .write_quota("acct-a", "codex", exhausted_primary())
            .await
            .expect("write codex fallback quota");
        let backend = LocalAccountPoolBackend::new(
            harness.runtime.clone(),
            default_config().lease_ttl_duration(),
        );
        let now = Utc::now();
        let reservation = backend
            .reserve_quota_probe("acct-a", "chatgpt", now, Duration::seconds(30))
            .await
            .expect("reserve fallback probe")
            .expect("probe reservation should succeed");
        let lease = backend
            .acquire_probe_lease("legacy-default", "acct-a", &reservation, "probe-holder")
            .await
            .expect("acquire probe lease through reserved fallback row");
        harness
            .write_quota("acct-a", "codex", exhausted_primary())
            .await
            .expect("replace reserved codex quota");
        let auth_home = harness
            .runtime
            .codex_home()
            .join(".pooled-auth/backends/local/accounts")
            .join(normalized_backend_account_handle("acct-a"));
        fs::remove_file(auth_home.join("lease_epoch")).expect("remove lease epoch marker");

        let outcome = backend
            .refresh_quota_probe(&lease, &reservation)
            .await
            .expect("ignore stale fallback quota probe");

        let codex_quota = harness
            .runtime
            .read_account_quota_state("acct-a", "codex")
            .await
            .expect("read codex quota")
            .expect("codex quota should exist");
        assert_eq!(outcome, None);
        assert_eq!(codex_quota.last_probe_result, None);
    }

    #[tokio::test]
    async fn probe_refresh_ambiguous_updates_codex_fallback_when_requested_family_row_is_absent() {
        let harness = quota_fixture_with_three_accounts().await;
        harness
            .write_quota("acct-a", "codex", exhausted_primary())
            .await
            .expect("write codex fallback quota");
        let backend = LocalAccountPoolBackend::new(
            harness.runtime.clone(),
            default_config().lease_ttl_duration(),
        );
        let now = Utc::now();
        let reservation = backend
            .reserve_quota_probe("acct-a", "chatgpt", now, Duration::seconds(30))
            .await
            .expect("reserve fallback probe")
            .expect("probe reservation should succeed");
        let lease = backend
            .acquire_probe_lease("legacy-default", "acct-a", &reservation, "probe-holder")
            .await
            .expect("acquire probe lease");
        let auth_home = harness
            .runtime
            .codex_home()
            .join(".pooled-auth/backends/local/accounts")
            .join(normalized_backend_account_handle("acct-a"));
        fs::remove_file(auth_home.join("lease_epoch")).expect("remove lease epoch marker");

        let outcome = backend
            .refresh_quota_probe(&lease, &reservation)
            .await
            .expect("refresh fallback quota probe");

        let codex_quota = harness
            .runtime
            .read_account_quota_state("acct-a", "codex")
            .await
            .expect("read codex quota")
            .expect("codex quota should exist");
        assert_eq!(outcome, Some(ProbeOutcome::Ambiguous));
        assert_eq!(
            harness
                .runtime
                .read_account_quota_state("acct-a", "chatgpt")
                .await
                .expect("read requested family quota"),
            None
        );
        assert_eq!(
            codex_quota.last_probe_result,
            Some(QuotaProbeResult::Ambiguous)
        );
        assert!(
            codex_quota.next_probe_after.is_some(),
            "codex fallback row should receive ambiguous probe backoff"
        );
    }

    #[tokio::test]
    async fn soft_rotation_probe_acquires_reserved_quota_exhausted_verification_lease() {
        let harness = fixture_with_registered_accounts(&["acct-current", "acct-probe"]).await;
        harness
            .write_quota("acct-probe", "codex", exhausted_primary())
            .await
            .expect("write probe quota");
        let mut config = default_config();
        config.min_switch_interval_secs = 0;
        let mut manager = harness
            .manager("test-holder", config)
            .expect("create manager");
        let current = manager
            .ensure_active_lease(runtime_selection_request())
            .await
            .expect("acquire current lease");
        manager
            .report_rate_limits(current.key(), snapshot(95.0))
            .await
            .expect("record soft pressure");

        let selected = manager
            .ensure_active_lease(SelectionRequest {
                now: Some(Utc::now()),
                pool_id: Some(current.pool_id().to_string()),
                intent: SelectionIntent::SoftRotation,
                selection_family: None,
                preferred_account_id: None,
                current_account_id: None,
                just_replaced_account_id: None,
                reserved_probe_target_account_id: None,
                proactive_threshold_percent: 85,
            })
            .await
            .expect("run soft-rotation probe");

        let probe_quota = harness
            .runtime
            .read_account_quota_state("acct-probe", "codex")
            .await
            .expect("read probe quota")
            .expect("probe quota should exist");
        assert_eq!(selected.account_id(), "acct-current");
        assert_eq!(
            probe_quota.last_probe_result,
            Some(QuotaProbeResult::StillBlocked)
        );
        assert_eq!(
            harness
                .runtime
                .read_active_holder_lease("test-holder:probe")
                .await
                .expect("read probe holder lease"),
            None
        );
    }

    #[tokio::test]
    async fn probe_reservation_is_left_in_place_when_verification_lease_loses_a_race() {
        let mut harness = ScriptedProbeHarness::new(ProbeScenario::Contention)
            .await
            .expect("create contention harness");

        let result = harness
            .force_probe_lease_contention()
            .await
            .expect("run contention scenario");

        assert_eq!(result, ProbeExecutionOutcome::SelectionRestartRequired);
        assert!(
            harness.probe_state().probe_reserved_until.is_some(),
            "probe reservation should remain in place after contention"
        );
    }

    #[tokio::test]
    async fn soft_rotation_probe_keeps_active_lease_and_releases_verification_lease_before_reselection()
     {
        let mut harness = ScriptedProbeHarness::new(ProbeScenario::Success)
            .await
            .expect("create success harness");

        let outcome = harness
            .trigger_soft_rotation_probe_success()
            .await
            .expect("run probe success scenario");

        assert_eq!(outcome.original_lease_account_id, "acct-current");
        assert_eq!(outcome.active_lease_retained_during_probe, true);
        assert_eq!(outcome.verification_lease_released, true);
        assert_eq!(outcome.final_selected_account_id, "acct-recovered");
        assert_eq!(harness.probe_state().probe_acquire_called, true);
    }

    #[tokio::test]
    async fn soft_rotation_probe_releases_verification_lease_when_refresh_is_still_blocked() {
        let mut harness = ScriptedProbeHarness::new(ProbeScenario::StillBlocked)
            .await
            .expect("create still-blocked harness");

        let result = harness
            .trigger_soft_rotation_probe_attempt()
            .await
            .expect("run probe still-blocked scenario");

        assert_eq!(result, Ok("acct-current".to_string()));
        assert_eq!(harness.probe_state().verification_released, true);
    }

    #[tokio::test]
    async fn soft_rotation_probe_releases_verification_lease_when_refresh_is_ambiguous() {
        let mut harness = ScriptedProbeHarness::new(ProbeScenario::Ambiguous)
            .await
            .expect("create ambiguous harness");

        let result = harness
            .trigger_soft_rotation_probe_attempt()
            .await
            .expect("run probe ambiguous scenario");

        assert_eq!(result, Ok("acct-current".to_string()));
        assert_eq!(harness.probe_state().verification_released, true);
    }

    #[tokio::test]
    async fn soft_rotation_probe_releases_verification_lease_when_refresh_errors() {
        let mut harness = ScriptedProbeHarness::new(ProbeScenario::RefreshError)
            .await
            .expect("create refresh-error harness");

        let result = harness
            .trigger_soft_rotation_probe_attempt()
            .await
            .expect("run probe refresh-error scenario");

        assert_eq!(result, Err("probe refresh failed".to_string()));
        assert_eq!(harness.probe_state().verification_released, true);
    }

    #[tokio::test]
    async fn soft_rotation_select_contention_replans_to_fresh_candidate() {
        let mut harness = ScriptedProbeHarness::new(ProbeScenario::OrdinarySelectContention)
            .await
            .expect("create ordinary-select contention harness");

        let result = harness
            .trigger_soft_rotation_probe_attempt()
            .await
            .expect("run ordinary-select contention scenario");
        let state = harness.probe_state();

        assert_eq!(result, Ok("acct-fallback".to_string()));
        assert!(state.normal_select_failed);
        assert_eq!(state.current_reacquired_after_select_failure, false);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn delete_registered_account_preserves_registry_when_backend_cleanup_fails() {
        super::delete_registered_account_preserves_registry_when_backend_cleanup_fails()
            .await
            .expect("backend cleanup failure should not drop local registry state");
    }
}

#[tokio::test]
async fn release_active_lease_persists_release_and_allows_immediate_reacquire() {
    let harness = fixture_with_registered_account("acct-legacy").await;
    let mut first = harness
        .manager("holder-a", default_config())
        .expect("create first manager");
    let lease = first
        .ensure_active_lease(SelectionRequest::default())
        .await
        .expect("acquire lease");

    first
        .release_active_lease()
        .await
        .expect("persist lease release");

    let mut second = harness
        .manager("holder-b", default_config())
        .expect("create second manager");
    let reacquired = second
        .ensure_active_lease(SelectionRequest::default())
        .await
        .expect("reacquire released lease");

    assert_eq!(reacquired.account_id(), lease.account_id());
}

#[tokio::test]
async fn rehydrated_existing_lease_seeds_next_health_event_sequence() {
    let harness = fixture_with_registered_account("acct-legacy").await;
    let mut first = harness
        .manager("holder-a", default_config())
        .expect("create first manager");
    let lease = first
        .ensure_active_lease(SelectionRequest::default())
        .await
        .expect("acquire lease");
    first
        .report_unauthorized(lease.key())
        .await
        .expect("record initial health event");

    let mut rehydrated = harness
        .manager("holder-a", default_config())
        .expect("create rehydrated manager");
    let same_lease = rehydrated
        .ensure_active_lease(SelectionRequest::default())
        .await
        .expect("rehydrate existing lease");
    rehydrated
        .report_unauthorized(same_lease.key())
        .await
        .expect("record newer health event");

    assert_eq!(
        harness
            .runtime
            .read_account_health_event_sequence("acct-legacy")
            .await
            .expect("read persisted health sequence"),
        Some(2)
    );
}

#[tokio::test]
async fn configured_lease_ttl_is_used_for_acquire_and_renew() {
    let harness = fixture_with_registered_account("acct-legacy").await;
    let config = AccountPoolConfig {
        lease_ttl_secs: 30,
        heartbeat_interval_secs: 10,
        ..default_config()
    };
    let mut manager = harness
        .manager("holder-a", config)
        .expect("create ttl-aware manager");
    let lease = manager
        .ensure_active_lease(SelectionRequest::default())
        .await
        .expect("acquire ttl-bound lease");

    assert_eq!(
        lease.expires_at() - lease.acquired_at(),
        Duration::seconds(30)
    );

    let renew_at = lease.acquired_at() + Duration::seconds(15);
    let renewed = manager
        .renew_active_lease_if_needed(renew_at)
        .await
        .expect("renew ttl-bound lease");

    assert_eq!(renewed.expires_at() - renew_at, Duration::seconds(30));
}

#[tokio::test]
async fn local_lease_scoped_session_refresh_preserves_stable_account_identity() {
    let harness = fixture_with_legacy_auth("acct-legacy").await;
    let grant = seed_local_lease_session(&harness, "holder-a")
        .await
        .expect("seed local lease session");

    let before = grant.auth_session.binding().account_id.clone();
    let refreshed = grant
        .auth_session
        .refresh_leased_turn_auth()
        .expect("refresh leased auth");
    let after = grant.auth_session.binding().account_id.clone();

    assert_eq!(before, after);
    assert_eq!(refreshed.account_id(), Some(before));
}

pub(crate) async fn register_account_persists_backend_private_auth_for_pooled_accounts()
-> anyhow::Result<()> {
    let harness = fixture_with_legacy_auth("acct-legacy").await;
    let backend = LocalAccountPoolBackend::new(
        harness.runtime.clone(),
        default_config().lease_ttl_duration(),
    );
    let record = backend
        .register_account(pooled_registration(
            "acct-1",
            "backend-handle-1",
            "fingerprint-1",
        ))
        .await?;
    let expected_backend_account_handle = normalized_backend_account_handle("acct-1");

    let auth_home = harness
        .runtime
        .codex_home()
        .join(".pooled-auth/backends/local/accounts")
        .join(expected_backend_account_handle.as_str());
    let auth = CodexAuth::from_auth_storage(&auth_home, AuthCredentialsStoreMode::File)?
        .expect("pooled auth should be persisted");

    assert_eq!(
        record.backend_account_handle,
        expected_backend_account_handle
    );
    assert_eq!(auth.get_account_id(), Some("acct-1".to_string()));
    Ok(())
}

pub(crate) async fn register_account_encodes_slash_account_id_for_backend_private_auth()
-> anyhow::Result<()> {
    let harness = fixture_with_legacy_auth("acct-legacy").await;
    let backend = LocalAccountPoolBackend::new(
        harness.runtime.clone(),
        default_config().lease_ttl_duration(),
    );
    let provider_account_id = "acct/1";
    let record = backend
        .register_account(pooled_registration(
            provider_account_id,
            "backend-handle-raw",
            "fingerprint-1",
        ))
        .await?;

    let expected_backend_account_handle = normalized_backend_account_handle(provider_account_id);
    let auth_home = harness
        .runtime
        .codex_home()
        .join(".pooled-auth/backends/local/accounts")
        .join(expected_backend_account_handle.as_str());
    let auth = CodexAuth::from_auth_storage(&auth_home, AuthCredentialsStoreMode::File)?
        .expect("pooled auth should be persisted in the encoded backend handle");

    assert_eq!(
        record.backend_account_handle,
        expected_backend_account_handle
    );
    assert_eq!(auth.get_account_id(), Some(provider_account_id.to_string()));
    Ok(())
}

pub(crate) async fn register_account_reuses_encoded_backend_handle_on_reregister()
-> anyhow::Result<()> {
    let harness = fixture_with_legacy_auth("acct-legacy").await;
    let backend = LocalAccountPoolBackend::new(
        harness.runtime.clone(),
        default_config().lease_ttl_duration(),
    );
    let provider_account_id = "acct/1";

    let first = backend
        .register_account(pooled_registration(
            provider_account_id,
            "backend-handle-first",
            "fingerprint-1",
        ))
        .await?;
    let second = backend
        .register_account(pooled_registration(
            provider_account_id,
            "backend-handle-second",
            "fingerprint-1",
        ))
        .await?;

    let expected_backend_account_handle = normalized_backend_account_handle(provider_account_id);
    assert_eq!(
        first.backend_account_handle,
        expected_backend_account_handle
    );
    assert_eq!(
        second.backend_account_handle,
        expected_backend_account_handle
    );
    assert_eq!(first.backend_account_handle, second.backend_account_handle);
    Ok(())
}

pub(crate) async fn register_account_removes_legacy_raw_backend_private_auth_on_reregister()
-> anyhow::Result<()> {
    let harness = fixture_with_legacy_auth("acct-legacy").await;
    let backend = LocalAccountPoolBackend::new(
        harness.runtime.clone(),
        default_config().lease_ttl_duration(),
    );
    let provider_account_id = "provider-acct-new";
    harness
        .runtime
        .upsert_registered_account(RegisteredAccountUpsert {
            account_id: "acct-existing".to_string(),
            backend_id: "local".to_string(),
            backend_family: "local".to_string(),
            workspace_id: Some("workspace-1".to_string()),
            backend_account_handle: provider_account_id.to_string(),
            account_kind: "chatgpt".to_string(),
            provider_fingerprint: "fingerprint-1".to_string(),
            display_name: Some("Existing ChatGPT".to_string()),
            source: None,
            enabled: true,
            healthy: true,
            membership: Some(RegisteredAccountMembership {
                pool_id: "legacy-default".to_string(),
                position: 1,
            }),
        })
        .await?;
    let legacy_auth_home = harness
        .runtime
        .codex_home()
        .join(".pooled-auth/backends/local/accounts")
        .join(provider_account_id);
    save_auth(
        legacy_auth_home.as_path(),
        &AuthDotJson {
            auth_mode: None,
            openai_api_key: None,
            tokens: Some(TokenData {
                id_token: parse_chatgpt_jwt_claims(&fake_access_token("acct-existing"))?,
                access_token: "existing-access".to_string(),
                refresh_token: "existing-refresh".to_string(),
                account_id: Some("acct-existing".to_string()),
            }),
            last_refresh: Some(Utc::now()),
        },
        AuthCredentialsStoreMode::File,
    )?;

    let record = backend
        .register_account(pooled_registration(
            provider_account_id,
            provider_account_id,
            "fingerprint-1",
        ))
        .await?;

    assert_eq!(record.account_id, "acct-existing");
    assert_eq!(
        record.backend_account_handle,
        normalized_backend_account_handle(provider_account_id)
    );
    assert!(
        !legacy_auth_home.exists(),
        "legacy raw backend-private auth should be removed"
    );

    let normalized_auth_home = harness
        .runtime
        .codex_home()
        .join(".pooled-auth/backends/local/accounts")
        .join(normalized_backend_account_handle(provider_account_id));
    let auth = CodexAuth::from_auth_storage(&normalized_auth_home, AuthCredentialsStoreMode::File)?
        .expect("normalized backend-private auth should be present");
    assert_eq!(auth.get_account_id(), Some(provider_account_id.to_string()));
    Ok(())
}

pub(crate) async fn register_account_preserves_existing_backend_private_auth_on_conflict()
-> anyhow::Result<()> {
    let harness = fixture_with_legacy_auth("acct-legacy").await;
    let backend = LocalAccountPoolBackend::new(
        harness.runtime.clone(),
        default_config().lease_ttl_duration(),
    );
    harness
        .runtime
        .upsert_registered_account(RegisteredAccountUpsert {
            account_id: "acct-existing".to_string(),
            backend_id: "local".to_string(),
            backend_family: "local".to_string(),
            workspace_id: Some("workspace-1".to_string()),
            backend_account_handle: normalized_backend_account_handle("acct-conflict"),
            account_kind: "chatgpt".to_string(),
            provider_fingerprint: "fingerprint-1".to_string(),
            display_name: Some("Existing ChatGPT".to_string()),
            source: None,
            enabled: true,
            healthy: true,
            membership: Some(RegisteredAccountMembership {
                pool_id: "legacy-default".to_string(),
                position: 1,
            }),
        })
        .await?;

    let auth_home = harness
        .runtime
        .codex_home()
        .join(".pooled-auth/backends/local/accounts")
        .join(normalized_backend_account_handle("acct-conflict"));
    save_auth(
        auth_home.as_path(),
        &AuthDotJson {
            auth_mode: None,
            openai_api_key: None,
            tokens: Some(TokenData {
                id_token: parse_chatgpt_jwt_claims(&fake_access_token("acct-existing"))?,
                access_token: "existing-access".to_string(),
                refresh_token: "existing-refresh".to_string(),
                account_id: Some("acct-existing".to_string()),
            }),
            last_refresh: Some(Utc::now()),
        },
        AuthCredentialsStoreMode::File,
    )?;

    harness
        .runtime
        .upsert_registered_account(RegisteredAccountUpsert {
            account_id: "acct-a".to_string(),
            backend_id: "local".to_string(),
            backend_family: "local".to_string(),
            workspace_id: None,
            backend_account_handle: normalized_backend_account_handle("acct-conflict"),
            account_kind: "chatgpt".to_string(),
            provider_fingerprint: "fingerprint-a".to_string(),
            display_name: Some("Conflicting ChatGPT A".to_string()),
            source: None,
            enabled: true,
            healthy: true,
            membership: Some(RegisteredAccountMembership {
                pool_id: "legacy-default".to_string(),
                position: 1,
            }),
        })
        .await?;
    harness
        .runtime
        .upsert_registered_account(RegisteredAccountUpsert {
            account_id: "acct-b".to_string(),
            backend_id: "local".to_string(),
            backend_family: "local".to_string(),
            workspace_id: None,
            backend_account_handle: "backend-handle-2".to_string(),
            account_kind: "chatgpt".to_string(),
            provider_fingerprint: "fingerprint-1".to_string(),
            display_name: Some("Conflicting ChatGPT B".to_string()),
            source: None,
            enabled: true,
            healthy: true,
            membership: Some(RegisteredAccountMembership {
                pool_id: "legacy-default".to_string(),
                position: 1,
            }),
        })
        .await?;

    let err = backend
        .register_account(RegisteredAccountRegistration {
            request: RegisteredAccountUpsert {
                account_id: "acct-conflict".to_string(),
                backend_id: "local".to_string(),
                backend_family: "local".to_string(),
                workspace_id: None,
                backend_account_handle: "backend-handle-1".to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: "fingerprint-1".to_string(),
                display_name: Some("Conflicting ChatGPT".to_string()),
                source: None,
                enabled: true,
                healthy: true,
                membership: Some(RegisteredAccountMembership {
                    pool_id: "legacy-default".to_string(),
                    position: 1,
                }),
            },
            pooled_registration_tokens: Some(ChatgptManagedRegistrationTokens {
                id_token: fake_access_token("acct-conflict"),
                access_token: "managed-access".to_string().into(),
                refresh_token: "managed-refresh".to_string().into(),
                account_id: "acct-conflict".to_string(),
            }),
        })
        .await
        .expect_err("conflicting registration should fail");
    assert!(err.to_string().contains("conflicting registered accounts"));

    let auth = CodexAuth::from_auth_storage(&auth_home, AuthCredentialsStoreMode::File)?
        .expect("existing backend-private auth should remain");
    assert_eq!(auth.get_account_id(), Some("acct-existing".to_string()));
    Ok(())
}

pub(crate) async fn register_account_returns_actual_persisted_row_for_existing_account()
-> anyhow::Result<()> {
    let harness = fixture_with_legacy_auth("acct-legacy").await;
    let backend = LocalAccountPoolBackend::new(
        harness.runtime.clone(),
        default_config().lease_ttl_duration(),
    );
    harness
        .runtime
        .upsert_registered_account(RegisteredAccountUpsert {
            account_id: "acct-existing".to_string(),
            backend_id: "local".to_string(),
            backend_family: "local".to_string(),
            workspace_id: Some("workspace-1".to_string()),
            backend_account_handle: "backend-handle-1".to_string(),
            account_kind: "chatgpt".to_string(),
            provider_fingerprint: "fingerprint-1".to_string(),
            display_name: Some("Existing ChatGPT".to_string()),
            source: None,
            enabled: true,
            healthy: true,
            membership: Some(RegisteredAccountMembership {
                pool_id: "legacy-default".to_string(),
                position: 1,
            }),
        })
        .await?;

    let returned = backend
        .register_account(pooled_registration(
            "acct-existing",
            "backend-handle-1",
            "fingerprint-1",
        ))
        .await?;
    let persisted = harness
        .runtime
        .read_registered_account("acct-existing")
        .await?
        .expect("existing account should remain persisted");

    assert_eq!(returned, persisted);
    Ok(())
}

pub(crate) async fn register_account_cleans_new_backend_private_auth_on_persistence_failure()
-> anyhow::Result<()> {
    let harness = fixture_with_legacy_auth("acct-legacy").await;
    let backend = LocalAccountPoolBackend::new(
        harness.runtime.clone(),
        default_config().lease_ttl_duration(),
    );
    harness
        .runtime
        .upsert_registered_account(RegisteredAccountUpsert {
            account_id: "acct-a".to_string(),
            backend_id: "local".to_string(),
            backend_family: "local".to_string(),
            workspace_id: None,
            backend_account_handle: normalized_backend_account_handle("acct-new"),
            account_kind: "chatgpt".to_string(),
            provider_fingerprint: "fingerprint-a".to_string(),
            display_name: Some("Conflicting ChatGPT A".to_string()),
            source: None,
            enabled: true,
            healthy: true,
            membership: Some(RegisteredAccountMembership {
                pool_id: "legacy-default".to_string(),
                position: 1,
            }),
        })
        .await?;
    harness
        .runtime
        .upsert_registered_account(RegisteredAccountUpsert {
            account_id: "acct-b".to_string(),
            backend_id: "local".to_string(),
            backend_family: "local".to_string(),
            workspace_id: None,
            backend_account_handle: "backend-handle-2".to_string(),
            account_kind: "chatgpt".to_string(),
            provider_fingerprint: "fingerprint-1".to_string(),
            display_name: Some("Conflicting ChatGPT B".to_string()),
            source: None,
            enabled: true,
            healthy: true,
            membership: Some(RegisteredAccountMembership {
                pool_id: "legacy-default".to_string(),
                position: 1,
            }),
        })
        .await?;
    let auth_home = harness
        .runtime
        .codex_home()
        .join(".pooled-auth/backends/local/accounts")
        .join(normalized_backend_account_handle("acct-new"));

    let err = backend
        .register_account(RegisteredAccountRegistration {
            request: RegisteredAccountUpsert {
                account_id: "acct-new".to_string(),
                backend_id: "local".to_string(),
                backend_family: "local".to_string(),
                workspace_id: None,
                backend_account_handle: "backend-handle-1".to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: "fingerprint-1".to_string(),
                display_name: Some("New ChatGPT".to_string()),
                source: None,
                enabled: true,
                healthy: true,
                membership: Some(RegisteredAccountMembership {
                    pool_id: "legacy-default".to_string(),
                    position: 1,
                }),
            },
            pooled_registration_tokens: Some(ChatgptManagedRegistrationTokens {
                id_token: fake_access_token("acct-new"),
                access_token: "managed-access".to_string().into(),
                refresh_token: "managed-refresh".to_string().into(),
                account_id: "acct-new".to_string(),
            }),
        })
        .await
        .expect_err("registration should fail");
    assert!(err.to_string().contains("conflicting registered accounts"));

    assert_eq!(
        harness.runtime.read_registered_account("acct-new").await?,
        None
    );
    assert!(
        !auth_home.exists(),
        "new backend-private auth should be cleaned up"
    );
    Ok(())
}

pub(crate) async fn stale_lease_scoped_session_fails_after_epoch_supersession() -> anyhow::Result<()>
{
    let harness = fixture_with_legacy_auth("acct-legacy").await;
    let grant = seed_local_lease_session(&harness, "holder-a")
        .await
        .expect("seed local lease session");

    grant
        .auth_session
        .refresh_leased_turn_auth()
        .expect("initial refresh should succeed");

    let backend = LocalAccountPoolBackend::new(
        harness.runtime.clone(),
        default_config().lease_ttl_duration(),
    );
    backend
        .release_lease(&grant.key(), Utc::now())
        .await
        .expect("release stale lease");
    let renewed = backend
        .acquire_lease("legacy-default", "holder-b")
        .await
        .expect("reacquire lease with higher epoch");
    assert_eq!(renewed.account_id(), grant.account_id());

    let err = grant
        .auth_session
        .refresh_leased_turn_auth()
        .expect_err("stale session should fail after epoch supersession");
    assert!(err.to_string().contains("lease"), "unexpected error: {err}");

    Ok(())
}

pub(crate) async fn acquire_lease_releases_lease_when_marker_write_fails() -> anyhow::Result<()> {
    let harness = fixture_with_legacy_auth("acct-legacy").await;
    let backend = LocalAccountPoolBackend::new(
        harness.runtime.clone(),
        default_config().lease_ttl_duration(),
    );
    backend
        .register_account(pooled_registration(
            "acct-legacy",
            "backend-handle-legacy",
            "fingerprint-legacy",
        ))
        .await?;

    let auth_home = harness
        .runtime
        .codex_home()
        .join(".pooled-auth/backends/local/accounts")
        .join(normalized_backend_account_handle("acct-legacy"));
    fs::remove_dir_all(&auth_home)?;
    fs::write(&auth_home, "blocking-file")?;

    let err = backend
        .acquire_lease("legacy-default", "holder-a")
        .await
        .expect_err("marker write should fail");
    assert!(
        err.to_string().contains("blocking-file")
            || err.to_string().contains("Not a directory")
            || err.to_string().contains("File exists"),
        "unexpected error: {err}"
    );

    assert_eq!(
        harness.runtime.read_active_holder_lease("holder-a").await?,
        None
    );

    Ok(())
}

#[cfg(unix)]
pub(crate) async fn delete_registered_account_preserves_registry_when_backend_cleanup_fails()
-> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let harness = fixture_with_legacy_auth("acct-legacy").await;
    let backend = LocalAccountPoolBackend::new(
        harness.runtime.clone(),
        default_config().lease_ttl_duration(),
    );
    backend
        .register_account(pooled_registration(
            "acct-legacy",
            "backend-handle-legacy",
            "fingerprint-legacy",
        ))
        .await?;

    let auth_root = harness
        .runtime
        .codex_home()
        .join(".pooled-auth/backends/local/accounts");
    let original_mode = fs::metadata(&auth_root)?.permissions().mode();
    let mut restricted = fs::metadata(&auth_root)?.permissions();
    restricted.set_mode(0o500);
    fs::set_permissions(&auth_root, restricted)?;

    let delete_result = backend.delete_registered_account("acct-legacy").await;

    let mut restored = fs::metadata(&auth_root)?.permissions();
    restored.set_mode(original_mode);
    fs::set_permissions(&auth_root, restored)?;

    let err = delete_result.expect_err("backend cleanup should fail");
    assert!(
        err.to_string()
            .contains("failed to delete backend-private auth")
            || err.to_string().contains("Permission denied")
            || err.to_string().contains("permission denied"),
        "unexpected error: {err}"
    );
    assert!(
        harness
            .runtime
            .read_registered_account("acct-legacy")
            .await?
            .is_some(),
        "local registry entry should remain after backend cleanup failure"
    );

    Ok(())
}

async fn seed_local_lease_session(
    harness: &TestHarness,
    holder_instance_id: &str,
) -> anyhow::Result<LeaseGrant> {
    let backend = LocalAccountPoolBackend::new(
        harness.runtime.clone(),
        default_config().lease_ttl_duration(),
    );
    backend
        .register_account(pooled_registration(
            "acct-legacy",
            "backend-handle-legacy",
            "fingerprint-legacy",
        ))
        .await?;
    let grant = backend
        .acquire_lease("legacy-default", holder_instance_id)
        .await?;

    Ok(grant)
}

struct TestHarness {
    runtime: Arc<StateRuntime>,
    bootstrap_calls: Arc<AtomicUsize>,
    legacy_account_id: Option<String>,
    _tempdir: TempDir,
}

impl TestHarness {
    fn manager(
        &self,
        holder_instance_id: &str,
        config: AccountPoolConfig,
    ) -> anyhow::Result<AccountPoolManager<LocalAccountPoolBackend, TestLegacyBootstrap>> {
        let backend =
            LocalAccountPoolBackend::new(self.runtime.clone(), config.lease_ttl_duration());
        let legacy_bootstrap = TestLegacyBootstrap {
            account_id: self.legacy_account_id.clone(),
            calls: self.bootstrap_calls.clone(),
        };
        AccountPoolManager::new(
            backend,
            legacy_bootstrap,
            config,
            holder_instance_id.to_string(),
        )
    }

    async fn write_quota(
        &self,
        account_id: &str,
        limit_id: &str,
        template: AccountQuotaStateRecord,
    ) -> anyhow::Result<()> {
        self.runtime
            .upsert_account_quota_state(AccountQuotaStateRecord {
                account_id: account_id.to_string(),
                limit_id: limit_id.to_string(),
                ..template
            })
            .await
    }

    async fn acquire_runtime_selected_lease(
        &self,
        request: SelectionRequest,
    ) -> anyhow::Result<codex_account_pool::LeasedAccount> {
        let mut manager = self.manager("runtime-select-holder", default_config())?;
        manager.ensure_active_lease(request).await
    }
}

async fn fixture_with_legacy_auth(account_id: &str) -> TestHarness {
    let tempdir = tempfile::tempdir().unwrap_or_else(|err| panic!("create tempdir failed: {err}"));
    let runtime = StateRuntime::init(tempdir.path().to_path_buf(), "test-provider".to_string())
        .await
        .unwrap_or_else(|err| panic!("initialize runtime failed: {err}"));
    let (_, bootstrap_calls) = TestLegacyBootstrap::with_legacy_account(account_id);
    TestHarness {
        runtime,
        bootstrap_calls,
        legacy_account_id: Some(account_id.to_string()),
        _tempdir: tempdir,
    }
}

async fn fixture_with_registered_account(account_id: &str) -> TestHarness {
    let harness = fixture_with_legacy_auth(account_id).await;
    let backend = LocalAccountPoolBackend::new(
        harness.runtime.clone(),
        default_config().lease_ttl_duration(),
    );
    backend
        .register_account(pooled_registration(
            account_id,
            &format!("backend-handle-{account_id}"),
            &format!("fingerprint-{account_id}"),
        ))
        .await
        .unwrap_or_else(|err| panic!("register pooled account failed: {err}"));
    harness
}

async fn fixture_with_registered_accounts(account_ids: &[&str]) -> TestHarness {
    let harness = fixture_with_legacy_auth(account_ids[0]).await;
    let backend = LocalAccountPoolBackend::new(
        harness.runtime.clone(),
        default_config().lease_ttl_duration(),
    );
    for account_id in account_ids {
        backend
            .register_account(pooled_registration(
                account_id,
                &format!("backend-handle-{account_id}"),
                &format!("fingerprint-{account_id}"),
            ))
            .await
            .unwrap_or_else(|err| panic!("register pooled account failed: {err}"));
    }
    harness
}

async fn quota_fixture_with_three_accounts() -> TestHarness {
    fixture_with_registered_accounts(&["acct-a", "acct-b", "acct-c"]).await
}

#[derive(Clone)]
struct TestLegacyBootstrap {
    account_id: Option<String>,
    calls: Arc<AtomicUsize>,
}

impl TestLegacyBootstrap {
    fn with_legacy_account(account_id: &str) -> (Self, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Self {
                account_id: Some(account_id.to_string()),
                calls: calls.clone(),
            },
            calls,
        )
    }
}

#[async_trait::async_trait]
impl LegacyAuthBootstrap for TestLegacyBootstrap {
    async fn current_legacy_auth(&self) -> anyhow::Result<Option<LegacyAccountImport>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .account_id
            .clone()
            .map(|account_id| LegacyAccountImport { account_id }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeScenario {
    Contention,
    Success,
    StillBlocked,
    Ambiguous,
    RefreshError,
    OrdinarySelectContention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeExecutionOutcome {
    SelectionRestartRequired,
    NoProbeAttempt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SoftRotationProbeOutcome {
    original_lease_account_id: String,
    active_lease_retained_during_probe: bool,
    verification_lease_released: bool,
    final_selected_account_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ProbeBackendState {
    probe_reserved_until: Option<chrono::DateTime<Utc>>,
    refresh_called: bool,
    active_released_before_refresh: bool,
    verification_released: bool,
    probe_acquire_called: bool,
    normal_select_failed: bool,
    current_reacquired_after_select_failure: bool,
}

struct ScriptedProbeHarness {
    manager: AccountPoolManager<ScriptedProbeBackend, NoLegacyAuthBootstrap>,
    backend: ScriptedProbeBackend,
}

impl ScriptedProbeHarness {
    async fn new(scenario: ProbeScenario) -> anyhow::Result<Self> {
        let backend = ScriptedProbeBackend::new("test-holder", scenario).await?;
        let manager = AccountPoolManager::new(
            backend.clone(),
            NoLegacyAuthBootstrap,
            damping_config(),
            "test-holder".to_string(),
        )?;
        Ok(Self { manager, backend })
    }

    async fn force_probe_lease_contention(&mut self) -> anyhow::Result<ProbeExecutionOutcome> {
        let current = self
            .manager
            .ensure_active_lease(runtime_selection_request())
            .await?;
        self.manager
            .report_rate_limits(
                current.key(),
                snapshot_at(95.0, current.acquired_at() + Duration::seconds(121)),
            )
            .await?;

        let outcome = match self
            .manager
            .ensure_active_lease(SelectionRequest {
                now: Some(current.acquired_at() + Duration::seconds(122)),
                pool_id: Some(current.pool_id().to_string()),
                intent: SelectionIntent::SoftRotation,
                selection_family: None,
                preferred_account_id: None,
                current_account_id: None,
                just_replaced_account_id: None,
                reserved_probe_target_account_id: None,
                proactive_threshold_percent: 85,
            })
            .await
        {
            Ok(lease)
                if lease.account_id() == current.account_id()
                    && self.backend.state().probe_reserved_until.is_some() =>
            {
                ProbeExecutionOutcome::SelectionRestartRequired
            }
            Ok(_) | Err(_) => ProbeExecutionOutcome::NoProbeAttempt,
        };

        Ok(outcome)
    }

    async fn trigger_soft_rotation_probe_success(
        &mut self,
    ) -> anyhow::Result<SoftRotationProbeOutcome> {
        let current = self
            .manager
            .ensure_active_lease(runtime_selection_request())
            .await?;
        self.manager
            .report_rate_limits(
                current.key(),
                snapshot_at(95.0, current.acquired_at() + Duration::seconds(121)),
            )
            .await?;

        let next = self
            .manager
            .ensure_active_lease(SelectionRequest {
                now: Some(current.acquired_at() + Duration::seconds(122)),
                pool_id: Some(current.pool_id().to_string()),
                intent: SelectionIntent::SoftRotation,
                selection_family: None,
                preferred_account_id: None,
                current_account_id: None,
                just_replaced_account_id: None,
                reserved_probe_target_account_id: None,
                proactive_threshold_percent: 85,
            })
            .await?;
        let state = self.backend.state();

        Ok(SoftRotationProbeOutcome {
            original_lease_account_id: current.account_id().to_string(),
            active_lease_retained_during_probe: state.refresh_called
                && !state.active_released_before_refresh,
            verification_lease_released: state.verification_released,
            final_selected_account_id: next.account_id().to_string(),
        })
    }

    async fn trigger_soft_rotation_probe_attempt(
        &mut self,
    ) -> anyhow::Result<Result<String, String>> {
        let current = self
            .manager
            .ensure_active_lease(runtime_selection_request())
            .await?;
        self.manager
            .report_rate_limits(
                current.key(),
                snapshot_at(95.0, current.acquired_at() + Duration::seconds(121)),
            )
            .await?;

        Ok(self
            .manager
            .ensure_active_lease(SelectionRequest {
                now: Some(current.acquired_at() + Duration::seconds(122)),
                pool_id: Some(current.pool_id().to_string()),
                intent: SelectionIntent::SoftRotation,
                selection_family: None,
                preferred_account_id: None,
                current_account_id: None,
                just_replaced_account_id: None,
                reserved_probe_target_account_id: None,
                proactive_threshold_percent: 85,
            })
            .await
            .map(|lease| lease.account_id().to_string())
            .map_err(|err| err.to_string()))
    }

    fn probe_state(&self) -> ProbeBackendState {
        self.backend.state()
    }
}

#[derive(Clone)]
struct ScriptedProbeBackend {
    main_holder_instance_id: String,
    current_grant: LeaseGrant,
    recovered_grant: LeaseGrant,
    fallback_grant: LeaseGrant,
    verification_grant: LeaseGrant,
    scenario: ProbeScenario,
    state: Arc<Mutex<ProbeBackendState>>,
}

impl ScriptedProbeBackend {
    async fn new(main_holder_instance_id: &str, scenario: ProbeScenario) -> anyhow::Result<Self> {
        Ok(Self {
            main_holder_instance_id: main_holder_instance_id.to_string(),
            current_grant: scripted_grant("acct-current", main_holder_instance_id).await?,
            recovered_grant: scripted_grant("acct-recovered", main_holder_instance_id).await?,
            fallback_grant: scripted_grant("acct-fallback", main_holder_instance_id).await?,
            verification_grant: scripted_grant("acct-recovered", "probe-holder").await?,
            scenario,
            state: Arc::new(Mutex::new(ProbeBackendState::default())),
        })
    }

    fn is_probe_holder(&self, holder_instance_id: &str) -> bool {
        holder_instance_id != self.main_holder_instance_id
    }

    fn state(&self) -> ProbeBackendState {
        self.state.lock().expect("lock probe state").clone()
    }

    fn plan_for_request(
        &self,
        request: &SelectionRequest,
    ) -> (String, codex_account_pool::SelectionPlan) {
        let terminal_action = if request.current_account_id.is_none() {
            SelectionAction::Select("acct-current".to_string())
        } else if self.state().refresh_called {
            match self.scenario {
                ProbeScenario::Success => SelectionAction::Select("acct-recovered".to_string()),
                ProbeScenario::OrdinarySelectContention if !self.state().normal_select_failed => {
                    SelectionAction::Select("acct-recovered".to_string())
                }
                ProbeScenario::OrdinarySelectContention => {
                    SelectionAction::Select("acct-fallback".to_string())
                }
                ProbeScenario::StillBlocked
                | ProbeScenario::Ambiguous
                | ProbeScenario::RefreshError
                | ProbeScenario::Contention => SelectionAction::StayOnCurrent,
            }
        } else if self.state().probe_reserved_until.is_some() {
            SelectionAction::StayOnCurrent
        } else {
            SelectionAction::Probe("acct-recovered".to_string())
        };

        (
            "codex".to_string(),
            codex_account_pool::SelectionPlan {
                eligible_candidates: Vec::new(),
                probe_candidate: matches!(terminal_action, SelectionAction::Probe(_))
                    .then(|| "acct-recovered".to_string()),
                rejected_candidates: Vec::new(),
                decision_reason: match terminal_action {
                    SelectionAction::Select(_) => {
                        codex_account_pool::SelectionDecisionReason::OrdinaryRanking
                    }
                    SelectionAction::Probe(_) => {
                        codex_account_pool::SelectionDecisionReason::ProbeFallback
                    }
                    SelectionAction::StayOnCurrent => {
                        codex_account_pool::SelectionDecisionReason::SoftRotationCurrentRetained
                    }
                    SelectionAction::NoCandidate => {
                        codex_account_pool::SelectionDecisionReason::NoCandidate
                    }
                },
                terminal_action,
            },
        )
    }
}

#[async_trait::async_trait]
impl AccountPoolExecutionBackend for ScriptedProbeBackend {
    async fn plan_runtime_selection(
        &self,
        request: &SelectionRequest,
        _holder_instance_id: &str,
    ) -> anyhow::Result<(String, codex_account_pool::SelectionPlan)> {
        Ok(self.plan_for_request(request))
    }

    async fn acquire_lease(
        &self,
        _pool_id: &str,
        holder_instance_id: &str,
    ) -> std::result::Result<LeaseGrant, AccountLeaseError> {
        if self.is_probe_holder(holder_instance_id) {
            match self.scenario {
                ProbeScenario::Contention => Err(AccountLeaseError::NoEligibleAccount),
                ProbeScenario::Success
                | ProbeScenario::StillBlocked
                | ProbeScenario::Ambiguous
                | ProbeScenario::RefreshError
                | ProbeScenario::OrdinarySelectContention => Ok(self.verification_grant.clone()),
            }
        } else if self.state().refresh_called {
            Ok(self.recovered_grant.clone())
        } else {
            Ok(self.current_grant.clone())
        }
    }

    async fn acquire_preferred_lease(
        &self,
        pool_id: &str,
        account_id: &str,
        _selection_family: &str,
        holder_instance_id: &str,
    ) -> std::result::Result<LeaseGrant, AccountLeaseError> {
        let _ = pool_id;
        if self.is_probe_holder(holder_instance_id) {
            return self.acquire_lease(pool_id, holder_instance_id).await;
        }
        if self.scenario == ProbeScenario::OrdinarySelectContention
            && account_id == "acct-recovered"
        {
            let mut state = self.state.lock().expect("lock probe state");
            state.normal_select_failed = true;
            return Err(AccountLeaseError::NoEligibleAccount);
        }
        if self.scenario == ProbeScenario::OrdinarySelectContention
            && account_id == "acct-current"
            && self.state().normal_select_failed
        {
            let mut state = self.state.lock().expect("lock probe state");
            state.current_reacquired_after_select_failure = true;
            return Ok(self.current_grant.clone());
        }
        if self.scenario == ProbeScenario::OrdinarySelectContention && account_id == "acct-fallback"
        {
            return Ok(self.fallback_grant.clone());
        }
        if account_id == "acct-recovered" && self.state().refresh_called {
            Ok(self.recovered_grant.clone())
        } else {
            Ok(self.current_grant.clone())
        }
    }

    async fn acquire_probe_lease(
        &self,
        pool_id: &str,
        account_id: &str,
        reservation: &ProbeReservation,
        holder_instance_id: &str,
    ) -> std::result::Result<LeaseGrant, AccountLeaseError> {
        let _ = (account_id, reservation);
        {
            let mut state = self.state.lock().expect("lock probe state");
            state.probe_acquire_called = true;
        }
        self.acquire_lease(pool_id, holder_instance_id).await
    }

    async fn reserve_quota_probe(
        &self,
        _account_id: &str,
        selection_family: &str,
        now: chrono::DateTime<Utc>,
        reserved_for: Duration,
    ) -> anyhow::Result<Option<ProbeReservation>> {
        let mut state = self.state.lock().expect("lock probe state");
        if state.probe_reserved_until.is_some() {
            return Ok(None);
        }
        let reserved_until = now + reserved_for;
        state.probe_reserved_until = Some(reserved_until);
        Ok(Some(ProbeReservation {
            limit_id: selection_family.to_string(),
            reserved_until,
        }))
    }

    async fn renew_lease(
        &self,
        lease: &codex_state::LeaseKey,
        _now: chrono::DateTime<Utc>,
    ) -> anyhow::Result<LeaseRenewal> {
        let grant = if lease.account_id == self.current_grant.account_id() {
            self.current_grant.clone()
        } else {
            self.recovered_grant.clone()
        };

        Ok(LeaseRenewal::Renewed(codex_state::AccountLeaseRecord {
            lease_id: grant.key().lease_id,
            pool_id: grant.pool_id().to_string(),
            account_id: grant.account_id().to_string(),
            holder_instance_id: self.main_holder_instance_id.clone(),
            lease_epoch: grant.lease_epoch(),
            acquired_at: grant.acquired_at(),
            renewed_at: grant.acquired_at(),
            expires_at: grant.expires_at(),
            released_at: None,
        }))
    }

    async fn release_lease(
        &self,
        lease: &codex_state::LeaseKey,
        _now: chrono::DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let mut state = self.state.lock().expect("lock probe state");
        if !state.refresh_called && lease.account_id == self.current_grant.account_id() {
            state.active_released_before_refresh = true;
        }
        if lease.account_id == self.verification_grant.account_id()
            && lease.lease_id == self.verification_grant.key().lease_id
        {
            state.verification_released = true;
        }
        Ok(true)
    }

    async fn record_health_event(&self, _event: AccountHealthEvent) -> anyhow::Result<()> {
        Ok(())
    }

    async fn read_account_health_event_sequence(
        &self,
        _account_id: &str,
    ) -> anyhow::Result<Option<i64>> {
        Ok(Some(0))
    }

    async fn read_startup_selection(&self) -> anyhow::Result<AccountStartupSelectionState> {
        Ok(AccountStartupSelectionState {
            default_pool_id: Some("legacy-default".to_string()),
            preferred_account_id: None,
            suppressed: false,
        })
    }

    async fn read_startup_pool_inventory(
        &self,
    ) -> anyhow::Result<codex_account_pool::StartupPoolInventory> {
        Ok(codex_account_pool::StartupPoolInventory {
            candidates: vec![codex_account_pool::StartupPoolCandidate {
                pool_id: "legacy-default".to_string(),
                display_name: None,
                status: None,
            }],
        })
    }

    async fn read_startup_selection_facts(
        &self,
        _pool_id: &str,
    ) -> anyhow::Result<codex_account_pool::StartupSelectionFacts> {
        Ok(codex_account_pool::StartupSelectionFacts {
            preferred_account_outcome: None,
            predicted_account_id: Some(self.current_grant.account_id().to_string()),
            any_eligible_account: true,
        })
    }

    async fn read_account_startup_status(
        &self,
        _configured_default_pool_id: Option<&str>,
    ) -> anyhow::Result<AccountStartupStatus> {
        Ok(AccountStartupStatus {
            preview: codex_state::AccountStartupSelectionPreview {
                effective_pool_id: Some("legacy-default".to_string()),
                preferred_account_id: None,
                suppressed: false,
                predicted_account_id: Some(self.current_grant.account_id().to_string()),
                eligibility: codex_state::AccountStartupEligibility::AutomaticAccountSelected,
            },
            configured_default_pool_id: Some("legacy-default".to_string()),
            persisted_default_pool_id: Some("legacy-default".to_string()),
            effective_pool_resolution_source: EffectivePoolResolutionSource::ConfigDefault,
            startup_availability: AccountStartupAvailability::Available,
            startup_resolution_issue: None,
            candidate_pools: Vec::new(),
        })
    }

    async fn refresh_quota_probe(
        &self,
        _lease: &LeaseGrant,
        _reservation: &ProbeReservation,
    ) -> anyhow::Result<Option<ProbeOutcome>> {
        let mut state = self.state.lock().expect("lock probe state");
        state.refresh_called = true;
        match self.scenario {
            ProbeScenario::Success
            | ProbeScenario::Contention
            | ProbeScenario::OrdinarySelectContention => Ok(Some(ProbeOutcome::Success)),
            ProbeScenario::StillBlocked => Ok(Some(ProbeOutcome::StillBlocked)),
            ProbeScenario::Ambiguous => Ok(Some(ProbeOutcome::Ambiguous)),
            ProbeScenario::RefreshError => Err(anyhow::anyhow!("probe refresh failed")),
        }
    }
}

async fn scripted_grant(account_id: &str, holder_instance_id: &str) -> anyhow::Result<LeaseGrant> {
    let harness = fixture_with_registered_account(account_id).await;
    let backend = LocalAccountPoolBackend::new(
        harness.runtime.clone(),
        default_config().lease_ttl_duration(),
    );
    backend
        .acquire_lease("legacy-default", holder_instance_id)
        .await
        .map_err(Into::into)
}

fn snapshot(used_percent: f64) -> RateLimitSnapshot {
    RateLimitSnapshot::new(used_percent, Utc::now())
}

fn snapshot_at(used_percent: f64, observed_at: chrono::DateTime<Utc>) -> RateLimitSnapshot {
    RateLimitSnapshot::new(used_percent, observed_at)
}

fn usage_limit_event() -> UsageLimitEvent {
    UsageLimitEvent::new(Utc::now())
}

fn usage_limit_event_at(observed_at: chrono::DateTime<Utc>) -> UsageLimitEvent {
    UsageLimitEvent::new(observed_at)
}

fn runtime_selection_request() -> SelectionRequest {
    SelectionRequest {
        pool_id: Some("legacy-default".to_string()),
        ..SelectionRequest::default()
    }
}

fn default_config() -> AccountPoolConfig {
    AccountPoolConfig {
        default_pool_id: Some("legacy-default".to_string()),
        ..AccountPoolConfig::default()
    }
}

fn damping_config() -> AccountPoolConfig {
    AccountPoolConfig {
        min_switch_interval_secs: 120,
        ..default_config()
    }
}

fn registry_entry_update(account_id: &str, enabled: bool) -> AccountRegistryEntryUpdate {
    AccountRegistryEntryUpdate {
        account_id: account_id.to_string(),
        pool_id: "legacy-default".to_string(),
        position: 1,
        account_kind: "chatgpt".to_string(),
        backend_family: "local".to_string(),
        workspace_id: None,
        enabled,
        healthy: true,
    }
}

fn healthy_primary(primary_used_percent: f64) -> AccountQuotaStateRecord {
    quota_template(QuotaExhaustedWindows::None, Some(primary_used_percent))
}

fn exhausted_primary() -> AccountQuotaStateRecord {
    quota_template(QuotaExhaustedWindows::Primary, Some(99.0))
}

fn exhausted_secondary() -> AccountQuotaStateRecord {
    quota_template(QuotaExhaustedWindows::Secondary, Some(88.0))
}

fn quota_template(
    exhausted_windows: QuotaExhaustedWindows,
    primary_used_percent: Option<f64>,
) -> AccountQuotaStateRecord {
    let now = Utc::now();
    AccountQuotaStateRecord {
        account_id: String::new(),
        limit_id: String::new(),
        primary_used_percent,
        primary_resets_at: None,
        secondary_used_percent: None,
        secondary_resets_at: None,
        observed_at: now,
        exhausted_windows,
        predicted_blocked_until: exhausted_windows
            .is_exhausted()
            .then_some(now + Duration::minutes(30)),
        next_probe_after: exhausted_windows
            .is_exhausted()
            .then_some(now - Duration::seconds(1)),
        probe_backoff_level: 0,
        last_probe_result: None,
    }
}

fn pooled_registration(
    account_id: &str,
    backend_account_handle: &str,
    provider_fingerprint: &str,
) -> RegisteredAccountRegistration {
    RegisteredAccountRegistration {
        request: pooled_registration_request(
            account_id,
            backend_account_handle,
            provider_fingerprint,
        ),
        pooled_registration_tokens: Some(fake_registration_tokens(account_id)),
    }
}

fn pooled_registration_request(
    account_id: &str,
    backend_account_handle: &str,
    provider_fingerprint: &str,
) -> RegisteredAccountUpsert {
    RegisteredAccountUpsert {
        account_id: account_id.to_string(),
        backend_id: "local".to_string(),
        backend_family: "local".to_string(),
        workspace_id: None,
        backend_account_handle: backend_account_handle.to_string(),
        account_kind: "chatgpt".to_string(),
        provider_fingerprint: provider_fingerprint.to_string(),
        display_name: Some("Managed ChatGPT".to_string()),
        source: None,
        enabled: true,
        healthy: true,
        membership: Some(RegisteredAccountMembership {
            pool_id: "legacy-default".to_string(),
            position: 1,
        }),
    }
}

fn fake_access_token(chatgpt_account_id: &str) -> String {
    make_chatgpt_jwt(chatgpt_account_id)
}

fn fake_registration_tokens(chatgpt_account_id: &str) -> ChatgptManagedRegistrationTokens {
    ChatgptManagedRegistrationTokens {
        id_token: make_chatgpt_jwt(chatgpt_account_id),
        access_token: format!("access-token-{chatgpt_account_id}").into(),
        refresh_token: format!("refresh-token-{chatgpt_account_id}").into(),
        account_id: chatgpt_account_id.to_string(),
    }
}

fn normalized_backend_account_handle(provider_account_id: &str) -> String {
    let encoded_provider_account_id = provider_account_id
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("chatgpt-{encoded_provider_account_id}")
}

fn make_chatgpt_jwt(chatgpt_account_id: &str) -> String {
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
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let header_b64 = b64(&serde_json::to_vec(&header).unwrap_or_else(|err| {
        panic!("serialize header: {err}");
    }));
    let payload_b64 = b64(&serde_json::to_vec(&payload).unwrap_or_else(|err| {
        panic!("serialize payload: {err}");
    }));
    let signature_b64 = b64(b"sig");
    format!("{header_b64}.{payload_b64}.{signature_b64}")
}
