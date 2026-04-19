use chrono::DateTime;
use chrono::Duration;
use chrono::TimeZone;
use chrono::Utc;
use codex_account_pool::AccountKind;
use codex_account_pool::AccountRecord;
use codex_account_pool::QuotaFamilyView;
use codex_account_pool::SelectionAction;
use codex_account_pool::SelectionDecisionReason;
use codex_account_pool::SelectionIntent;
use codex_account_pool::SelectionPlan;
use codex_account_pool::SelectionRejectReason;
use codex_account_pool::SelectionRequest;
use codex_account_pool::build_selection_plan;
use codex_state::AccountQuotaStateRecord;
use codex_state::QuotaExhaustedWindows;
use pretty_assertions::assert_eq;

#[test]
fn quota_selection_secondary_exhausted_veto_before_primary_ranking() {
    let plan = build_selection_plan(selection_request(SelectionIntent::Startup))
        .with_candidate(candidate("acct-hot").with_secondary_exhausted())
        .with_candidate(candidate("acct-cool").with_primary_used(41.0))
        .run();

    assert_eq!(
        plan.terminal_action,
        SelectionAction::Select("acct-cool".to_string())
    );
    assert_rejected_reason(&plan, "acct-hot", SelectionRejectReason::PredictedBlocked);
}

#[test]
fn quota_selection_soft_rotation_stays_on_current_when_no_other_admissible_candidate_exists() {
    let plan = build_selection_plan(
        selection_request(SelectionIntent::SoftRotation)
            .with_current_account("acct-a")
            .with_just_replaced_account("acct-b"),
    )
    .with_candidate(candidate("acct-a").with_primary_used(91.0))
    .with_candidate(candidate("acct-b").with_primary_used(20.0))
    .run();

    assert_eq!(plan.terminal_action, SelectionAction::StayOnCurrent);
}

#[test]
fn quota_selection_stale_primary_blocked_account_becomes_probe_candidate_after_ordinary_candidates_exhaust()
 {
    let plan = build_selection_plan(selection_request(SelectionIntent::HardFailover))
        .with_candidate(candidate("acct-a").with_primary_block(now_minus_minutes(20)))
        .run();

    assert_eq!(
        plan.terminal_action,
        SelectionAction::Probe("acct-a".to_string())
    );
}

#[test]
fn quota_selection_missing_partial_quota_data_stays_not_blocked_but_ranks_below_complete_rows() {
    let plan = build_selection_plan(selection_request(SelectionIntent::Startup))
        .with_candidate(candidate("acct-low-confidence").with_missing_secondary_window())
        .with_candidate(
            candidate("acct-complete")
                .with_primary_used(48.0)
                .with_secondary_used(22.0),
        )
        .run();

    assert_eq!(plan.eligible_candidates[0].account_id, "acct-complete");
    assert_eq!(
        plan.eligible_candidates[1].account_id,
        "acct-low-confidence"
    );
}

#[test]
fn quota_selection_selection_family_row_used_before_codex_fallback() {
    let plan = build_selection_plan(
        selection_request(SelectionIntent::HardFailover).with_selection_family("chatgpt"),
    )
    .with_candidate(
        candidate("acct-a")
            .with_family_quota("codex", healthy_primary(10.0))
            .with_family_quota("chatgpt", exhausted_primary()),
    )
    .with_candidate(candidate("acct-b").with_family_quota("chatgpt", healthy_primary(44.0)))
    .run();

    assert_eq!(
        plan.terminal_action,
        SelectionAction::Select("acct-b".to_string())
    );
}

#[test]
fn quota_selection_hard_failover_may_reuse_just_replaced_account_and_reports_decision_reason() {
    let plan = build_selection_plan(
        selection_request(SelectionIntent::HardFailover)
            .with_current_account("acct-a")
            .with_just_replaced_account("acct-b"),
    )
    .with_candidate(candidate("acct-a").with_primary_used(99.0))
    .with_candidate(candidate("acct-b").with_primary_used(18.0))
    .run();

    assert_eq!(
        plan.terminal_action,
        SelectionAction::Select("acct-b".to_string())
    );
    assert_eq!(
        plan.decision_reason,
        SelectionDecisionReason::HardFailoverOverride
    );
}

#[test]
fn quota_selection_probe_recovery_only_rechecks_reserved_target_and_returns_probe_for_that_target()
{
    let plan = build_selection_plan(
        selection_request(SelectionIntent::ProbeRecovery).with_reserved_probe_target("acct-probe"),
    )
    .with_candidate(candidate("acct-probe").with_primary_block(now_minus_minutes(30)))
    .run();

    assert_eq!(plan.probe_candidate.as_deref(), Some("acct-probe"));
    assert!(plan.eligible_candidates.is_empty());
    assert_eq!(
        plan.terminal_action,
        SelectionAction::Probe("acct-probe".to_string())
    );
}

