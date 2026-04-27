use codex_app_server_protocol::AccountStartupAvailability as ProtocolStartupAvailability;
use codex_app_server_protocol::AccountStartupCandidatePool as ProtocolStartupCandidatePool;
use codex_app_server_protocol::AccountStartupResolutionIssue as ProtocolStartupResolutionIssue;
use codex_app_server_protocol::AccountStartupResolutionIssueSource as ProtocolStartupResolutionIssueSource;
use codex_app_server_protocol::AccountStartupResolutionIssueType as ProtocolStartupResolutionIssueType;
use codex_app_server_protocol::AccountStartupSnapshot;
use codex_state::AccountStartupAvailability;
use codex_state::AccountStartupCandidatePool;
use codex_state::AccountStartupEligibility;
use codex_state::AccountStartupResolutionIssue;
use codex_state::AccountStartupResolutionIssueKind;
use codex_state::AccountStartupResolutionIssueSource;
use codex_state::AccountStartupStatus;
use codex_state::EffectivePoolResolutionSource;

pub(crate) fn snapshot_from_startup_status(
    startup: &AccountStartupStatus,
) -> AccountStartupSnapshot {
    AccountStartupSnapshot {
        effective_pool_id: startup.preview.effective_pool_id.clone(),
        effective_pool_resolution_source: effective_pool_resolution_source_to_wire_string(
            startup.effective_pool_resolution_source,
        )
        .to_string(),
        configured_default_pool_id: startup.configured_default_pool_id.clone(),
        persisted_default_pool_id: startup.persisted_default_pool_id.clone(),
        startup_availability: startup_availability_to_protocol(startup.startup_availability),
        startup_resolution_issue: startup
            .startup_resolution_issue
            .as_ref()
            .map(startup_resolution_issue_to_protocol),
        selection_eligibility: selection_eligibility_to_wire_string(
            startup.startup_availability,
            &startup.preview.eligibility,
        )
        .to_string(),
    }
}

pub(crate) fn unavailable_snapshot() -> AccountStartupSnapshot {
    AccountStartupSnapshot {
        effective_pool_id: None,
        effective_pool_resolution_source: "none".to_string(),
        configured_default_pool_id: None,
        persisted_default_pool_id: None,
        startup_availability: ProtocolStartupAvailability::Unavailable,
        startup_resolution_issue: None,
        selection_eligibility: "missingPool".to_string(),
    }
}

pub(crate) fn effective_pool_resolution_source_to_wire_string(
    source: EffectivePoolResolutionSource,
) -> &'static str {
    match source {
        EffectivePoolResolutionSource::Override => "override",
        EffectivePoolResolutionSource::ConfigDefault => "configDefault",
        EffectivePoolResolutionSource::PersistedSelection => "persistedSelection",
        EffectivePoolResolutionSource::SingleVisiblePool => "singleVisiblePool",
        EffectivePoolResolutionSource::None => "none",
    }
}

pub(crate) fn selection_eligibility_to_wire_string(
    startup_availability: AccountStartupAvailability,
    eligibility: &AccountStartupEligibility,
) -> &'static str {
    match startup_availability {
        AccountStartupAvailability::MultiplePoolsRequireDefault
        | AccountStartupAvailability::InvalidExplicitDefault
        | AccountStartupAvailability::Unavailable => "missingPool",
        AccountStartupAvailability::Available | AccountStartupAvailability::Suppressed => {
            match eligibility {
                AccountStartupEligibility::Suppressed => "durablySuppressed",
                AccountStartupEligibility::MissingPool => "missingPool",
                AccountStartupEligibility::PreferredAccountSelected => "preferredAccountSelected",
                AccountStartupEligibility::AutomaticAccountSelected => "automaticAccountSelected",
                AccountStartupEligibility::PreferredAccountMissing => "preferredAccountMissing",
                AccountStartupEligibility::PreferredAccountInOtherPool { .. } => {
                    "preferredAccountInOtherPool"
                }
                AccountStartupEligibility::PreferredAccountDisabled => "preferredAccountDisabled",
                AccountStartupEligibility::PreferredAccountUnhealthy => "preferredAccountUnhealthy",
                AccountStartupEligibility::PreferredAccountBusy => "preferredAccountBusy",
                AccountStartupEligibility::NoEligibleAccount => "noEligibleAccount",
            }
        }
    }
}

