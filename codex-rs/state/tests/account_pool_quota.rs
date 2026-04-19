use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use codex_state::AccountHealthEvent;
use codex_state::AccountHealthState;
use codex_state::AccountQuotaProbeBackoff;
use codex_state::AccountQuotaProbeObservation;
use codex_state::AccountQuotaProbeStillBlocked;
use codex_state::AccountQuotaStateRecord;
use codex_state::AccountRegistryEntryUpdate;
use codex_state::QuotaExhaustedWindows;
use codex_state::QuotaProbeResult;
use codex_state::StateRuntime;
use pretty_assertions::assert_eq;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use uuid::Uuid;

#[tokio::test]
async fn account_pool_quota_rows_are_scoped_by_account_and_limit_family() {
    let harness = AccountPoolQuotaHarness::new().await;
    harness
        .write_quota_observation(quota_row("acct-a", "codex").with_primary_used(82.0))
        .await
        .expect("write codex quota");
    harness
        .write_quota_observation(quota_row("acct-a", "chatgpt").with_primary_used(37.0))
        .await
        .expect("write chatgpt quota");

    let codex = harness
        .read_quota_state("acct-a", "codex")
        .await
        .expect("read codex quota")
        .expect("missing codex quota");
    let chatgpt = harness
        .read_quota_state("acct-a", "chatgpt")
        .await
        .expect("read chatgpt quota")
        .expect("missing chatgpt quota");

    assert_eq!(codex.limit_id, "codex");
    assert_eq!(chatgpt.limit_id, "chatgpt");
    assert_ne!(codex.primary_used_percent, chatgpt.primary_used_percent);
}

#[tokio::test]
async fn account_pool_quota_probe_reservation_only_succeeds_when_next_probe_after_has_elapsed() {
    let harness = AccountPoolQuotaHarness::new().await;
    let now = timestamp(1_000);
    let predicted_blocked_until = now + Duration::hours(1);
    harness
        .write_quota_observation(
            quota_row("acct-a", "codex")
                .with_exhausted_windows(QuotaExhaustedWindows::Secondary)
                .with_predicted_blocked_until(predicted_blocked_until)
                .with_next_probe_after(now - Duration::seconds(1)),
        )
        .await
        .expect("write blocked quota");

    assert!(
        harness
            .reserve_probe_slot("acct-a", "codex", now, now + Duration::seconds(30))
            .await
            .expect("reserve available probe slot")
    );
    assert!(
        !harness
            .reserve_probe_slot("acct-a", "codex", now, now + Duration::seconds(30))
            .await
            .expect("reject second probe reservation")
    );

    let refreshed = harness
        .read_quota_state("acct-a", "codex")
        .await
        .expect("read refreshed quota")
        .expect("missing refreshed quota");
    assert_eq!(
        refreshed.predicted_blocked_until,
        Some(predicted_blocked_until)
    );
    assert_eq!(
        refreshed.next_probe_after,
        Some(now + Duration::seconds(30))
    );
}

#[tokio::test]
async fn account_pool_quota_selection_family_row_wins_before_codex_fallback_and_probe_results_update_backoff()
 {
    let harness = AccountPoolQuotaHarness::new().await;
    let now = timestamp(2_000);
    harness
        .write_quota_observation(quota_row("acct-a", "codex").with_primary_used(12.0))
        .await
        .expect("write codex quota");
    harness
        .write_quota_observation(
            quota_row("acct-a", "chatgpt")
                .with_primary_used(88.0)
                .with_exhausted_windows(QuotaExhaustedWindows::Unknown)
                .with_next_probe_after(now),
        )
        .await
        .expect("write chatgpt quota");

    let selected = harness
        .read_selection_quota_facts("acct-a", "chatgpt")
        .await
        .expect("read selected family quota")
        .expect("missing selected family quota");
    assert_eq!(selected.limit_id, "chatgpt");

    let fallback = harness
        .read_selection_quota_facts("acct-a", "gizmo")
        .await
        .expect("read fallback quota")
        .expect("missing fallback quota");
    assert_eq!(fallback.limit_id, "codex");

    harness
        .record_probe_ambiguous(
            "acct-a",
            "chatgpt",
            AccountQuotaProbeObservation {
                observed_at: now,
                reserved_until: now,
            },
            AccountQuotaProbeBackoff {
                predicted_blocked_until: now + Duration::minutes(10),
                next_probe_after: now + Duration::minutes(10),
            },
        )
        .await
        .expect("record ambiguous probe");

    let refreshed = harness
        .read_quota_state("acct-a", "chatgpt")
        .await
        .expect("read refreshed chatgpt quota")
        .expect("missing refreshed chatgpt quota");
    assert_eq!(
        refreshed.last_probe_result,
        Some(QuotaProbeResult::Ambiguous)
    );
    assert_eq!(refreshed.probe_backoff_level, 1);
    assert_eq!(
        refreshed.predicted_blocked_until,
        Some(now + Duration::minutes(10))
    );
    assert_eq!(
        refreshed.next_probe_after,
        Some(now + Duration::minutes(10))
    );
}