#[test]
fn quota_selection_probe_recovery_returns_no_candidate_until_reserved_target_becomes_probe_eligible()
 {
    let plan = build_selection_plan(
        selection_request(SelectionIntent::ProbeRecovery).with_reserved_probe_target("acct-probe"),
    )
    .with_candidate(candidate("acct-probe").with_primary_block(now_plus_minutes(30)))
    .run();

    assert!(plan.eligible_candidates.is_empty());
    assert_eq!(plan.probe_candidate, None);
    assert_eq!(plan.decision_reason, SelectionDecisionReason::NoCandidate);
    assert_eq!(plan.terminal_action, SelectionAction::NoCandidate);
    assert_rejected_reason(&plan, "acct-probe", SelectionRejectReason::PredictedBlocked);
}

#[test]
fn quota_selection_primary_threshold_beats_later_tie_breakers() {
    let plan = build_selection_plan(
        selection_request(SelectionIntent::Startup).with_proactive_threshold_percent(85),
    )
    .with_candidate(
        candidate("acct-threshold-hot")
            .with_primary_used(87.0)
            .with_secondary_used(5.0),
    )
    .with_candidate(
        candidate("acct-threshold-safe")
            .with_primary_used(52.0)
            .with_secondary_used(40.0),
    )
    .run();

    assert_eq!(
        plan.eligible_candidates[0].account_id,
        "acct-threshold-safe"
    );
}

#[test]
fn quota_selection_secondary_safety_breaks_primary_ties_before_reset_ordering() {
    let plan = build_selection_plan(selection_request(SelectionIntent::Startup))
        .with_candidate(
            candidate("acct-reset-early")
                .with_primary_used(40.0)
                .with_secondary_used(30.0)
                .with_primary_reset(now_plus_minutes(5)),
        )
        .with_candidate(
            candidate("acct-secondary-safe")
                .with_primary_used(40.0)
                .with_secondary_used(10.0)
                .with_primary_reset(now_plus_minutes(20)),
        )
        .run();

    assert_eq!(
        plan.eligible_candidates[0].account_id,
        "acct-secondary-safe"
    );
}

#[test]
fn quota_selection_ranking_uses_primary_then_secondary_then_reset_then_stable_tie_breakers() {
    let plan = build_selection_plan(selection_request(SelectionIntent::Startup))
        .with_candidate(
            candidate("acct-b")
                .with_primary_used(40.0)
                .with_secondary_used(30.0),
        )
        .with_candidate(
            candidate("acct-a")
                .with_primary_used(40.0)
                .with_secondary_used(30.0)
                .with_primary_reset(now_plus_minutes(5)),
        )
        .run();

    assert_eq!(plan.eligible_candidates[0].account_id, "acct-a");
}

#[test]
fn quota_selection_ranking_uses_pool_position_then_account_id_as_stable_tie_breakers() {
    let plan = build_selection_plan(selection_request(SelectionIntent::Startup))
        .with_candidate(
            candidate("acct-b")
                .with_primary_used(40.0)
                .with_secondary_used(30.0)
                .with_primary_reset(now_plus_minutes(5))
                .with_pool_position(2),
        )
        .with_candidate(
            candidate("acct-a")
                .with_primary_used(40.0)
                .with_secondary_used(30.0)
                .with_primary_reset(now_plus_minutes(5))
                .with_pool_position(2),
        )
        .with_candidate(
            candidate("acct-front")
                .with_primary_used(40.0)
                .with_secondary_used(30.0)
                .with_primary_reset(now_plus_minutes(5))
                .with_pool_position(1),
        )
        .run();

    assert_eq!(
        plan.eligible_candidates
            .into_iter()
            .map(|candidate| candidate.account_id)
            .collect::<Vec<_>>(),
        vec![
            "acct-front".to_string(),
            "acct-a".to_string(),
            "acct-b".to_string(),
        ]
    );
}

#[test]
fn quota_selection_reprobe_prefers_stale_primary_block_over_fresher_secondary_block() {
    let plan = build_selection_plan(selection_request(SelectionIntent::HardFailover))
        .with_candidate(candidate("acct-secondary").with_secondary_block(now_minus_minutes(5)))
        .with_candidate(candidate("acct-primary").with_primary_block(now_minus_minutes(40)))
        .run();

    assert_eq!(
        plan.terminal_action,
        SelectionAction::Probe("acct-primary".to_string())
    );
}

