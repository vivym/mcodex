use crate::types::AccountRecord;
use codex_state::AccountQuotaStateRecord;

/// Selector intent for quota-aware account choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionIntent {
    Startup,
    SoftRotation,
    HardFailover,
    ProbeRecovery,
}

impl SelectionIntent {
    pub fn allows_just_replaced_reuse(self) -> bool {
        matches!(self, Self::HardFailover)
    }

    pub fn stays_on_current_when_exhausted(self) -> bool {
        matches!(self, Self::SoftRotation)
    }

    pub fn is_probe_recovery(self) -> bool {
        matches!(self, Self::ProbeRecovery)
    }
}

/// Terminal action chosen by the shared selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionAction {
    Select(String),
    Probe(String),
    StayOnCurrent,
    NoCandidate,
}

/// Why a candidate was rejected from ordinary selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionRejectReason {
    Disabled,
    Unhealthy,
    LeasedToOtherHolder,
    CurrentAccount,
    JustReplacedAccount,
    PredictedBlocked,
    ProbeTargetOnly,
}

/// Why the selector chose its terminal action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionDecisionReason {
    #[default]
    OrdinaryRanking,
    HardFailoverOverride,
    ProbeFallback,
    SoftRotationCurrentRetained,
    NoCandidate,
}

/// Family-scoped quota facts for one candidate.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QuotaFamilyView {
    pub selection: Option<AccountQuotaStateRecord>,
    pub codex_fallback: Option<AccountQuotaStateRecord>,
}

impl QuotaFamilyView {
    pub fn effective_quota(&self, selection_family: &str) -> Option<&AccountQuotaStateRecord> {
        if selection_family == "codex" {
            self.selection
                .as_ref()
                .filter(|row| row.limit_id == "codex")
                .or(self.codex_fallback.as_ref())
        } else {
            self.selection
                .as_ref()
                .filter(|row| row.limit_id == selection_family)
                .or(self.codex_fallback.as_ref())
        }
    }
}

/// Soft-block classification for a hard-filtered candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaBlockClass {
    NotBlocked { low_confidence: bool },
    PredictedBlocked,
    ProbeEligibleBlocked,
}

/// Result of a quota-refresh probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeOutcome {
    Success,
    StillBlocked,
    Ambiguous,
}

/// Rejected candidate annotation surfaced by the selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedCandidate {
    pub account_id: String,
    pub reason: SelectionRejectReason,
}

/// Full shared selector output.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionPlan {
    pub eligible_candidates: Vec<AccountRecord>,
    pub probe_candidate: Option<String>,
    pub rejected_candidates: Vec<RejectedCandidate>,
    pub decision_reason: SelectionDecisionReason,
    pub terminal_action: SelectionAction,
}
