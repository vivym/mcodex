use crate::quota::QuotaBlockClass;
use crate::quota::RejectedCandidate;
use crate::quota::SelectionAction;
use crate::quota::SelectionDecisionReason;
use crate::quota::SelectionPlan;
use crate::quota::SelectionRejectReason;
use crate::types::AccountRecord;
use crate::types::SelectionRequest;
use chrono::DateTime;
use chrono::Utc;
use codex_state::AccountQuotaStateRecord;
use codex_state::QuotaExhaustedWindows;
use std::cmp::Ordering;

pub fn build_selection_plan(request: SelectionRequest) -> SelectionPlanBuilder {
    SelectionPlanBuilder {
        request,
        candidates: Vec::new(),
    }
}

#[derive(Debug, Clone)]
pub struct SelectionPlanBuilder {
    request: SelectionRequest,
    candidates: Vec<AccountRecord>,
}

impl SelectionPlanBuilder {
    pub fn with_candidate(mut self, candidate: impl Into<AccountRecord>) -> Self {
        self.candidates.push(candidate.into());
        self
    }

    pub fn run(self) -> SelectionPlan {
        evaluate_selection(self.request, self.candidates)
    }
}

#[derive(Clone)]
struct RankedCandidate {
    record: AccountRecord,
    block_class: QuotaBlockClass,
}

pub fn evaluate_selection(
    request: SelectionRequest,
    candidates: Vec<AccountRecord>,
) -> SelectionPlan {
    let selection_family = request.selection_family();
    let now = request.now.unwrap_or_else(Utc::now);
    let mut rejected_candidates = Vec::new();
    let mut admitted = Vec::new();

    for mut candidate in candidates {
        if candidate.pool_position == 0 {
            candidate.pool_position = admitted.len() + rejected_candidates.len();
        }

        if let Some(reason) = hard_reject_reason(&request, &candidate) {
            rejected_candidates.push(RejectedCandidate {
                account_id: candidate.account_id,
                reason,
            });
            continue;
        }

        let block_class = classify_candidate(&candidate, selection_family, now);
        if matches!(
            block_class,
            QuotaBlockClass::PredictedBlocked | QuotaBlockClass::ProbeEligibleBlocked
        ) {
            rejected_candidates.push(RejectedCandidate {
                account_id: candidate.account_id.clone(),
                reason: SelectionRejectReason::PredictedBlocked,
            });
        }
        admitted.push(RankedCandidate {
            record: candidate,
            block_class,
        });
    }

    if request.intent.is_probe_recovery() {
        return finalize_probe_recovery(admitted, rejected_candidates);
    }

    let mut eligible_candidates = admitted
        .iter()
        .filter(|candidate| matches!(candidate.block_class, QuotaBlockClass::NotBlocked { .. }))
        .map(|candidate| candidate.record.clone())
        .collect::<Vec<_>>();
    eligible_candidates.sort_by(|left, right| compare_ordinary(left, right, &request, now));

    if let Some(selected) = eligible_candidates.first() {
        let decision_reason = if request.intent.allows_just_replaced_reuse()
            && request.just_replaced_account_id.as_deref() == Some(selected.account_id.as_str())
        {
            SelectionDecisionReason::HardFailoverOverride
        } else {
            SelectionDecisionReason::OrdinaryRanking
        };
        return SelectionPlan {
            terminal_action: SelectionAction::Select(selected.account_id.clone()),
            eligible_candidates,
            probe_candidate: None,
            rejected_candidates,
            decision_reason,
        };
    }

    let probe_candidate = admitted
        .iter()
        .filter(|candidate| matches!(candidate.block_class, QuotaBlockClass::ProbeEligibleBlocked))
        .map(|candidate| &candidate.record)
        .max_by(|left, right| compare_probe_candidates(left, right, selection_family, now))
        .map(|candidate| candidate.account_id.clone());

    let (terminal_action, decision_reason) = match probe_candidate.as_deref() {
        Some(account_id) => (
            SelectionAction::Probe(account_id.to_string()),
            SelectionDecisionReason::ProbeFallback,
        ),
        None if request.intent.stays_on_current_when_exhausted() => (
            SelectionAction::StayOnCurrent,
            SelectionDecisionReason::SoftRotationCurrentRetained,
        ),
        None => (
            SelectionAction::NoCandidate,
            SelectionDecisionReason::NoCandidate,
        ),
    };

    SelectionPlan {
        eligible_candidates,
        probe_candidate,
        rejected_candidates,
        decision_reason,
        terminal_action,
    }
}

