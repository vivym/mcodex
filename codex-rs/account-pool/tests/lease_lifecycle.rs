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
use codex_account_pool::RateLimitSnapshot;
use codex_account_pool::RegisteredAccountRegistration;
use codex_account_pool::SelectionRequest;
use codex_account_pool::UsageLimitEvent;
use codex_login::AuthCredentialsStoreMode;
use codex_login::AuthDotJson;
use codex_login::ChatgptManagedRegistrationTokens;
use codex_login::CodexAuth;
use codex_login::TokenData;
use codex_login::save_auth;
use codex_login::token_data::parse_chatgpt_jwt_claims;
use codex_state::LegacyAccountImport;
use codex_state::RegisteredAccountMembership;
use codex_state::RegisteredAccountUpsert;
use codex_state::StateRuntime;
use pretty_assertions::assert_eq;
use std::fs;
use std::sync::Arc;
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
            err.to_string().contains("no eligible account"),
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
        .report_rate_limits(lease.key(), snapshot(/* used_percent */ 95.0))
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

fn snapshot(used_percent: f64) -> RateLimitSnapshot {
    RateLimitSnapshot::new(used_percent, Utc::now())
}

fn usage_limit_event() -> UsageLimitEvent {
    UsageLimitEvent::new(Utc::now())
}

fn default_config() -> AccountPoolConfig {
    AccountPoolConfig {
        default_pool_id: Some("legacy-default".to_string()),
        ..AccountPoolConfig::default()
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