#[tokio::test]
async fn account_pool_quota_legacy_rate_limited_rows_do_not_synthesize_quota_blocking_truth_after_upgrade()
 {
    let harness = AccountPoolQuotaHarness::new().await;
    harness
        .seed_legacy_rate_limited_health("acct-a")
        .await
        .expect("seed legacy rate limited row");

    assert_eq!(
        harness
            .read_selection_quota_facts("acct-a", "codex")
            .await
            .expect("read quota facts"),
        None
    );
}

#[tokio::test]
async fn account_pool_quota_fresh_non_exhausted_observation_immediately_clears_existing_blocked_row()
 {
    let harness = AccountPoolQuotaHarness::new().await;
    let now = timestamp(3_000);
    harness
        .write_quota_observation(
            quota_row("acct-a", "codex")
                .with_exhausted_windows(QuotaExhaustedWindows::Secondary)
                .with_predicted_blocked_until(now + Duration::hours(1))
                .with_next_probe_after(now + Duration::minutes(15)),
        )
        .await
        .expect("write blocked quota");

    harness
        .write_quota_observation(
            quota_row("acct-a", "codex")
                .with_observed_at(now + Duration::minutes(5))
                .with_exhausted_windows(QuotaExhaustedWindows::None),
        )
        .await
        .expect("write recovered quota");

    let refreshed = harness
        .read_quota_state("acct-a", "codex")
        .await
        .expect("read recovered quota")
        .expect("missing recovered quota");
    assert_eq!(refreshed.exhausted_windows, QuotaExhaustedWindows::None);
    assert_eq!(refreshed.predicted_blocked_until, None);
    assert_eq!(refreshed.next_probe_after, None);
}

#[tokio::test]
async fn account_pool_quota_stale_probe_results_do_not_overwrite_fresher_observations() {
    let harness = AccountPoolQuotaHarness::new().await;
    let stale_probe_observed_at = timestamp(5_000);
    let fresh_observed_at = stale_probe_observed_at + Duration::minutes(5);
    let stale_observation = AccountQuotaProbeObservation {
        observed_at: stale_probe_observed_at,
        reserved_until: stale_probe_observed_at + Duration::minutes(15),
    };
    for account_id in ["acct-success", "acct-still-blocked", "acct-ambiguous"] {
        harness
            .write_quota_observation(
                quota_row(account_id, "codex")
                    .with_observed_at(stale_probe_observed_at)
                    .with_exhausted_windows(QuotaExhaustedWindows::Secondary)
                    .with_predicted_blocked_until(stale_probe_observed_at + Duration::hours(1))
                    .with_next_probe_after(stale_probe_observed_at + Duration::minutes(15)),
            )
            .await
            .expect("write initial blocked quota");
        harness
            .write_quota_observation(
                quota_row(account_id, "codex")
                    .with_observed_at(fresh_observed_at)
                    .with_primary_used(3.0)
                    .with_exhausted_windows(QuotaExhaustedWindows::None),
            )
            .await
            .expect("write fresher recovered quota");
    }

    assert!(
        !harness
            .record_probe_success("acct-success", "codex", stale_observation,)
            .await
            .expect("record stale probe success")
    );
    assert!(
        !harness
            .record_probe_still_blocked(
                "acct-still-blocked",
                "codex",
                stale_observation,
                AccountQuotaProbeStillBlocked {
                    exhausted_windows: QuotaExhaustedWindows::Secondary,
                    predicted_blocked_until: Some(stale_probe_observed_at + Duration::hours(1)),
                    next_probe_after: stale_probe_observed_at + Duration::minutes(15),
                },
            )
            .await
            .expect("record stale still-blocked probe")
    );
    assert!(
        !harness
            .record_probe_ambiguous(
                "acct-ambiguous",
                "codex",
                stale_observation,
                AccountQuotaProbeBackoff {
                    predicted_blocked_until: stale_probe_observed_at + Duration::hours(1),
                    next_probe_after: stale_probe_observed_at + Duration::minutes(15),
                },
            )
            .await
            .expect("record stale ambiguous probe")
    );

    for account_id in ["acct-success", "acct-still-blocked", "acct-ambiguous"] {
        let refreshed = harness
            .read_quota_state(account_id, "codex")
            .await
            .expect("read refreshed quota")
            .expect("missing refreshed quota");

        assert_eq!(refreshed.observed_at, fresh_observed_at);
        assert_eq!(refreshed.exhausted_windows, QuotaExhaustedWindows::None);
        assert_eq!(refreshed.predicted_blocked_until, None);
        assert_eq!(refreshed.next_probe_after, None);
        assert_eq!(refreshed.probe_backoff_level, 0);
        assert_eq!(refreshed.last_probe_result, None);
    }
}

