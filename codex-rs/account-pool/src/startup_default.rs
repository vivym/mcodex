use codex_state::AccountStartupSelectionUpdate;
use codex_state::StateRuntime;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDefaultPoolSetRequest {
    pub pool_id: String,
    pub configured_default_pool_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDefaultPoolClearRequest {
    pub configured_default_pool_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDefaultPoolMutationOutcome {
    pub state_changed: bool,
    pub persisted_default_pool_id: Option<String>,
    pub effective_pool_still_config_controlled: bool,
    pub suppressed: bool,
    pub preferred_account_cleared: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalDefaultPoolSetError {
    PoolNotVisible { pool_id: String },
}

impl fmt::Display for LocalDefaultPoolSetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PoolNotVisible { pool_id } => {
                write!(
                    f,
                    "pool {pool_id} is not visible in local startup inventory"
                )
            }
        }
    }
}

impl std::error::Error for LocalDefaultPoolSetError {}

pub async fn set_local_default_pool(
    runtime: &StateRuntime,
    request: LocalDefaultPoolSetRequest,
) -> anyhow::Result<LocalDefaultPoolMutationOutcome> {
    let requested_pool_id = request.pool_id.as_str();
    if !runtime
        .read_account_startup_inventory()
        .await?
        .into_iter()
        .map(|pool| pool.pool_id)
        .any(|pool_id| pool_id == requested_pool_id)
    {
        return Err(LocalDefaultPoolSetError::PoolNotVisible {
            pool_id: request.pool_id,
        }
        .into());
    }

    let selection = runtime.read_account_startup_selection().await?;
    // The reset rule is presence-based: a configured default, valid or invalid,
    // still means config owns the effective pool for this helper's purposes.
    let config_controls_effective_pool = request.configured_default_pool_id.is_some();
    let preferred_account_cleared =
        !config_controls_effective_pool && selection.preferred_account_id.is_some();
    let state_changed = selection.default_pool_id.as_deref() != Some(request.pool_id.as_str())
        || preferred_account_cleared;
    if state_changed {
        runtime
            .write_account_startup_selection(AccountStartupSelectionUpdate {
                default_pool_id: Some(request.pool_id.clone()),
                preferred_account_id: if !config_controls_effective_pool {
                    None
                } else {
                    selection.preferred_account_id.clone()
                },
                suppressed: selection.suppressed,
            })
            .await?;
    }

    Ok(LocalDefaultPoolMutationOutcome {
        state_changed,
        persisted_default_pool_id: Some(request.pool_id),
        effective_pool_still_config_controlled: config_controls_effective_pool,
        suppressed: selection.suppressed,
        preferred_account_cleared,
    })
}

pub async fn clear_local_default_pool(
    runtime: &StateRuntime,
    request: LocalDefaultPoolClearRequest,
) -> anyhow::Result<LocalDefaultPoolMutationOutcome> {
    let selection = runtime.read_account_startup_selection().await?;
    // The reset rule is presence-based: a configured default, valid or invalid,
    // still means config owns the effective pool for this helper's purposes.
    let config_controls_effective_pool = request.configured_default_pool_id.is_some();
    let preferred_account_cleared = selection.default_pool_id.is_some()
        && !config_controls_effective_pool
        && selection.preferred_account_id.is_some();
    let state_changed = selection.default_pool_id.is_some();
    if state_changed {
        runtime
            .write_account_startup_selection(AccountStartupSelectionUpdate {
                default_pool_id: None,
                preferred_account_id: if !config_controls_effective_pool {
                    None
                } else {
                    selection.preferred_account_id.clone()
                },
                suppressed: selection.suppressed,
            })
            .await?;
    }

    Ok(LocalDefaultPoolMutationOutcome {
        state_changed,
        persisted_default_pool_id: None,
        effective_pool_still_config_controlled: config_controls_effective_pool,
        suppressed: selection.suppressed,
        preferred_account_cleared,
    })
}

