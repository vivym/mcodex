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
use codex_account_pool::SelectionRequest;
use codex_account_pool::UsageLimitEvent;
use codex_login::AuthCredentialsStoreMode;
use codex_login::ChatgptManagedRegistrationTokens;
use codex_login::CodexAuth;
use codex_state::LegacyAccountImport;
use codex_state::RegisteredAccountMembership;
use codex_state::RegisteredAccountUpsert;
use codex_state::StateRuntime;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use tempfile::TempDir;

#[tokio::test]
async fn ensure_active_lease_reuses_sticky_account_until_threshold() {
    let harness = fixture_with_legacy_auth("acct-legacy").await;
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
    let harness = fixture_with_legacy_auth("acct-legacy").await;
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
    async fn lease_lifecycle_stale_session_fails_after_epoch_supersession() {
        stale_lease_scoped_session_fails_after_epoch_supersession()
            .await
            .expect("stale session should fail after epoch supersession");
    }
}

#[tokio::test]
async fn release_active_lease_persists_release_and_allows_immediate_reacquire() {
    let harness = fixture_with_legacy_auth("acct-legacy").await;
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
    let harness = fixture_with_legacy_auth("acct-legacy").await;
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
    let harness = fixture_with_legacy_auth("acct-legacy").await;
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
    let request = RegisteredAccountUpsert {
        account_id: "acct-1".to_string(),
        backend_id: "local".to_string(),
        backend_family: "local".to_string(),
        workspace_id: None,
        backend_account_handle: "backend-handle-1".to_string(),
        account_kind: "chatgpt".to_string(),
        provider_fingerprint: "fingerprint-1".to_string(),
        display_name: Some("Managed ChatGPT".to_string()),
        source: None,
        enabled: true,
        healthy: true,
        membership: Some(RegisteredAccountMembership {
            pool_id: "legacy-default".to_string(),
            position: 1,
        }),
    };
    let tokens = ChatgptManagedRegistrationTokens {
        id_token: fake_access_token("acct-1"),
        access_token: "managed-access".to_string().into(),
        refresh_token: "managed-refresh".to_string().into(),
        account_id: "acct-1".to_string(),
    };

    let record = backend.register_account(request, Some(tokens)).await?;

    let auth_home = harness
        .runtime
        .codex_home()
        .join(".pooled-auth/backends/local/accounts")
        .join(record.backend_account_handle.as_str());
    let auth = CodexAuth::from_auth_storage(&auth_home, AuthCredentialsStoreMode::File)?
        .expect("pooled auth should be persisted");

    assert_eq!(record.backend_account_handle, "backend-handle-1");
    assert_eq!(auth.get_account_id(), Some("acct-1".to_string()));
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

async fn seed_local_lease_session(
    harness: &TestHarness,
    holder_instance_id: &str,
) -> anyhow::Result<LeaseGrant> {
    let backend = LocalAccountPoolBackend::new(
        harness.runtime.clone(),
        default_config().lease_ttl_duration(),
    );
    backend
        .register_account(
            pooled_account_registration_request("acct-legacy", "backend-handle-legacy"),
            Some(pooled_account_registration_tokens("acct-legacy")),
        )
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

fn pooled_account_registration_request(
    account_id: &str,
    backend_account_handle: &str,
) -> RegisteredAccountUpsert {
    RegisteredAccountUpsert {
        account_id: account_id.to_string(),
        backend_id: "local".to_string(),
        backend_family: "local".to_string(),
        workspace_id: None,
        backend_account_handle: backend_account_handle.to_string(),
        account_kind: "chatgpt".to_string(),
        provider_fingerprint: format!("fingerprint-{account_id}"),
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

fn pooled_account_registration_tokens(account_id: &str) -> ChatgptManagedRegistrationTokens {
    ChatgptManagedRegistrationTokens {
        id_token: fake_access_token(account_id),
        access_token: "managed-access".to_string().into(),
        refresh_token: "managed-refresh".to_string().into(),
        account_id: account_id.to_string(),
    }
}

fn fake_access_token(chatgpt_account_id: &str) -> String {
    let _ = chatgpt_account_id;
    "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJlbWFpbCI6InVzZXJAZXhhbXBsZS5jb20iLCJlbWFpbF92ZXJpZmllZCI6dHJ1ZSwiaHR0cHM6Ly9hcGkub3BlbmFpLmNvbS9hdXRoIjp7ImNoYXRncHRfcGxhbl90eXBlIjoicHJvIiwiY2hhdGdwdF91c2VyX2lkIjoidXNlci0xMjM0NSIsImNoYXRncHRfYWNjb3VudF9pZCI6ImFjY3QtbGVnYWN5In19.c2ln".to_string()
}
