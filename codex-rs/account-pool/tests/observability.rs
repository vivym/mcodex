use chrono::Duration;
use codex_account_pool::AccountOperationalState;
use codex_account_pool::AccountPoolAccountsListRequest;
use codex_account_pool::AccountPoolAccountsPage;
use codex_account_pool::AccountPoolDiagnostics;
use codex_account_pool::AccountPoolDiagnosticsReadRequest;
use codex_account_pool::AccountPoolEventType;
use codex_account_pool::AccountPoolEventsListRequest;
use codex_account_pool::AccountPoolEventsPage;
use codex_account_pool::AccountPoolObservabilityReader;
use codex_account_pool::AccountPoolReadRequest;
use codex_account_pool::AccountPoolSnapshot;
use codex_account_pool::LocalAccountPoolBackend;
use codex_state::AccountHealthEvent;
use codex_state::AccountHealthState;
use codex_state::AccountPoolAccountsListQuery;
use codex_state::AccountPoolEventRecord;
use codex_state::AccountPoolEventsListQuery;
use codex_state::AccountQuotaStateRecord;
use codex_state::AccountRegistryEntryUpdate;
use codex_state::QuotaExhaustedWindows;
use codex_state::StateRuntime;
use pretty_assertions::assert_eq;
use std::sync::Arc;

#[tokio::test]
async fn local_observability_reader_passthroughs_runtime_reads() {
    let runtime = test_runtime().await;
    seed_account(&runtime, "acct-1", "team-main", 0, true, true).await;
    seed_account(&runtime, "acct-2", "team-main", 1, true, true).await;
    expect_ok(
        runtime
            .acquire_account_lease("team-main", "inst-a", Duration::seconds(300))
            .await,
    );
    expect_ok(
        runtime
            .append_account_pool_event(test_event(
                "evt-1",
                100,
                "team-main",
                Some("acct-1"),
                "leaseAcquired",
            ))
            .await,
    );
    expect_ok(
        runtime
            .record_account_health_event(AccountHealthEvent {
                account_id: "acct-2".to_string(),
                pool_id: "team-main".to_string(),
                health_state: AccountHealthState::Unauthorized,
                sequence_number: 1,
                observed_at: timestamp(10),
            })
            .await,
    );

    let backend = LocalAccountPoolBackend::new(runtime.clone(), Duration::seconds(300));

    let observed_pool = expect_ok(
        backend
            .read_pool(AccountPoolReadRequest {
                pool_id: "team-main".to_string(),
            })
            .await,
    );
    let expected_pool = expect_ok(
        runtime
            .read_account_pool_snapshot("team-main")
            .await
            .and_then(AccountPoolSnapshot::try_from),
    );
    assert_eq!(
        normalize_snapshot(observed_pool),
        normalize_snapshot(expected_pool)
    );

    let observed_accounts = expect_ok(
        backend
            .list_accounts(AccountPoolAccountsListRequest {
                pool_id: "team-main".to_string(),
                account_id: None,
                cursor: None,
                limit: Some(10),
                states: None,
                account_kinds: None,
            })
            .await,
    );
    let expected_accounts = expect_ok(
        runtime
            .list_account_pool_accounts(AccountPoolAccountsListQuery {
                pool_id: "team-main".to_string(),
                account_id: None,
                cursor: None,
                limit: Some(10),
                states: None,
                account_kinds: None,
            })
            .await
            .and_then(AccountPoolAccountsPage::try_from),
    );
    assert_eq!(observed_accounts, expected_accounts);

    let observed_events = expect_ok(
        backend
            .list_events(AccountPoolEventsListRequest {
                pool_id: "team-main".to_string(),
                account_id: None,
                types: None,
                cursor: None,
                limit: Some(10),
            })
            .await,
    );
    let expected_events = expect_ok(
        runtime
            .list_account_pool_events(AccountPoolEventsListQuery {
                pool_id: "team-main".to_string(),
                account_id: None,
                types: None,
                cursor: None,
                limit: Some(10),
            })
            .await
            .and_then(AccountPoolEventsPage::try_from),
    );
    assert_eq!(observed_events, expected_events);

    let observed_diagnostics = expect_ok(
        backend
            .read_diagnostics(AccountPoolDiagnosticsReadRequest {
                pool_id: "team-main".to_string(),
            })
            .await,
    );
    let expected_diagnostics = expect_ok(
        runtime
            .read_account_pool_diagnostics("team-main")
            .await
            .and_then(AccountPoolDiagnostics::try_from),
    );
    assert_eq!(
        normalize_diagnostics(observed_diagnostics),
        normalize_diagnostics(expected_diagnostics)
    );
}

