use chrono::Utc;
use codex_account_pool::AccountPoolConfig;
use codex_account_pool::AccountPoolManager;
use codex_account_pool::HealthEventDisposition;
use codex_account_pool::LegacyAuthBootstrap;
use codex_account_pool::LocalAccountPoolBackend;
use codex_account_pool::RateLimitSnapshot;
use codex_account_pool::SelectionRequest;
use codex_account_pool::UsageLimitEvent;
use codex_state::LegacyAccountImport;
use codex_state::StateRuntime;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use tempfile::TempDir;

#[tokio::test]
async fn ensure_active_lease_reuses_sticky_account_until_threshold() {
    let mut harness = fixture_manager_with_legacy_auth("acct-legacy").await;
    let first = harness
        .manager
        .ensure_active_lease(SelectionRequest::default())
        .await
        .unwrap();

    harness
        .manager
        .report_rate_limits(first.key(), snapshot(/* used_percent */ 70.0))
        .await
        .unwrap();

    let second = harness
        .manager
        .ensure_active_lease(SelectionRequest::default())
        .await
        .unwrap();

    assert_eq!(first.account_id(), second.account_id());
}

#[tokio::test]
async fn stale_holder_health_event_is_ignored_after_epoch_bump() {
    let mut harness = fixture_manager_with_legacy_auth("acct-legacy").await;
    let lease = harness
        .manager
        .ensure_active_lease(SelectionRequest::default())
        .await
        .unwrap();
    harness
        .manager
        .force_epoch_bump_for_test(lease.account_id())
        .unwrap();

    let result = harness
        .manager
        .report_usage_limit_reached(lease.key(), usage_limit_event())
        .await;

    assert_eq!(result.unwrap(), HealthEventDisposition::IgnoredAsStale);
}

#[tokio::test]
async fn bootstrap_imports_legacy_default_only_once() {
    let mut harness = fixture_manager_with_legacy_auth("acct-legacy").await;

    harness.manager.bootstrap_from_legacy_auth().await.unwrap();
    harness.manager.bootstrap_from_legacy_auth().await.unwrap();

    let selection = harness
        .manager
        .read_startup_selection_for_test()
        .await
        .unwrap();

    assert_eq!(
        selection.preferred_account_id.as_deref(),
        Some("acct-legacy")
    );
    assert_eq!(harness.bootstrap_calls.load(Ordering::SeqCst), 1);
}

struct TestHarness {
    manager: AccountPoolManager<LocalAccountPoolBackend, TestLegacyBootstrap>,
    bootstrap_calls: Arc<AtomicUsize>,
    _tempdir: TempDir,
}

async fn fixture_manager_with_legacy_auth(account_id: &str) -> TestHarness {
    let tempdir = tempfile::tempdir().unwrap();
    let runtime = StateRuntime::init(tempdir.path().to_path_buf(), "test-provider".to_string())
        .await
        .unwrap();
    let backend = LocalAccountPoolBackend::new(runtime);
    let (legacy_bootstrap, bootstrap_calls) = TestLegacyBootstrap::with_legacy_account(account_id);
    let config = AccountPoolConfig {
        default_pool_id: Some("legacy-default".to_string()),
        ..AccountPoolConfig::default()
    };
    let manager =
        AccountPoolManager::new(backend, legacy_bootstrap, config, "test-holder".to_string())
            .unwrap();

    TestHarness {
        manager,
        bootstrap_calls,
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