#[cfg(test)]
mod tests {
    use super::LocalDefaultPoolClearRequest;
    use super::LocalDefaultPoolMutationOutcome;
    use super::LocalDefaultPoolSetRequest;
    use super::clear_local_default_pool;
    use super::set_local_default_pool;
    use codex_state::AccountStartupSelectionState;
    use codex_state::AccountStartupSelectionUpdate;
    use codex_state::LegacyAccountImport;
    use codex_state::StateRuntime;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[tokio::test]
    async fn set_default_clears_preferred_when_state_backed_source_is_active() {
        let (_tempdir, runtime) =
            seeded_runtime_with_pools(&[("acct-main", "pool-main"), ("acct-other", "pool-other")])
                .await;
        runtime
            .write_account_startup_selection(AccountStartupSelectionUpdate {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-main".to_string()),
                suppressed: true,
            })
            .await
            .expect("write selection");

        let outcome = set_local_default_pool(
            runtime.as_ref(),
            LocalDefaultPoolSetRequest {
                pool_id: "pool-other".to_string(),
                configured_default_pool_id: None,
            },
        )
        .await
        .expect("set default");

        assert_eq!(
            outcome,
            LocalDefaultPoolMutationOutcome {
                state_changed: true,
                persisted_default_pool_id: Some("pool-other".to_string()),
                effective_pool_still_config_controlled: false,
                suppressed: true,
                preferred_account_cleared: true,
            }
        );
        assert_eq!(
            runtime.read_account_startup_selection().await.unwrap(),
            AccountStartupSelectionState {
                default_pool_id: Some("pool-other".to_string()),
                preferred_account_id: None,
                suppressed: true,
            }
        );
    }

    #[tokio::test]
    async fn set_default_same_pool_without_preferred_reset_is_no_op() {
        let (_tempdir, runtime) = seeded_runtime_with_pools(&[("acct-main", "pool-main")]).await;
        runtime
            .write_account_startup_selection(AccountStartupSelectionUpdate {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-main".to_string()),
                suppressed: true,
            })
            .await
            .expect("write selection");

        let outcome = set_local_default_pool(
            runtime.as_ref(),
            LocalDefaultPoolSetRequest {
                pool_id: "pool-main".to_string(),
                configured_default_pool_id: Some("config-main".to_string()),
            },
        )
        .await
        .expect("set default");

        assert_eq!(
            outcome,
            LocalDefaultPoolMutationOutcome {
                state_changed: false,
                persisted_default_pool_id: Some("pool-main".to_string()),
                effective_pool_still_config_controlled: true,
                suppressed: true,
                preferred_account_cleared: false,
            }
        );
        assert_eq!(
            runtime.read_account_startup_selection().await.unwrap(),
            AccountStartupSelectionState {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-main".to_string()),
                suppressed: true,
            }
        );
    }

    #[tokio::test]
    async fn set_default_same_pool_with_preferred_reset_changes_state() {
        let (_tempdir, runtime) = seeded_runtime_with_pools(&[("acct-main", "pool-main")]).await;
        runtime
            .write_account_startup_selection(AccountStartupSelectionUpdate {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-main".to_string()),
                suppressed: true,
            })
            .await
            .expect("write selection");

        let outcome = set_local_default_pool(
            runtime.as_ref(),
            LocalDefaultPoolSetRequest {
                pool_id: "pool-main".to_string(),
                configured_default_pool_id: None,
            },
        )
        .await
        .expect("set default");

        assert_eq!(
            outcome,
            LocalDefaultPoolMutationOutcome {
                state_changed: true,
                persisted_default_pool_id: Some("pool-main".to_string()),
                effective_pool_still_config_controlled: false,
                suppressed: true,
                preferred_account_cleared: true,
            }
        );
        assert_eq!(
            runtime.read_account_startup_selection().await.unwrap(),
            AccountStartupSelectionState {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: None,
                suppressed: true,
            }
        );
    }

    #[tokio::test]
    async fn set_default_rejects_pool_that_is_not_visible() {
        let (_tempdir, runtime) = seeded_runtime_with_pools(&[("acct-main", "pool-main")]).await;

        let error = set_local_default_pool(
            runtime.as_ref(),
            LocalDefaultPoolSetRequest {
                pool_id: "missing-pool".to_string(),
                configured_default_pool_id: None,
            },
        )
        .await
        .expect_err("missing pool should fail");

        assert_eq!(
            error.to_string(),
            "pool missing-pool is not visible in local startup inventory"
        );
    }

    #[tokio::test]
    async fn clear_default_clears_preferred_only_when_state_backed_default_exists() {
        let (_tempdir, runtime) = seeded_runtime_with_pools(&[("acct-main", "pool-main")]).await;
        runtime
            .write_account_startup_selection(AccountStartupSelectionUpdate {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-main".to_string()),
                suppressed: true,
            })
            .await
            .expect("write selection");

        let outcome = clear_local_default_pool(
            runtime.as_ref(),
            LocalDefaultPoolClearRequest {
                configured_default_pool_id: None,
            },
        )
        .await
        .expect("clear default");

        assert_eq!(
            outcome,
            LocalDefaultPoolMutationOutcome {
                state_changed: true,
                persisted_default_pool_id: None,
                effective_pool_still_config_controlled: false,
                suppressed: true,
                preferred_account_cleared: true,
            }
        );
        assert_eq!(
            runtime.read_account_startup_selection().await.unwrap(),
            AccountStartupSelectionState {
                default_pool_id: None,
                preferred_account_id: None,
                suppressed: true,
            }
        );
    }