fn finalize_probe_recovery(
    admitted: Vec<RankedCandidate>,
    mut rejected_candidates: Vec<RejectedCandidate>,
) -> SelectionPlan {
    let probe_candidate = admitted
        .iter()
        .find(|candidate| matches!(candidate.block_class, QuotaBlockClass::ProbeEligibleBlocked))
        .map(|candidate| candidate.record.account_id.clone());

    if probe_candidate.is_none() {
        for candidate in admitted.iter().filter(|candidate| {
            !matches!(candidate.block_class, QuotaBlockClass::ProbeEligibleBlocked)
        }) {
            if rejected_candidates
                .iter()
                .any(|rejected| rejected.account_id == candidate.record.account_id)
            {
                continue;
            }
            let reason = if matches!(candidate.block_class, QuotaBlockClass::PredictedBlocked) {
                SelectionRejectReason::PredictedBlocked
            } else {
                SelectionRejectReason::ProbeTargetOnly
            };
            rejected_candidates.push(RejectedCandidate {
                account_id: candidate.record.account_id.clone(),
                reason,
            });
        }
    }

    let terminal_action = probe_candidate
        .as_ref()
        .map(|account_id| SelectionAction::Probe(account_id.clone()))
        .unwrap_or(SelectionAction::NoCandidate);

    SelectionPlan {
        eligible_candidates: Vec::new(),
        probe_candidate,
        rejected_candidates,
        decision_reason: if matches!(terminal_action, SelectionAction::Probe(_)) {
            SelectionDecisionReason::ProbeFallback
        } else {
            SelectionDecisionReason::NoCandidate
        },
        terminal_action,
    }
}

fn hard_reject_reason(
    request: &SelectionRequest,
    candidate: &AccountRecord,
) -> Option<SelectionRejectReason> {
    if !candidate.enabled {
        return Some(SelectionRejectReason::Disabled);
    }
    if !candidate.healthy || !candidate.selector_auth_eligible {
        return Some(SelectionRejectReason::Unhealthy);
    }
    if candidate.leased_to_other_holder {
        return Some(SelectionRejectReason::LeasedToOtherHolder);
    }
    if request.intent.is_probe_recovery() {
        if request.reserved_probe_target_account_id.as_deref()
            != Some(candidate.account_id.as_str())
        {
            return Some(SelectionRejectReason::ProbeTargetOnly);
        }
        return None;
    }
    if request.current_account_id.as_deref() == Some(candidate.account_id.as_str()) {
        return Some(SelectionRejectReason::CurrentAccount);
    }
    if !request.intent.allows_just_replaced_reuse()
        && request.just_replaced_account_id.as_deref() == Some(candidate.account_id.as_str())
    {
        return Some(SelectionRejectReason::JustReplacedAccount);
    }
    None
}

fn classify_candidate(
    candidate: &AccountRecord,
    selection_family: &str,
    now: DateTime<Utc>,
) -> QuotaBlockClass {
    let Some(row) = candidate.quota.effective_quota(selection_family) else {
        return QuotaBlockClass::NotBlocked {
            low_confidence: true,
        };
    };

    if row.exhausted_windows.is_exhausted() {
        return if row
            .next_probe_after
            .is_none_or(|next_probe_after| now >= next_probe_after)
        {
            QuotaBlockClass::ProbeEligibleBlocked
        } else {
            QuotaBlockClass::PredictedBlocked
        };
    }

    QuotaBlockClass::NotBlocked {
        low_confidence: is_low_confidence(row),
    }
}