#[tokio::test]
async fn local_observability_reader_forwards_account_and_event_filters() {
    let runtime = test_runtime().await;
    seed_account(&runtime, "acct-1", "team-main", 0, true, true).await;
    seed_account(&runtime, "acct-2", "team-main", 1, true, true).await;
    expect_ok(
        runtime
            .acquire_account_lease("team-main", "inst-a", Duration::seconds(300))
            .await,
    );
    expect_ok(
        runtime
            .append_account_pool_event(test_event(
                "evt-1",
                100,
                "team-main",
                Some("acct-1"),
                "leaseAcquired",
            ))
            .await,
    );
    expect_ok(
        runtime
            .append_account_pool_event(test_event(
                "evt-2",
                101,
                "team-main",
                Some("acct-2"),
                "leaseReleased",
            ))
            .await,
    );

    let backend = LocalAccountPoolBackend::new(runtime.clone(), Duration::seconds(300));

    let observed_accounts = expect_ok(
        backend
            .list_accounts(AccountPoolAccountsListRequest {
                pool_id: "team-main".to_string(),
                account_id: None,
                cursor: None,
                limit: Some(10),
                states: Some(vec![AccountOperationalState::Available]),
                account_kinds: Some(vec!["chatgpt".to_string()]),
            })
            .await,
    );
    let expected_accounts = expect_ok(
        runtime
            .list_account_pool_accounts(AccountPoolAccountsListQuery {
                pool_id: "team-main".to_string(),
                account_id: None,
                cursor: None,
                limit: Some(10),
                states: Some(vec!["available".to_string()]),
                account_kinds: Some(vec!["chatgpt".to_string()]),
            })
            .await
            .and_then(AccountPoolAccountsPage::try_from),
    );
    assert_eq!(observed_accounts, expected_accounts);

    let observed_events = expect_ok(
        backend
            .list_events(AccountPoolEventsListRequest {
                pool_id: "team-main".to_string(),
                account_id: Some("acct-1".to_string()),
                types: Some(vec![AccountPoolEventType::LeaseAcquired]),
                cursor: None,
                limit: Some(10),
            })
            .await,
    );
    let expected_events = expect_ok(
        runtime
            .list_account_pool_events(AccountPoolEventsListQuery {
                pool_id: "team-main".to_string(),
                account_id: Some("acct-1".to_string()),
                types: Some(vec!["leaseAcquired".to_string()]),
                cursor: None,
                limit: Some(10),
            })
            .await
            .and_then(AccountPoolEventsPage::try_from),
    );
    assert_eq!(observed_events, expected_events);
}

#[tokio::test]
async fn local_observability_reader_keeps_nulls_for_unknown_state() {
    let runtime = test_runtime().await;
    seed_account(&runtime, "acct-1", "team-main", 0, false, true).await;

    let backend = LocalAccountPoolBackend::new(runtime.clone(), Duration::seconds(300));
    let observed = expect_ok(
        backend
            .list_accounts(AccountPoolAccountsListRequest {
                pool_id: "team-main".to_string(),
                account_id: None,
                cursor: None,
                limit: Some(10),
                states: None,
                account_kinds: None,
            })
            .await,
    );
    let expected = expect_ok(
        runtime
            .list_account_pool_accounts(AccountPoolAccountsListQuery {
                pool_id: "team-main".to_string(),
                account_id: None,
                cursor: None,
                limit: Some(10),
                states: None,
                account_kinds: None,
            })
            .await
            .and_then(AccountPoolAccountsPage::try_from),
    );

    assert_eq!(observed, expected);
}