    #[tokio::test]
    async fn clear_default_absent_persisted_default_is_no_op() {
        let (_tempdir, runtime) = seeded_runtime_with_pools(&[("acct-main", "pool-main")]).await;
        runtime
            .write_account_startup_selection(AccountStartupSelectionUpdate {
                default_pool_id: None,
                preferred_account_id: Some("acct-main".to_string()),
                suppressed: true,
            })
            .await
            .expect("write selection");

        let outcome = clear_local_default_pool(
            runtime.as_ref(),
            LocalDefaultPoolClearRequest {
                configured_default_pool_id: None,
            },
        )
        .await
        .expect("clear default");

        assert_eq!(
            outcome,
            LocalDefaultPoolMutationOutcome {
                state_changed: false,
                persisted_default_pool_id: None,
                effective_pool_still_config_controlled: false,
                suppressed: true,
                preferred_account_cleared: false,
            }
        );
        assert_eq!(
            runtime.read_account_startup_selection().await.unwrap(),
            AccountStartupSelectionState {
                default_pool_id: None,
                preferred_account_id: Some("acct-main".to_string()),
                suppressed: true,
            }
        );
    }

    #[tokio::test]
    async fn set_default_preserves_preferred_when_config_controls_effective_pool() {
        let (_tempdir, runtime) =
            seeded_runtime_with_pools(&[("acct-main", "pool-main"), ("acct-other", "pool-other")])
                .await;
        runtime
            .write_account_startup_selection(AccountStartupSelectionUpdate {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-main".to_string()),
                suppressed: true,
            })
            .await
            .expect("write selection");

        let outcome = set_local_default_pool(
            runtime.as_ref(),
            LocalDefaultPoolSetRequest {
                pool_id: "pool-other".to_string(),
                configured_default_pool_id: Some("config-main".to_string()),
            },
        )
        .await
        .expect("set default");

        assert_eq!(
            outcome,
            LocalDefaultPoolMutationOutcome {
                state_changed: true,
                persisted_default_pool_id: Some("pool-other".to_string()),
                effective_pool_still_config_controlled: true,
                suppressed: true,
                preferred_account_cleared: false,
            }
        );
        assert_eq!(
            runtime.read_account_startup_selection().await.unwrap(),
            AccountStartupSelectionState {
                default_pool_id: Some("pool-other".to_string()),
                preferred_account_id: Some("acct-main".to_string()),
                suppressed: true,
            }
        );
    }

    #[tokio::test]
    async fn clear_default_preserves_preferred_when_config_controls_effective_pool() {
        let (_tempdir, runtime) = seeded_runtime_with_pools(&[("acct-main", "pool-main")]).await;
        runtime
            .write_account_startup_selection(AccountStartupSelectionUpdate {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-main".to_string()),
                suppressed: true,
            })
            .await
            .expect("write selection");

        let outcome = clear_local_default_pool(
            runtime.as_ref(),
            LocalDefaultPoolClearRequest {
                configured_default_pool_id: Some("config-main".to_string()),
            },
        )
        .await
        .expect("clear default");

        assert_eq!(
            outcome,
            LocalDefaultPoolMutationOutcome {
                state_changed: true,
                persisted_default_pool_id: None,
                effective_pool_still_config_controlled: true,
                suppressed: true,
                preferred_account_cleared: false,
            }
        );
        assert_eq!(
            runtime.read_account_startup_selection().await.unwrap(),
            AccountStartupSelectionState {
                default_pool_id: None,
                preferred_account_id: Some("acct-main".to_string()),
                suppressed: true,
            }
        );
    }

    async fn seeded_runtime_with_pools(
        memberships: &[(&str, &str)],
    ) -> (TempDir, Arc<StateRuntime>) {
        let tempdir = tempfile::tempdir().expect("create tempdir");
        let runtime = StateRuntime::init(tempdir.path().to_path_buf(), "test-provider".to_string())
            .await
            .expect("initialize runtime");

        for (account_id, pool_id) in memberships {
            runtime
                .import_legacy_default_account(LegacyAccountImport {
                    account_id: (*account_id).to_string(),
                })
                .await
                .expect("import legacy account");
            runtime
                .assign_account_pool(account_id, pool_id)
                .await
                .expect("assign account pool");
        }

        (tempdir, runtime)
    }
}