fn compare_ordinary(
    left: &AccountRecord,
    right: &AccountRecord,
    request: &SelectionRequest,
    now: DateTime<Utc>,
) -> Ordering {
    let selection_family = request.selection_family();
    let threshold = f64::from(request.proactive_threshold_percent);
    let left_row = left.quota.effective_quota(selection_family);
    let right_row = right.quota.effective_quota(selection_family);
    let left_below_threshold = primary_used_percent(left_row) < threshold;
    let right_below_threshold = primary_used_percent(right_row) < threshold;

    left_below_threshold
        .cmp(&right_below_threshold)
        .reverse()
        .then_with(|| {
            compare_f64_desc(
                primary_safety_margin(left_row),
                primary_safety_margin(right_row),
            )
        })
        .then_with(|| {
            compare_f64_desc(
                secondary_safety_margin(left_row),
                secondary_safety_margin(right_row),
            )
        })
        .then_with(|| {
            compare_reset_ascending(primary_reset(left_row), primary_reset(right_row), now)
        })
        .then_with(|| low_confidence(left_row).cmp(&low_confidence(right_row)))
        .then_with(|| left.pool_position.cmp(&right.pool_position))
        .then_with(|| left.account_id.cmp(&right.account_id))
}

fn compare_probe_candidates(
    left: &AccountRecord,
    right: &AccountRecord,
    selection_family: &str,
    now: DateTime<Utc>,
) -> Ordering {
    let Some(left_row) = left.quota.effective_quota(selection_family) else {
        return Ordering::Equal;
    };
    let Some(right_row) = right.quota.effective_quota(selection_family) else {
        return Ordering::Equal;
    };

    probe_priority(left_row, now)
        .cmp(&probe_priority(right_row, now))
        .then_with(|| left.pool_position.cmp(&right.pool_position).reverse())
        .then_with(|| right.account_id.cmp(&left.account_id))
}

fn probe_priority(row: &AccountQuotaStateRecord, now: DateTime<Utc>) -> (i8, i64, i64) {
    let kind_rank = match row.exhausted_windows {
        QuotaExhaustedWindows::Primary => 3,
        QuotaExhaustedWindows::Both => 2,
        QuotaExhaustedWindows::Secondary => 1,
        QuotaExhaustedWindows::Unknown | QuotaExhaustedWindows::None => 0,
    };
    let probe_staleness_secs = row
        .next_probe_after
        .map(|next_probe_after| (now - next_probe_after).num_seconds().max(0))
        .unwrap_or(i64::MAX);
    let predicted_recovery_secs = row
        .predicted_blocked_until
        .map(|predicted_blocked_until| {
            (predicted_blocked_until - now)
                .num_seconds()
                .saturating_neg()
        })
        .unwrap_or(i64::MIN);

    (kind_rank, probe_staleness_secs, predicted_recovery_secs)
}

fn is_low_confidence(row: &AccountQuotaStateRecord) -> bool {
    row.primary_used_percent.is_none()
        || row.primary_resets_at.is_none()
        || row.secondary_used_percent.is_none()
        || row.secondary_resets_at.is_none()
}

fn low_confidence(row: Option<&AccountQuotaStateRecord>) -> bool {
    row.is_none_or(is_low_confidence)
}

fn primary_used_percent(row: Option<&AccountQuotaStateRecord>) -> f64 {
    row.and_then(|row| row.primary_used_percent)
        .unwrap_or(100.0)
}

fn primary_safety_margin(row: Option<&AccountQuotaStateRecord>) -> f64 {
    100.0 - primary_used_percent(row)
}

fn secondary_safety_margin(row: Option<&AccountQuotaStateRecord>) -> f64 {
    100.0
        - row
            .and_then(|row| row.secondary_used_percent)
            .unwrap_or(100.0)
}

fn primary_reset(row: Option<&AccountQuotaStateRecord>) -> Option<DateTime<Utc>> {
    row.and_then(|row| row.primary_resets_at)
}

fn compare_f64_desc(left: f64, right: f64) -> Ordering {
    right.partial_cmp(&left).unwrap_or(Ordering::Equal)
}

fn compare_reset_ascending(
    left: Option<DateTime<Utc>>,
    right: Option<DateTime<Utc>>,
    _now: DateTime<Utc>,
) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}