#[tokio::test]
async fn local_observability_reader_preserves_quota_families_and_compat_projection() {
    let runtime = test_runtime().await;
    seed_account(&runtime, "acct-1", "team-main", 0, true, true).await;
    expect_ok(
        runtime
            .upsert_account_quota_state(test_quota_state("acct-1", "codex", 82.0, 120))
            .await,
    );
    expect_ok(
        runtime
            .upsert_account_quota_state(test_quota_state("acct-1", "chatgpt", 72.0, 180))
            .await,
    );

    let backend = LocalAccountPoolBackend::new(runtime.clone(), Duration::seconds(300));
    let observed = expect_ok(
        backend
            .list_accounts(AccountPoolAccountsListRequest {
                pool_id: "team-main".to_string(),
                account_id: None,
                cursor: None,
                limit: Some(10),
                states: None,
                account_kinds: None,
            })
            .await,
    );
    let expected = expect_ok(
        runtime
            .list_account_pool_accounts(AccountPoolAccountsListQuery {
                pool_id: "team-main".to_string(),
                account_id: None,
                cursor: None,
                limit: Some(10),
                states: None,
                account_kinds: None,
            })
            .await
            .and_then(AccountPoolAccountsPage::try_from),
    );

    assert_eq!(observed, expected);
    assert_eq!(observed.data[0].quotas.len(), 2);
    assert_eq!(observed.data[0].quotas[0].limit_id, "chatgpt");
    assert_eq!(observed.data[0].quotas[1].limit_id, "codex");
    assert_eq!(
        observed.data[0]
            .quota
            .as_ref()
            .and_then(|quota| quota.remaining_percent),
        Some(18.0)
    );
}

async fn test_runtime() -> Arc<StateRuntime> {
    let codex_home = expect_ok(tempfile::tempdir()).keep();
    expect_ok(StateRuntime::init(codex_home, "test-provider".to_string()).await)
}

async fn seed_account(
    runtime: &StateRuntime,
    account_id: &str,
    pool_id: &str,
    position: i64,
    enabled: bool,
    healthy: bool,
) {
    expect_ok(
        runtime
            .upsert_account_registry_entry(AccountRegistryEntryUpdate {
                account_id: account_id.to_string(),
                pool_id: pool_id.to_string(),
                position,
                account_kind: "chatgpt".to_string(),
                backend_family: "local".to_string(),
                workspace_id: None,
                enabled,
                healthy,
            })
            .await,
    );
    expect_ok(
        runtime
            .record_account_health_event(AccountHealthEvent {
                account_id: account_id.to_string(),
                pool_id: pool_id.to_string(),
                health_state: AccountHealthState::Healthy,
                sequence_number: 1,
                observed_at: timestamp(1),
            })
            .await,
    );
}

fn normalize_snapshot(mut snapshot: AccountPoolSnapshot) -> AccountPoolSnapshot {
    snapshot.refreshed_at = timestamp(0);
    snapshot
}

fn normalize_diagnostics(mut diagnostics: AccountPoolDiagnostics) -> AccountPoolDiagnostics {
    diagnostics.generated_at = timestamp(0);
    diagnostics
}

fn test_event(
    event_id: &str,
    occurred_at: i64,
    pool_id: &str,
    account_id: Option<&str>,
    event_type: &str,
) -> AccountPoolEventRecord {
    AccountPoolEventRecord {
        event_id: event_id.to_string(),
        occurred_at: timestamp(occurred_at),
        pool_id: pool_id.to_string(),
        account_id: account_id.map(str::to_owned),
        lease_id: None,
        holder_instance_id: None,
        event_type: event_type.to_string(),
        reason_code: None,
        message: format!("{event_type} event"),
        details_json: None,
    }
}

fn test_quota_state(
    account_id: &str,
    limit_id: &str,
    used_percent: f64,
    resets_at: i64,
) -> AccountQuotaStateRecord {
    AccountQuotaStateRecord {
        account_id: account_id.to_string(),
        limit_id: limit_id.to_string(),
        primary_used_percent: Some(used_percent),
        primary_resets_at: Some(timestamp(resets_at)),
        secondary_used_percent: Some(10.0),
        secondary_resets_at: Some(timestamp(resets_at + 60)),
        observed_at: timestamp(60),
        exhausted_windows: QuotaExhaustedWindows::Primary,
        predicted_blocked_until: Some(timestamp(resets_at)),
        next_probe_after: Some(timestamp(resets_at - 30)),
        probe_backoff_level: 1,
        last_probe_result: None,
    }
}

fn timestamp(seconds: i64) -> chrono::DateTime<chrono::Utc> {
    match chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0) {
        Some(value) => value,
        None => panic!("invalid unix timestamp"),
    }
}

fn expect_ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("unexpected error: {err:?}"),
    }
}
