use crate::AccountPoolExecutionBackend;
use crate::startup_resolution::StartupResolutionInput;
use crate::startup_resolution::resolve_startup_status;
use codex_state::AccountStartupAvailability;
use codex_state::AccountStartupStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedStartupStatus {
    pub startup: AccountStartupStatus,
    pub pooled_applicable: bool,
}

pub async fn read_shared_startup_status<B: AccountPoolExecutionBackend>(
    backend: &B,
    configured_default_pool_id: Option<&str>,
    explicit_override_pool_id: Option<&str>,
) -> anyhow::Result<SharedStartupStatus> {
    let selection = backend.read_startup_selection().await?;
    let inventory = backend.read_startup_pool_inventory().await?;
    let startup = resolve_startup_status(
        StartupResolutionInput {
            override_pool_id: explicit_override_pool_id.map(ToOwned::to_owned),
            configured_default_pool_id: configured_default_pool_id.map(ToOwned::to_owned),
            persisted_default_pool_id: selection.default_pool_id,
            persisted_preferred_account_id: selection.preferred_account_id,
            suppressed: selection.suppressed,
            inventory,
        },
        |pool_id| async move { backend.read_startup_selection_facts(&pool_id).await },
    )
    .await?;

    Ok(SharedStartupStatus {
        pooled_applicable: startup.startup_availability != AccountStartupAvailability::Unavailable,
        startup,
    })
}

#[cfg(test)]
mod tests {
    use super::SharedStartupStatus;
    use super::read_shared_startup_status;
    use crate::AccountPoolExecutionBackend;
    use crate::StartupPoolCandidate;
    use crate::StartupPoolInventory;
    use crate::StartupPreferredAccountOutcome;
    use crate::StartupSelectionFacts;
    use async_trait::async_trait;
    use codex_state::AccountHealthEvent;
    use codex_state::AccountLeaseError;
    use codex_state::AccountStartupAvailability;
    use codex_state::AccountStartupResolutionIssueKind;
    use codex_state::AccountStartupSelectionState;
    use codex_state::LeaseKey;
    use codex_state::LeaseRenewal;
    use pretty_assertions::assert_eq;

    struct FakeStartupBackend {
        selection: AccountStartupSelectionState,
        inventory: StartupPoolInventory,
        facts: StartupSelectionFacts,
    }

    impl FakeStartupBackend {
        fn with_visible_pools(pool_ids: [&str; 1]) -> Self {
            Self {
                selection: AccountStartupSelectionState::default(),
                inventory: StartupPoolInventory {
                    candidates: pool_ids
                        .into_iter()
                        .map(|pool_id| StartupPoolCandidate {
                            pool_id: pool_id.to_string(),
                            display_name: None,
                            status: None,
                        })
                        .collect(),
                },
                facts: StartupSelectionFacts {
                    preferred_account_outcome: None,
                    predicted_account_id: Some("acct-1".to_string()),
                    any_eligible_account: true,
                },
            }
        }
    }

    #[async_trait]
    impl AccountPoolExecutionBackend for FakeStartupBackend {
        async fn plan_runtime_selection(
            &self,
            _request: &crate::types::SelectionRequest,
            _holder_instance_id: &str,
        ) -> anyhow::Result<(String, crate::SelectionPlan)> {
            unimplemented!("not used in tests")
        }

        async fn acquire_lease(
            &self,
            _pool_id: &str,
            _holder_instance_id: &str,
        ) -> std::result::Result<crate::LeaseGrant, AccountLeaseError> {
            unimplemented!("not used in tests")
        }

        async fn renew_lease(
            &self,
            _lease: &LeaseKey,
            _now: chrono::DateTime<chrono::Utc>,
        ) -> anyhow::Result<LeaseRenewal> {
            unimplemented!("not used in tests")
        }

        async fn release_lease(
            &self,
            _lease: &LeaseKey,
            _now: chrono::DateTime<chrono::Utc>,
        ) -> anyhow::Result<bool> {
            unimplemented!("not used in tests")
        }

        async fn record_health_event(&self, _event: AccountHealthEvent) -> anyhow::Result<()> {
            unimplemented!("not used in tests")
        }

        async fn read_account_health_event_sequence(
            &self,
            _account_id: &str,
        ) -> anyhow::Result<Option<i64>> {
            unimplemented!("not used in tests")
        }

        async fn read_startup_selection(&self) -> anyhow::Result<AccountStartupSelectionState> {
            Ok(self.selection.clone())
        }

        async fn read_startup_pool_inventory(&self) -> anyhow::Result<StartupPoolInventory> {
            Ok(self.inventory.clone())
        }

        async fn read_startup_selection_facts(
            &self,
            _pool_id: &str,
        ) -> anyhow::Result<StartupSelectionFacts> {
            Ok(self.facts.clone())
        }
    }

    #[tokio::test]
    async fn startup_status_invalid_override_reports_override_issue_not_config_issue() {
        let backend = FakeStartupBackend::with_visible_pools(["pool-main"]);

        let status =
            read_shared_startup_status(&backend, Some("pool-main"), Some("missing-override"))
                .await
                .expect("read shared startup status");

        assert_eq!(
            status.startup.startup_availability,
            AccountStartupAvailability::InvalidExplicitDefault
        );
        assert_eq!(
            status
                .startup
                .startup_resolution_issue
                .as_ref()
                .map(|issue| issue.kind),
            Some(AccountStartupResolutionIssueKind::OverridePoolUnavailable)
        );
    }

    #[tokio::test]
    async fn startup_status_multiple_pool_blocker_keeps_pooled_surface_applicable() {
        let backend = FakeStartupBackend {
            selection: AccountStartupSelectionState::default(),
            inventory: StartupPoolInventory {
                candidates: vec![
                    StartupPoolCandidate {
                        pool_id: "pool-main".to_string(),
                        display_name: None,
                        status: None,
                    },
                    StartupPoolCandidate {
                        pool_id: "pool-other".to_string(),
                        display_name: None,
                        status: None,
                    },
                ],
            },
            facts: StartupSelectionFacts {
                preferred_account_outcome: Some(StartupPreferredAccountOutcome::Selected),
                predicted_account_id: Some("acct-1".to_string()),
                any_eligible_account: true,
            },
        };

        let status: SharedStartupStatus = read_shared_startup_status(&backend, None, None)
            .await
            .expect("read shared startup status");

        assert_eq!(status.pooled_applicable, true);
        assert_eq!(
            status.startup.startup_availability,
            AccountStartupAvailability::MultiplePoolsRequireDefault
        );
    }
}