#[test]
fn quota_selection_exhausted_row_stays_blocked_after_predicted_blocked_until_until_cleared_by_new_fact()
 {
    let plan = build_selection_plan(
        selection_request(SelectionIntent::Startup).with_now(now_plus_hours(2)),
    )
    .with_candidate(
        candidate("acct-a")
            .with_exhausted_windows(QuotaExhaustedWindows::Secondary)
            .with_predicted_blocked_until(now_plus_minutes(5)),
    )
    .run();

    assert_eq!(
        plan.terminal_action,
        SelectionAction::Probe("acct-a".to_string())
    );
    assert_rejected_reason(&plan, "acct-a", SelectionRejectReason::PredictedBlocked);
}

fn assert_rejected_reason(
    plan: &SelectionPlan,
    account_id: &str,
    expected_reason: SelectionRejectReason,
) {
    let Some(rejected) = plan
        .rejected_candidates
        .iter()
        .find(|candidate| candidate.account_id == account_id)
    else {
        panic!("candidate should be rejected");
    };
    assert_eq!(rejected.reason, expected_reason);
}

fn selection_request(intent: SelectionIntent) -> SelectionRequest {
    SelectionRequest::for_intent(intent).with_now(now())
}

fn candidate(account_id: &str) -> CandidateBuilder {
    CandidateBuilder::new(account_id)
}

fn now() -> DateTime<Utc> {
    let Some(now) = Utc.with_ymd_and_hms(2026, 4, 19, 12, 0, 0).single() else {
        panic!("valid timestamp");
    };
    now
}

fn now_minus_minutes(minutes: i64) -> DateTime<Utc> {
    now() - Duration::minutes(minutes)
}

fn now_plus_minutes(minutes: i64) -> DateTime<Utc> {
    now() + Duration::minutes(minutes)
}

fn now_plus_hours(hours: i64) -> DateTime<Utc> {
    now() + Duration::hours(hours)
}

fn healthy_primary(primary_used_percent: f64) -> AccountQuotaStateRecord {
    quota_row()
        .with_primary_used_percent(primary_used_percent)
        .with_exhausted_windows(QuotaExhaustedWindows::None)
        .build()
}

fn exhausted_primary() -> AccountQuotaStateRecord {
    quota_row()
        .with_primary_used_percent(99.0)
        .with_exhausted_windows(QuotaExhaustedWindows::Primary)
        .with_predicted_blocked_until(Some(now_plus_minutes(30)))
        .with_next_probe_after(Some(now_plus_minutes(15)))
        .build()
}

fn quota_row() -> QuotaRowBuilder {
    QuotaRowBuilder::default()
}

struct CandidateBuilder {
    record: AccountRecord,
}

impl CandidateBuilder {
    fn new(account_id: &str) -> Self {
        Self {
            record: AccountRecord {
                account_id: account_id.to_string(),
                healthy: true,
                kind: AccountKind::ChatGpt,
                enabled: true,
                selector_auth_eligible: true,
                pool_position: 0,
                leased_to_other_holder: false,
                quota: QuotaFamilyView::default(),
            },
        }
    }

    fn with_primary_used(mut self, primary_used_percent: f64) -> Self {
        self.record.quota.selection = Some(
            quota_row()
                .with_primary_used_percent(primary_used_percent)
                .build(),
        );
        self
    }

    fn with_secondary_used(mut self, secondary_used_percent: f64) -> Self {
        let row = self
            .record
            .quota
            .selection
            .take()
            .unwrap_or_else(|| quota_row().build());
        self.record.quota.selection = Some(AccountQuotaStateRecord {
            secondary_used_percent: Some(secondary_used_percent),
            secondary_resets_at: Some(now_plus_hours(6)),
            ..row
        });
        self
    }

    fn with_primary_reset(mut self, primary_resets_at: DateTime<Utc>) -> Self {
        let row = self
            .record
            .quota
            .selection
            .take()
            .unwrap_or_else(|| quota_row().build());
        self.record.quota.selection = Some(AccountQuotaStateRecord {
            primary_resets_at: Some(primary_resets_at),
            ..row
        });
        self
    }

    fn with_pool_position(mut self, pool_position: usize) -> Self {
        self.record.pool_position = pool_position;
        self
    }

    fn with_primary_block(mut self, next_probe_after: DateTime<Utc>) -> Self {
        self.record.quota.selection = Some(AccountQuotaStateRecord {
            exhausted_windows: QuotaExhaustedWindows::Primary,
            predicted_blocked_until: Some(now_plus_minutes(10)),
            next_probe_after: Some(next_probe_after),
            primary_used_percent: Some(99.0),
            primary_resets_at: Some(now_plus_hours(1)),
            ..quota_row().build()
        });
        self
    }