#[tokio::test]
async fn account_pool_quota_stale_probe_success_is_rejected_after_same_row_is_rereserved() {
    let harness = AccountPoolQuotaHarness::new().await;
    let observed_at = timestamp(5_500);
    let stale_observation = AccountQuotaProbeObservation {
        observed_at,
        reserved_until: observed_at + Duration::minutes(15),
    };
    let refreshed_reservation = observed_at + Duration::minutes(30);
    harness
        .write_quota_observation(
            quota_row("acct-a", "codex")
                .with_observed_at(observed_at)
                .with_exhausted_windows(QuotaExhaustedWindows::Secondary)
                .with_predicted_blocked_until(observed_at + Duration::hours(1))
                .with_next_probe_after(stale_observation.reserved_until),
        )
        .await
        .expect("write initial blocked quota");
    harness
        .write_quota_observation(
            quota_row("acct-a", "codex")
                .with_observed_at(observed_at)
                .with_exhausted_windows(QuotaExhaustedWindows::Secondary)
                .with_predicted_blocked_until(observed_at + Duration::hours(1))
                .with_next_probe_after(refreshed_reservation),
        )
        .await
        .expect("write reregistered reservation");

    assert!(
        !harness
            .record_probe_success("acct-a", "codex", stale_observation)
            .await
            .expect("reject stale reregistered probe success")
    );

    let quota = harness
        .read_quota_state("acct-a", "codex")
        .await
        .expect("read quota")
        .expect("missing quota");
    assert_eq!(quota.observed_at, observed_at);
    assert_eq!(quota.exhausted_windows, QuotaExhaustedWindows::Secondary);
    assert_eq!(quota.next_probe_after, Some(refreshed_reservation));
    assert_eq!(quota.last_probe_result, None);
}

#[tokio::test]
async fn account_pool_quota_stale_probe_still_blocked_is_rejected_after_same_row_is_rereserved() {
    let harness = AccountPoolQuotaHarness::new().await;
    let observed_at = timestamp(5_600);
    let stale_observation = AccountQuotaProbeObservation {
        observed_at,
        reserved_until: observed_at + Duration::minutes(15),
    };
    let refreshed_reservation = observed_at + Duration::minutes(30);
    harness
        .write_quota_observation(
            quota_row("acct-a", "codex")
                .with_observed_at(observed_at)
                .with_exhausted_windows(QuotaExhaustedWindows::Secondary)
                .with_predicted_blocked_until(observed_at + Duration::hours(1))
                .with_next_probe_after(stale_observation.reserved_until),
        )
        .await
        .expect("write initial blocked quota");
    harness
        .write_quota_observation(
            quota_row("acct-a", "codex")
                .with_observed_at(observed_at)
                .with_exhausted_windows(QuotaExhaustedWindows::Secondary)
                .with_predicted_blocked_until(observed_at + Duration::hours(1))
                .with_next_probe_after(refreshed_reservation),
        )
        .await
        .expect("write reregistered reservation");

    assert!(
        !harness
            .record_probe_still_blocked(
                "acct-a",
                "codex",
                stale_observation,
                AccountQuotaProbeStillBlocked {
                    exhausted_windows: QuotaExhaustedWindows::Secondary,
                    predicted_blocked_until: Some(observed_at + Duration::hours(1)),
                    next_probe_after: observed_at + Duration::minutes(45),
                },
            )
            .await
            .expect("reject stale reregistered still-blocked probe")
    );

    let quota = harness
        .read_quota_state("acct-a", "codex")
        .await
        .expect("read quota")
        .expect("missing quota");
    assert_eq!(quota.observed_at, observed_at);
    assert_eq!(quota.exhausted_windows, QuotaExhaustedWindows::Secondary);
    assert_eq!(quota.next_probe_after, Some(refreshed_reservation));
    assert_eq!(quota.last_probe_result, None);
}