fn startup_availability_to_protocol(
    availability: AccountStartupAvailability,
) -> ProtocolStartupAvailability {
    match availability {
        AccountStartupAvailability::Available => ProtocolStartupAvailability::Available,
        AccountStartupAvailability::Suppressed => ProtocolStartupAvailability::Suppressed,
        AccountStartupAvailability::MultiplePoolsRequireDefault => {
            ProtocolStartupAvailability::MultiplePoolsRequireDefault
        }
        AccountStartupAvailability::InvalidExplicitDefault => {
            ProtocolStartupAvailability::InvalidExplicitDefault
        }
        AccountStartupAvailability::Unavailable => ProtocolStartupAvailability::Unavailable,
    }
}

fn startup_resolution_issue_to_protocol(
    issue: &AccountStartupResolutionIssue,
) -> ProtocolStartupResolutionIssue {
    ProtocolStartupResolutionIssue {
        r#type: startup_resolution_issue_kind_to_protocol(issue.kind),
        source: startup_resolution_issue_source_to_protocol(issue.source),
        pool_id: issue.pool_id.clone(),
        candidate_pool_count: issue
            .candidate_pool_count
            .and_then(|count| u32::try_from(count).ok()),
        candidate_pools: issue.candidate_pools.as_ref().map(|candidate_pools| {
            candidate_pools
                .iter()
                .map(candidate_pool_to_protocol)
                .collect()
        }),
        message: issue.message.clone(),
    }
}

fn startup_resolution_issue_kind_to_protocol(
    kind: AccountStartupResolutionIssueKind,
) -> ProtocolStartupResolutionIssueType {
    match kind {
        AccountStartupResolutionIssueKind::MultiplePoolsRequireDefault => {
            ProtocolStartupResolutionIssueType::MultiplePoolsRequireDefault
        }
        AccountStartupResolutionIssueKind::OverridePoolUnavailable => {
            ProtocolStartupResolutionIssueType::OverridePoolUnavailable
        }
        AccountStartupResolutionIssueKind::ConfigDefaultPoolUnavailable => {
            ProtocolStartupResolutionIssueType::ConfigDefaultPoolUnavailable
        }
        AccountStartupResolutionIssueKind::PersistedDefaultPoolUnavailable => {
            ProtocolStartupResolutionIssueType::PersistedDefaultPoolUnavailable
        }
    }
}

fn startup_resolution_issue_source_to_protocol(
    source: AccountStartupResolutionIssueSource,
) -> ProtocolStartupResolutionIssueSource {
    match source {
        AccountStartupResolutionIssueSource::Override => {
            ProtocolStartupResolutionIssueSource::Override
        }
        AccountStartupResolutionIssueSource::ConfigDefault => {
            ProtocolStartupResolutionIssueSource::ConfigDefault
        }
        AccountStartupResolutionIssueSource::PersistedSelection => {
            ProtocolStartupResolutionIssueSource::PersistedSelection
        }
        AccountStartupResolutionIssueSource::None => ProtocolStartupResolutionIssueSource::None,
    }
}

fn candidate_pool_to_protocol(
    candidate_pool: &AccountStartupCandidatePool,
) -> ProtocolStartupCandidatePool {
    ProtocolStartupCandidatePool {
        pool_id: candidate_pool.pool_id.clone(),
        display_name: candidate_pool.display_name.clone(),
        status: candidate_pool.status.clone(),
    }
}