    fn with_secondary_block(mut self, next_probe_after: DateTime<Utc>) -> Self {
        self.record.quota.selection = Some(AccountQuotaStateRecord {
            exhausted_windows: QuotaExhaustedWindows::Secondary,
            predicted_blocked_until: Some(now_plus_minutes(10)),
            next_probe_after: Some(next_probe_after),
            secondary_used_percent: Some(99.0),
            secondary_resets_at: Some(now_plus_hours(24)),
            ..quota_row().build()
        });
        self
    }

    fn with_secondary_exhausted(mut self) -> Self {
        self.record.quota.selection = Some(AccountQuotaStateRecord {
            exhausted_windows: QuotaExhaustedWindows::Secondary,
            predicted_blocked_until: Some(now_plus_minutes(30)),
            next_probe_after: Some(now_plus_minutes(15)),
            secondary_used_percent: Some(99.0),
            secondary_resets_at: Some(now_plus_hours(24)),
            ..quota_row().build()
        });
        self
    }

    fn with_missing_secondary_window(mut self) -> Self {
        self.record.quota.selection = Some(AccountQuotaStateRecord {
            primary_used_percent: Some(48.0),
            primary_resets_at: Some(now_plus_minutes(30)),
            secondary_used_percent: None,
            secondary_resets_at: None,
            exhausted_windows: QuotaExhaustedWindows::None,
            ..quota_row().build()
        });
        self
    }

    fn with_family_quota(mut self, family: &str, row: AccountQuotaStateRecord) -> Self {
        if family == "codex" {
            self.record.quota.codex_fallback = Some(AccountQuotaStateRecord {
                limit_id: family.to_string(),
                ..row
            });
        } else {
            self.record.quota.selection = Some(AccountQuotaStateRecord {
                limit_id: family.to_string(),
                ..row
            });
        }
        self
    }

    fn with_exhausted_windows(mut self, exhausted_windows: QuotaExhaustedWindows) -> Self {
        let row = self
            .record
            .quota
            .selection
            .take()
            .unwrap_or_else(|| quota_row().build());
        self.record.quota.selection = Some(AccountQuotaStateRecord {
            exhausted_windows,
            ..row
        });
        self
    }

    fn with_predicted_blocked_until(mut self, predicted_blocked_until: DateTime<Utc>) -> Self {
        let row = self
            .record
            .quota
            .selection
            .take()
            .unwrap_or_else(|| quota_row().build());
        self.record.quota.selection = Some(AccountQuotaStateRecord {
            predicted_blocked_until: Some(predicted_blocked_until),
            ..row
        });
        self
    }
}

impl From<CandidateBuilder> for AccountRecord {
    fn from(value: CandidateBuilder) -> Self {
        value.record
    }
}

#[derive(Default)]
struct QuotaRowBuilder {
    primary_used_percent: Option<f64>,
    primary_resets_at: Option<DateTime<Utc>>,
    secondary_used_percent: Option<f64>,
    secondary_resets_at: Option<DateTime<Utc>>,
    exhausted_windows: Option<QuotaExhaustedWindows>,
    predicted_blocked_until: Option<Option<DateTime<Utc>>>,
    next_probe_after: Option<Option<DateTime<Utc>>>,
}

impl QuotaRowBuilder {
    fn with_primary_used_percent(mut self, primary_used_percent: f64) -> Self {
        self.primary_used_percent = Some(primary_used_percent);
        self.primary_resets_at = Some(now_plus_minutes(20));
        self
    }

    fn with_exhausted_windows(mut self, exhausted_windows: QuotaExhaustedWindows) -> Self {
        self.exhausted_windows = Some(exhausted_windows);
        self
    }

    fn with_predicted_blocked_until(
        mut self,
        predicted_blocked_until: Option<DateTime<Utc>>,
    ) -> Self {
        self.predicted_blocked_until = Some(predicted_blocked_until);
        self
    }

    fn with_next_probe_after(mut self, next_probe_after: Option<DateTime<Utc>>) -> Self {
        self.next_probe_after = Some(next_probe_after);
        self
    }

    fn build(self) -> AccountQuotaStateRecord {
        AccountQuotaStateRecord {
            account_id: "fixture".to_string(),
            limit_id: "codex".to_string(),
            primary_used_percent: self.primary_used_percent,
            primary_resets_at: self.primary_resets_at,
            secondary_used_percent: self.secondary_used_percent,
            secondary_resets_at: self.secondary_resets_at,
            observed_at: now(),
            exhausted_windows: self
                .exhausted_windows
                .unwrap_or(QuotaExhaustedWindows::None),
            predicted_blocked_until: self.predicted_blocked_until.unwrap_or(None),
            next_probe_after: self.next_probe_after.unwrap_or(None),
            probe_backoff_level: 0,
            last_probe_result: None,
        }
    }
}