#[tokio::test]
async fn account_pool_quota_same_second_fresher_live_observation_beats_stale_probe() {
    let harness = AccountPoolQuotaHarness::new().await;
    let same_second = timestamp_with_nanos(6_000, 0);
    let stale_probe_observed_at = timestamp_with_nanos(6_000, 100_000_000);
    let fresh_observed_at = timestamp_with_nanos(6_000, 900_000_000);
    let stale_observation = AccountQuotaProbeObservation {
        observed_at: stale_probe_observed_at,
        reserved_until: same_second + Duration::minutes(15),
    };
    harness
        .write_quota_observation(
            quota_row("acct-a", "codex")
                .with_observed_at(same_second)
                .with_exhausted_windows(QuotaExhaustedWindows::Secondary)
                .with_predicted_blocked_until(same_second + Duration::hours(1))
                .with_next_probe_after(same_second + Duration::minutes(15)),
        )
        .await
        .expect("write initial blocked quota");
    harness
        .write_quota_observation(
            quota_row("acct-a", "codex")
                .with_observed_at(fresh_observed_at)
                .with_primary_used(5.0)
                .with_exhausted_windows(QuotaExhaustedWindows::None),
        )
        .await
        .expect("write fresher same-second observation");

    assert!(
        !harness
            .record_probe_still_blocked(
                "acct-a",
                "codex",
                stale_observation,
                AccountQuotaProbeStillBlocked {
                    exhausted_windows: QuotaExhaustedWindows::Secondary,
                    predicted_blocked_until: Some(stale_probe_observed_at + Duration::hours(1)),
                    next_probe_after: stale_probe_observed_at + Duration::minutes(15),
                },
            )
            .await
            .expect("record stale same-second probe")
    );

    let refreshed = harness
        .read_quota_state("acct-a", "codex")
        .await
        .expect("read refreshed quota")
        .expect("missing refreshed quota");
    assert_eq!(refreshed.observed_at, fresh_observed_at);
    assert_eq!(refreshed.exhausted_windows, QuotaExhaustedWindows::None);
    assert_eq!(refreshed.predicted_blocked_until, None);
    assert_eq!(refreshed.next_probe_after, None);
    assert_eq!(refreshed.last_probe_result, None);
}

#[tokio::test]
async fn account_pool_quota_selection_family_does_not_splice_nullable_codex_fields() {
    let harness = AccountPoolQuotaHarness::new().await;
    let family_observed_at = timestamp(7_000);
    harness
        .write_quota_observation(
            quota_row("acct-a", "codex")
                .with_observed_at(timestamp_with_nanos(7_000, 100_000_000))
                .with_exhausted_windows(QuotaExhaustedWindows::Secondary)
                .with_predicted_blocked_until(timestamp(7_600))
                .with_next_probe_after(timestamp(7_300)),
        )
        .await
        .expect("write codex quota");
    harness
        .write_quota_observation(
            quota_row("acct-a", "chatgpt")
                .with_observed_at(family_observed_at)
                .with_primary_used(91.0)
                .with_exhausted_windows(QuotaExhaustedWindows::Unknown),
        )
        .await
        .expect("write family quota with null timings");

    let selected = harness
        .read_selection_quota_facts("acct-a", "chatgpt")
        .await
        .expect("read selected family quota")
        .expect("missing family quota");
    assert_eq!(selected.limit_id, "chatgpt");
    assert_eq!(selected.observed_at, family_observed_at);
    assert_eq!(selected.predicted_blocked_until, None);
    assert_eq!(selected.next_probe_after, None);
}

struct AccountPoolQuotaHarness {
    runtime: Arc<StateRuntime>,
}

impl AccountPoolQuotaHarness {
    async fn new() -> Self {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .unwrap_or_else(|err| panic!("initialize runtime: {err}"));
        Self { runtime }
    }

    async fn write_quota_observation(&self, record: AccountQuotaStateRecord) -> anyhow::Result<()> {
        self.ensure_account(&record.account_id).await?;
        self.runtime.upsert_account_quota_state(record).await
    }

    async fn read_quota_state(
        &self,
        account_id: &str,
        limit_id: &str,
    ) -> anyhow::Result<Option<AccountQuotaStateRecord>> {
        self.runtime
            .read_account_quota_state(account_id, limit_id)
            .await
    }

    async fn read_selection_quota_facts(
        &self,
        account_id: &str,
        selection_family: &str,
    ) -> anyhow::Result<Option<AccountQuotaStateRecord>> {
        self.runtime
            .read_selection_quota_state(account_id, selection_family)
            .await
    }

