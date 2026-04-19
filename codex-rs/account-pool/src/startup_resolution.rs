use anyhow::Context;
use codex_state::AccountStartupAvailability;
use codex_state::AccountStartupCandidatePool;
use codex_state::AccountStartupEligibility;
use codex_state::AccountStartupResolutionIssue;
use codex_state::AccountStartupResolutionIssueKind;
use codex_state::AccountStartupResolutionIssueSource;
use codex_state::AccountStartupSelectionPreview;
use codex_state::AccountStartupStatus;
use codex_state::EffectivePoolResolutionSource;
use std::future::Future;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupResolutionInput {
    pub override_pool_id: Option<String>,
    pub configured_default_pool_id: Option<String>,
    pub persisted_default_pool_id: Option<String>,
    pub persisted_preferred_account_id: Option<String>,
    pub suppressed: bool,
    pub inventory: StartupPoolInventory,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StartupPoolInventory {
    pub candidates: Vec<StartupPoolCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupPoolCandidate {
    pub pool_id: String,
    pub display_name: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupSelectionFacts {
    pub preferred_account_outcome: Option<StartupPreferredAccountOutcome>,
    pub predicted_account_id: Option<String>,
    pub any_eligible_account: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupPreferredAccountOutcome {
    Selected,
    Missing,
    InOtherPool { actual_pool_id: String },
    Disabled,
    Unhealthy,
    Busy,
}

pub async fn resolve_startup_status<F, Fut>(
    input: StartupResolutionInput,
    selection_facts_for_pool: F,
) -> anyhow::Result<AccountStartupStatus>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = anyhow::Result<StartupSelectionFacts>>,
{
    let mut candidate_pools = input
        .inventory
        .candidates
        .into_iter()
        .map(|candidate| AccountStartupCandidatePool {
            pool_id: candidate.pool_id,
            display_name: candidate.display_name,
            status: candidate.status,
        })
        .collect::<Vec<_>>();
    candidate_pools.sort_by(|left, right| left.pool_id.cmp(&right.pool_id));

    let candidate_pool_ids = candidate_pools
        .iter()
        .map(|pool| pool.pool_id.as_str())
        .collect::<Vec<_>>();

    let (
        effective_pool_id,
        effective_pool_resolution_source,
        startup_availability,
        startup_resolution_issue,
    ) = if let Some(pool_id) = input.override_pool_id.as_deref() {
        if candidate_pool_ids.contains(&pool_id) {
            (
                Some(pool_id.to_string()),
                EffectivePoolResolutionSource::Override,
                None,
                None,
            )
        } else {
            (
                None,
                EffectivePoolResolutionSource::Override,
                Some(AccountStartupAvailability::InvalidExplicitDefault),
                Some(AccountStartupResolutionIssue {
                    kind: AccountStartupResolutionIssueKind::OverridePoolUnavailable,
                    source: AccountStartupResolutionIssueSource::Override,
                    pool_id: Some(pool_id.to_string()),
                    candidate_pool_count: Some(candidate_pools.len()),
                    candidate_pools: Some(candidate_pools.clone()),
                    message: None,
                }),
            )
        }
    } else if let Some(pool_id) = input.configured_default_pool_id.as_deref() {
        if candidate_pool_ids.contains(&pool_id) {
            (
                Some(pool_id.to_string()),
                EffectivePoolResolutionSource::ConfigDefault,
                None,
                None,
            )
        } else {
            (
                None,
                EffectivePoolResolutionSource::ConfigDefault,
                Some(AccountStartupAvailability::InvalidExplicitDefault),
                Some(AccountStartupResolutionIssue {
                    kind: AccountStartupResolutionIssueKind::ConfigDefaultPoolUnavailable,
                    source: AccountStartupResolutionIssueSource::ConfigDefault,
                    pool_id: Some(pool_id.to_string()),
                    candidate_pool_count: Some(candidate_pools.len()),
                    candidate_pools: Some(candidate_pools.clone()),
                    message: None,
                }),
            )
        }
    } else if let Some(pool_id) = input.persisted_default_pool_id.as_deref() {
        if candidate_pool_ids.contains(&pool_id) {
            (
                Some(pool_id.to_string()),
                EffectivePoolResolutionSource::PersistedSelection,
                None,
                None,
            )
        } else {
            (
                None,
                EffectivePoolResolutionSource::PersistedSelection,
                Some(AccountStartupAvailability::InvalidExplicitDefault),
                Some(AccountStartupResolutionIssue {
                    kind: AccountStartupResolutionIssueKind::PersistedDefaultPoolUnavailable,
                    source: AccountStartupResolutionIssueSource::PersistedSelection,
                    pool_id: Some(pool_id.to_string()),
                    candidate_pool_count: Some(candidate_pools.len()),
                    candidate_pools: Some(candidate_pools.clone()),
                    message: None,
                }),
            )
        }
    } else if candidate_pools.len() == 1 {
        (
            Some(candidate_pools[0].pool_id.clone()),
            EffectivePoolResolutionSource::SingleVisiblePool,
            None,
            None,
        )
    } else if candidate_pools.len() > 1 {
        (
            None,
            EffectivePoolResolutionSource::None,
            Some(AccountStartupAvailability::MultiplePoolsRequireDefault),
            Some(AccountStartupResolutionIssue {
                kind: AccountStartupResolutionIssueKind::MultiplePoolsRequireDefault,
                source: AccountStartupResolutionIssueSource::None,
                pool_id: None,
                candidate_pool_count: Some(candidate_pools.len()),
                candidate_pools: Some(candidate_pools.clone()),
                message: None,
            }),
        )
    } else {
        (
            None,
            EffectivePoolResolutionSource::None,
            Some(AccountStartupAvailability::Unavailable),
            None,
        )
    };

    let preview = if let Some(pool_id) = effective_pool_id.clone() {
        let facts = selection_facts_for_pool(pool_id.clone()).await?;
        let (eligibility, predicted_account_id) =
            if let Some(preferred_account_id) = input.persisted_preferred_account_id.clone() {
                let preferred_account_outcome = facts
                    .preferred_account_outcome
                    .context("preferred account outcome missing for persisted preferred account")?;
                let eligibility = match preferred_account_outcome {
                    StartupPreferredAccountOutcome::Selected => {
                        AccountStartupEligibility::PreferredAccountSelected
                    }
                    StartupPreferredAccountOutcome::Missing => {
                        AccountStartupEligibility::PreferredAccountMissing
                    }
                    StartupPreferredAccountOutcome::InOtherPool { actual_pool_id } => {
                        AccountStartupEligibility::PreferredAccountInOtherPool { actual_pool_id }
                    }
                    StartupPreferredAccountOutcome::Disabled => {
                        AccountStartupEligibility::PreferredAccountDisabled
                    }
                    StartupPreferredAccountOutcome::Unhealthy => {
                        AccountStartupEligibility::PreferredAccountUnhealthy
                    }
                    StartupPreferredAccountOutcome::Busy => {
                        AccountStartupEligibility::PreferredAccountBusy
                    }
                };
                let predicted_account_id = matches!(
                    eligibility,
                    AccountStartupEligibility::PreferredAccountSelected
                )
                .then_some(preferred_account_id);
                (eligibility, predicted_account_id)
            } else {
                let predicted_account_id = facts
                    .any_eligible_account
                    .then_some(facts.predicted_account_id)
                    .flatten();
                let eligibility = if predicted_account_id.is_some() {
                    AccountStartupEligibility::AutomaticAccountSelected
                } else {
                    AccountStartupEligibility::NoEligibleAccount
                };
                (eligibility, predicted_account_id)
            };

        AccountStartupSelectionPreview {
            effective_pool_id: Some(pool_id),
            preferred_account_id: input.persisted_preferred_account_id,
            suppressed: input.suppressed,
            predicted_account_id,
            eligibility,
        }
    } else {
        AccountStartupSelectionPreview {
            effective_pool_id: None,
            preferred_account_id: input.persisted_preferred_account_id,
            suppressed: input.suppressed,
            predicted_account_id: None,
            eligibility: AccountStartupEligibility::MissingPool,
        }
    };
    let startup_availability = match startup_availability {
        Some(startup_availability) => startup_availability,
        None if input.suppressed => AccountStartupAvailability::Suppressed,
        None => AccountStartupAvailability::Available,
    };

    Ok(AccountStartupStatus {
        preview,
        configured_default_pool_id: input.configured_default_pool_id,
        persisted_default_pool_id: input.persisted_default_pool_id,
        effective_pool_resolution_source,
        startup_availability,
        startup_resolution_issue,
        candidate_pools,
    })
}

#[cfg(test)]
mod tests {
    use super::StartupPoolCandidate;
    use super::StartupPoolInventory;
    use super::StartupPreferredAccountOutcome;
    use super::StartupResolutionInput;
    use super::StartupSelectionFacts;
    use super::resolve_startup_status;
    use codex_state::AccountStartupAvailability;
    use codex_state::AccountStartupEligibility;
    use codex_state::AccountStartupResolutionIssueKind;
    use codex_state::EffectivePoolResolutionSource;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn resolve_startup_status_uses_single_visible_pool_when_no_default_exists() {
        let status = resolve_startup_status(
            StartupResolutionInput {
                override_pool_id: None,
                configured_default_pool_id: None,
                persisted_default_pool_id: None,
                persisted_preferred_account_id: None,
                suppressed: false,
                inventory: StartupPoolInventory {
                    candidates: vec![StartupPoolCandidate {
                        pool_id: "pool-main".to_string(),
                        display_name: None,
                        status: None,
                    }],
                },
            },
            |_pool_id| async {
                Ok(StartupSelectionFacts {
                    preferred_account_outcome: None,
                    predicted_account_id: Some("acct-1".to_string()),
                    any_eligible_account: true,
                })
            },
        )
        .await
        .expect("resolve startup status");

        assert_eq!(
            status.effective_pool_resolution_source,
            EffectivePoolResolutionSource::SingleVisiblePool
        );
        assert_eq!(
            status.startup_availability,
            AccountStartupAvailability::Available
        );
        assert_eq!(
            status.preview.eligibility,
            AccountStartupEligibility::AutomaticAccountSelected
        );
    }

    #[tokio::test]
    async fn resolve_startup_status_requires_default_when_multiple_pools_are_visible() {
        let status = resolve_startup_status(
            StartupResolutionInput {
                override_pool_id: None,
                configured_default_pool_id: None,
                persisted_default_pool_id: None,
                persisted_preferred_account_id: None,
                suppressed: false,
                inventory: StartupPoolInventory {
                    candidates: vec![
                        StartupPoolCandidate {
                            pool_id: "pool-other".to_string(),
                            display_name: None,
                            status: None,
                        },
                        StartupPoolCandidate {
                            pool_id: "pool-main".to_string(),
                            display_name: None,
                            status: None,
                        },
                    ],
                },
            },
            |_pool_id| async {
                Ok(StartupSelectionFacts {
                    preferred_account_outcome: None,
                    predicted_account_id: Some("acct-1".to_string()),
                    any_eligible_account: true,
                })
            },
        )
        .await
        .expect("resolve startup status");

        assert_eq!(
            status.startup_availability,
            AccountStartupAvailability::MultiplePoolsRequireDefault
        );
        assert_eq!(
            status
                .startup_resolution_issue
                .as_ref()
                .map(|issue| issue.kind),
            Some(AccountStartupResolutionIssueKind::MultiplePoolsRequireDefault)
        );
        let pool_ids = status
            .candidate_pools
            .iter()
            .map(|pool| pool.pool_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(pool_ids, vec!["pool-main", "pool-other"]);
    }

    #[tokio::test]
    async fn resolve_startup_status_does_not_fall_back_from_invalid_config_default() {
        let status = resolve_startup_status(
            StartupResolutionInput {
                override_pool_id: None,
                configured_default_pool_id: Some("missing-pool".to_string()),
                persisted_default_pool_id: None,
                persisted_preferred_account_id: None,
                suppressed: false,
                inventory: StartupPoolInventory {
                    candidates: vec![StartupPoolCandidate {
                        pool_id: "pool-main".to_string(),
                        display_name: None,
                        status: None,
                    }],
                },
            },
            |_pool_id| async {
                Ok(StartupSelectionFacts {
                    preferred_account_outcome: None,
                    predicted_account_id: Some("acct-1".to_string()),
                    any_eligible_account: true,
                })
            },
        )
        .await
        .expect("resolve startup status");

        assert_eq!(
            status.startup_availability,
            AccountStartupAvailability::InvalidExplicitDefault
        );
        assert_eq!(status.preview.effective_pool_id, None);
        assert_eq!(
            status
                .startup_resolution_issue
                .as_ref()
                .map(|issue| issue.kind),
            Some(AccountStartupResolutionIssueKind::ConfigDefaultPoolUnavailable)
        );
    }

    #[tokio::test]
    async fn resolve_startup_status_preserves_underlying_eligibility_when_suppressed() {
        let status = resolve_startup_status(
            StartupResolutionInput {
                override_pool_id: None,
                configured_default_pool_id: None,
                persisted_default_pool_id: None,
                persisted_preferred_account_id: Some("acct-1".to_string()),
                suppressed: true,
                inventory: StartupPoolInventory {
                    candidates: vec![StartupPoolCandidate {
                        pool_id: "pool-main".to_string(),
                        display_name: None,
                        status: None,
                    }],
                },
            },
            |_pool_id| async {
                Ok(StartupSelectionFacts {
                    preferred_account_outcome: Some(StartupPreferredAccountOutcome::Selected),
                    predicted_account_id: Some("acct-1".to_string()),
                    any_eligible_account: true,
                })
            },
        )
        .await
        .expect("resolve startup status");

        assert_eq!(
            status.startup_availability,
            AccountStartupAvailability::Suppressed
        );
        assert_eq!(
            status.preview.eligibility,
            AccountStartupEligibility::PreferredAccountSelected
        );
        assert_eq!(
            status.preview.predicted_account_id.as_deref(),
            Some("acct-1")
        );
    }
}