    async fn reserve_probe_slot(
        &self,
        account_id: &str,
        limit_id: &str,
        now: DateTime<Utc>,
        reserved_until: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        self.runtime
            .reserve_account_quota_probe(account_id, limit_id, now, reserved_until)
            .await
    }

    async fn record_probe_ambiguous(
        &self,
        account_id: &str,
        limit_id: &str,
        observation: AccountQuotaProbeObservation,
        backoff: AccountQuotaProbeBackoff,
    ) -> anyhow::Result<bool> {
        self.runtime
            .record_account_quota_probe_ambiguous(account_id, limit_id, observation, backoff)
            .await
    }

    async fn record_probe_success(
        &self,
        account_id: &str,
        limit_id: &str,
        observation: AccountQuotaProbeObservation,
    ) -> anyhow::Result<bool> {
        self.runtime
            .record_account_quota_probe_success(account_id, limit_id, observation)
            .await
    }

    async fn record_probe_still_blocked(
        &self,
        account_id: &str,
        limit_id: &str,
        observation: AccountQuotaProbeObservation,
        update: AccountQuotaProbeStillBlocked,
    ) -> anyhow::Result<bool> {
        self.runtime
            .record_account_quota_probe_still_blocked(account_id, limit_id, observation, update)
            .await
    }

    async fn seed_legacy_rate_limited_health(&self, account_id: &str) -> anyhow::Result<()> {
        self.ensure_account(account_id).await?;
        self.runtime
            .record_account_health_event(AccountHealthEvent {
                account_id: account_id.to_string(),
                pool_id: "pool-main".to_string(),
                health_state: AccountHealthState::RateLimited,
                sequence_number: 1,
                observed_at: timestamp(4_000),
            })
            .await
    }

    async fn ensure_account(&self, account_id: &str) -> anyhow::Result<()> {
        self.runtime
            .upsert_account_registry_entry(AccountRegistryEntryUpdate {
                account_id: account_id.to_string(),
                pool_id: "pool-main".to_string(),
                position: 0,
                account_kind: "chatgpt".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some("workspace-main".to_string()),
                enabled: true,
                healthy: true,
            })
            .await
    }
}

fn quota_row(account_id: &str, limit_id: &str) -> AccountQuotaStateRecord {
    AccountQuotaStateRecord {
        account_id: account_id.to_string(),
        limit_id: limit_id.to_string(),
        primary_used_percent: None,
        primary_resets_at: None,
        secondary_used_percent: None,
        secondary_resets_at: None,
        observed_at: timestamp(1),
        exhausted_windows: QuotaExhaustedWindows::None,
        predicted_blocked_until: None,
        next_probe_after: None,
        probe_backoff_level: 0,
        last_probe_result: None,
    }
}

trait AccountQuotaStateRecordExt {
    fn with_primary_used(self, primary_used_percent: f64) -> Self;
    fn with_observed_at(self, observed_at: DateTime<Utc>) -> Self;
    fn with_exhausted_windows(self, exhausted_windows: QuotaExhaustedWindows) -> Self;
    fn with_predicted_blocked_until(self, predicted_blocked_until: DateTime<Utc>) -> Self;
    fn with_next_probe_after(self, next_probe_after: DateTime<Utc>) -> Self;
}

impl AccountQuotaStateRecordExt for AccountQuotaStateRecord {
    fn with_primary_used(mut self, primary_used_percent: f64) -> Self {
        self.primary_used_percent = Some(primary_used_percent);
        self
    }

    fn with_observed_at(mut self, observed_at: DateTime<Utc>) -> Self {
        self.observed_at = observed_at;
        self
    }

    fn with_exhausted_windows(mut self, exhausted_windows: QuotaExhaustedWindows) -> Self {
        self.exhausted_windows = exhausted_windows;
        self
    }

    fn with_predicted_blocked_until(mut self, predicted_blocked_until: DateTime<Utc>) -> Self {
        self.predicted_blocked_until = Some(predicted_blocked_until);
        self
    }

    fn with_next_probe_after(mut self, next_probe_after: DateTime<Utc>) -> Self {
        self.next_probe_after = Some(next_probe_after);
        self
    }
}

fn timestamp(seconds: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(seconds, 0).unwrap_or_else(|| panic!("timestamp {seconds}"))
}

fn timestamp_with_nanos(seconds: i64, nanos: u32) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(seconds, nanos)
        .unwrap_or_else(|| panic!("timestamp {seconds}.{nanos}"))
}

fn unique_temp_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!(
        "codex-state-account-pool-quota-test-{nanos}-{}",
        Uuid::new_v4()
    ))
}
