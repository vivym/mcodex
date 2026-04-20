#![allow(dead_code)]

use crate::LoginStatus;
use crate::app_server_session::AppServerSession;
use crate::legacy_core::config::Config;
use anyhow::Result;
use anyhow::anyhow;
use codex_account_pool::AccountPoolConfig;
use codex_account_pool::LocalAccountPoolBackend;
use codex_account_pool::read_shared_startup_status;
use codex_state::AccountStartupAvailability;
use codex_state::AccountStartupResolutionIssue;
use codex_state::AccountStartupResolutionIssueSource;
use codex_state::AccountStartupStatus;
use codex_state::StateRuntime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartupNoticeData {
    pub issue_kind: StartupNoticeIssueKind,
    pub issue_source: StartupNoticeIssueSource,
    pub candidate_pool_ids: Vec<String>,
}

impl StartupNoticeData {
    fn from_startup_status(startup_status: &AccountStartupStatus) -> Option<Self> {
        match startup_status.startup_availability {
            AccountStartupAvailability::MultiplePoolsRequireDefault => Some(Self {
                issue_kind: StartupNoticeIssueKind::MultiplePoolsRequireDefault,
                issue_source: startup_status
                    .startup_resolution_issue
                    .as_ref()
                    .map_or(StartupNoticeIssueSource::None, |issue| {
                        StartupNoticeIssueSource::from(issue.source)
                    }),
                candidate_pool_ids: candidate_pool_ids(
                    startup_status.startup_resolution_issue.as_ref(),
                    &startup_status.candidate_pools,
                ),
            }),
            AccountStartupAvailability::InvalidExplicitDefault => Some(Self {
                issue_kind: StartupNoticeIssueKind::InvalidExplicitDefault,
                issue_source: startup_status
                    .startup_resolution_issue
                    .as_ref()
                    .map_or(StartupNoticeIssueSource::None, |issue| {
                        StartupNoticeIssueSource::from(issue.source)
                    }),
                candidate_pool_ids: candidate_pool_ids(
                    startup_status.startup_resolution_issue.as_ref(),
                    &startup_status.candidate_pools,
                ),
            }),
            AccountStartupAvailability::Available
            | AccountStartupAvailability::Suppressed
            | AccountStartupAvailability::Unavailable => None,
        }
    }
}

fn candidate_pool_ids(
    issue: Option<&AccountStartupResolutionIssue>,
    fallback_candidates: &[codex_state::AccountStartupCandidatePool],
) -> Vec<String> {
    if let Some(candidate_pools) =
        issue.and_then(|resolution_issue| resolution_issue.candidate_pools.as_deref())
    {
        return candidate_pools
            .iter()
            .map(|candidate_pool| candidate_pool.pool_id.clone())
            .collect();
    }

    fallback_candidates
        .iter()
        .map(|candidate_pool| candidate_pool.pool_id.clone())
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartupNoticeIssueKind {
    MultiplePoolsRequireDefault,
    InvalidExplicitDefault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartupNoticeIssueSource {
    Override,
    ConfigDefault,
    PersistedSelection,
    None,
}

impl From<AccountStartupResolutionIssueSource> for StartupNoticeIssueSource {
    fn from(source: AccountStartupResolutionIssueSource) -> Self {
        match source {
            AccountStartupResolutionIssueSource::Override => Self::Override,
            AccountStartupResolutionIssueSource::ConfigDefault => Self::ConfigDefault,
            AccountStartupResolutionIssueSource::PersistedSelection => Self::PersistedSelection,
            AccountStartupResolutionIssueSource::None => Self::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartupProbe {
    Unavailable,
    PooledAvailable {
        remote: bool,
    },
    PooledSuppressed {
        remote: bool,
    },
    PooledDefaultSelectionRequired {
        remote: bool,
        notice: StartupNoticeData,
    },
    PooledInvalidDefault {
        remote: bool,
        notice: StartupNoticeData,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartupPromptDecision {
    NeedsLogin,
    PooledOnlyNotice,
    PooledAccessPausedNotice,
    PooledDefaultSelectionNotice(StartupNoticeData),
    NoPrompt,
}

pub(crate) fn decide_startup_access(
    login_status: LoginStatus,
    provider_requires_openai_auth: bool,
    notice_hidden: bool,
    probe: StartupProbe,
) -> StartupPromptDecision {
    if !provider_requires_openai_auth || login_status != LoginStatus::NotAuthenticated {
        return StartupPromptDecision::NoPrompt;
    }

    match probe {
        StartupProbe::Unavailable => StartupPromptDecision::NeedsLogin,
        StartupProbe::PooledSuppressed { .. } => StartupPromptDecision::PooledAccessPausedNotice,
        StartupProbe::PooledDefaultSelectionRequired { notice, .. }
        | StartupProbe::PooledInvalidDefault { notice, .. } => {
            StartupPromptDecision::PooledDefaultSelectionNotice(notice)
        }
        StartupProbe::PooledAvailable { .. } if notice_hidden => StartupPromptDecision::NoPrompt,
        StartupProbe::PooledAvailable { .. } => StartupPromptDecision::PooledOnlyNotice,
    }
}

pub(crate) async fn resolve_startup_prompt_decision_with_probe(
    login_status: LoginStatus,
    provider_requires_openai_auth: bool,
    notice_hidden: bool,
    probe_result: Result<StartupProbe>,
) -> Result<StartupPromptDecision> {
    let probe = match probe_result {
        Ok(probe) => probe,
        Err(err) => {
            tracing::warn!(error = %err, "startup access probe failed; falling back to login");
            StartupProbe::Unavailable
        }
    };

    Ok(decide_startup_access(
        login_status,
        provider_requires_openai_auth,
        notice_hidden,
        probe,
    ))
}

pub(crate) async fn probe_startup_access(
    app_server_session: &AppServerSession,
    config: &Config,
) -> Result<StartupProbe> {
    if app_server_session.is_remote() {
        probe_remote_startup_access(app_server_session).await
    } else {
        probe_local_startup_access(config).await
    }
}

async fn probe_local_startup_access(config: &Config) -> Result<StartupProbe> {
    let runtime =
        StateRuntime::init(config.sqlite_home.clone(), config.model_provider_id.clone()).await?;
    let lease_ttl_secs = config
        .accounts
        .as_ref()
        .and_then(|accounts| accounts.lease_ttl_secs)
        .unwrap_or(AccountPoolConfig::default().lease_ttl_secs);
    let backend = LocalAccountPoolBackend::new(
        runtime.clone(),
        AccountPoolConfig {
            lease_ttl_secs,
            ..AccountPoolConfig::default()
        }
        .lease_ttl_duration(),
    );
    let startup_status =
        read_shared_startup_status(&backend, configured_default_pool_id(config), None).await?;

    match startup_status.startup.startup_availability {
        AccountStartupAvailability::Available => {
            Ok(StartupProbe::PooledAvailable { remote: false })
        }
        AccountStartupAvailability::Suppressed => {
            Ok(StartupProbe::PooledSuppressed { remote: false })
        }
        AccountStartupAvailability::MultiplePoolsRequireDefault => {
            let notice = StartupNoticeData::from_startup_status(&startup_status.startup)
                .ok_or_else(|| anyhow!("missing startup notice data for multi-pool blocker"))?;
            Ok(StartupProbe::PooledDefaultSelectionRequired {
                remote: false,
                notice,
            })
        }
        AccountStartupAvailability::InvalidExplicitDefault => {
            let notice = StartupNoticeData::from_startup_status(&startup_status.startup)
                .ok_or_else(|| anyhow!("missing startup notice data for invalid default"))?;
            Ok(StartupProbe::PooledInvalidDefault {
                remote: false,
                notice,
            })
        }
        AccountStartupAvailability::Unavailable => Ok(StartupProbe::Unavailable),
    }
}

async fn probe_remote_startup_access(
    app_server_session: &AppServerSession,
) -> Result<StartupProbe> {
    let response = app_server_session
        .read_account_lease_startup_probe()
        .await
        .map_err(|err| anyhow!(err.to_string()))?;
    Ok(response.map_or(
        StartupProbe::Unavailable,
        remote_startup_probe_from_response,
    ))
}

fn configured_default_pool_id(config: &Config) -> Option<&str> {
    config
        .accounts
        .as_ref()
        .and_then(|accounts| accounts.default_pool.as_deref())
}

fn remote_startup_probe_from_response(
    response: codex_app_server_protocol::AccountLeaseReadResponse,
) -> StartupProbe {
    if response.suppressed {
        return StartupProbe::PooledSuppressed { remote: true };
    }

    // Remote startup intentionally uses the visible pooled surface as a
    // best-effort availability signal instead of requiring an active lease.
    if response.pool_id.is_some() {
        StartupProbe::PooledAvailable { remote: true }
    } else {
        StartupProbe::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy_core::config::ConfigBuilder;
    use crate::legacy_core::config_loader::LoaderOverrides;
    use anyhow::anyhow;
    use codex_app_server_protocol::AccountLeaseReadResponse;
    use codex_app_server_protocol::AuthMode as AppServerAuthMode;
    use codex_config::types::AccountsConfigToml;
    use codex_state::AccountRegistryEntryUpdate;
    use codex_state::AccountStartupSelectionUpdate;
    use codex_state::state_db_path;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    async fn test_config(fixture_root: &std::path::Path) -> Config {
        test_config_with_accounts(
            fixture_root,
            Some(AccountsConfigToml {
                default_pool: Some("pool-main".to_string()),
                ..Default::default()
            }),
        )
        .await
    }

    async fn test_config_with_accounts(
        fixture_root: &std::path::Path,
        accounts: Option<AccountsConfigToml>,
    ) -> Config {
        let cwd = fixture_root.join("cwd");
        let sqlite_home = fixture_root.join("sqlite");
        std::fs::create_dir_all(&cwd).expect("create cwd fixture");
        std::fs::create_dir_all(&sqlite_home).expect("create sqlite fixture");

        let mut config = ConfigBuilder::default()
            .codex_home(fixture_root.to_path_buf())
            .fallback_cwd(Some(cwd.clone()))
            .loader_overrides(LoaderOverrides::without_managed_config_for_tests())
            .build()
            .await
            .expect("load config");
        config.accounts = accounts;
        config.cwd = AbsolutePathBuf::try_from(cwd).expect("cwd should be absolute");
        config.sqlite_home = sqlite_home;
        config
    }

    async fn seed_account(
        runtime: &StateRuntime,
        account_id: &str,
        enabled: bool,
    ) -> anyhow::Result<()> {
        runtime
            .upsert_account_registry_entry(AccountRegistryEntryUpdate {
                account_id: account_id.to_string(),
                pool_id: "pool-main".to_string(),
                position: 0,
                account_kind: "chatgpt".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: None,
                enabled,
                healthy: true,
            })
            .await?;
        Ok(())
    }

    #[test]
    fn startup_decision_is_no_prompt_when_shared_login_exists() {
        let decision = decide_startup_access(
            /*login_status*/ LoginStatus::AuthMode(AppServerAuthMode::Chatgpt),
            /*provider_requires_openai_auth*/ true,
            /*notice_hidden*/ false,
            /*probe*/ StartupProbe::PooledAvailable { remote: false },
        );

        assert_eq!(decision, StartupPromptDecision::NoPrompt);
    }

    #[test]
    fn startup_decision_uses_pooled_only_notice_when_pooled_access_exists() {
        let decision = decide_startup_access(
            LoginStatus::NotAuthenticated,
            true,
            false,
            StartupProbe::PooledAvailable { remote: false },
        );

        assert_eq!(decision, StartupPromptDecision::PooledOnlyNotice);
    }

    #[test]
    fn startup_decision_uses_paused_notice_when_probe_is_suppressed() {
        let decision = decide_startup_access(
            LoginStatus::NotAuthenticated,
            true,
            false,
            StartupProbe::PooledSuppressed { remote: true },
        );

        assert_eq!(decision, StartupPromptDecision::PooledAccessPausedNotice);
    }

    #[test]
    fn startup_decision_uses_pool_default_notice_for_multi_pool_blocker() {
        let notice = StartupNoticeData {
            issue_kind: StartupNoticeIssueKind::MultiplePoolsRequireDefault,
            issue_source: StartupNoticeIssueSource::None,
            candidate_pool_ids: vec!["team-main".to_string(), "team-other".to_string()],
        };
        let decision = decide_startup_access(
            LoginStatus::NotAuthenticated,
            true,
            false,
            StartupProbe::PooledDefaultSelectionRequired {
                remote: false,
                notice: notice.clone(),
            },
        );

        assert_eq!(
            decision,
            StartupPromptDecision::PooledDefaultSelectionNotice(notice)
        );
    }

    #[test]
    fn startup_decision_honors_hidden_notice_without_redefining_login() {
        let decision = decide_startup_access(
            LoginStatus::NotAuthenticated,
            true,
            true,
            StartupProbe::PooledAvailable { remote: false },
        );

        assert_eq!(decision, StartupPromptDecision::NoPrompt);
    }

    #[tokio::test]
    async fn local_probe_uses_state_only_membership_without_config_accounts() {
        let codex_home = tempdir().expect("tempdir");
        let config = test_config_with_accounts(codex_home.path(), None).await;
        let runtime =
            StateRuntime::init(config.sqlite_home.clone(), config.model_provider_id.clone())
                .await
                .expect("initialize runtime");

        seed_account(runtime.as_ref(), "acct-1", true)
            .await
            .expect("seed enabled account");
        runtime
            .write_account_startup_selection(AccountStartupSelectionUpdate {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: None,
                suppressed: false,
            })
            .await
            .expect("write state-only selection");

        let probe = probe_local_startup_access(&config)
            .await
            .expect("probe local startup access");

        assert_eq!(probe, StartupProbe::PooledAvailable { remote: false });
    }

    #[tokio::test]
    async fn local_probe_reports_invalid_config_default_without_membership() {
        let codex_home = tempdir().expect("tempdir");
        let config = test_config(codex_home.path()).await;

        let probe = probe_local_startup_access(&config)
            .await
            .expect("probe local startup access");

        assert_eq!(
            probe,
            StartupProbe::PooledInvalidDefault {
                remote: false,
                notice: StartupNoticeData {
                    issue_kind: StartupNoticeIssueKind::InvalidExplicitDefault,
                    issue_source: StartupNoticeIssueSource::ConfigDefault,
                    candidate_pool_ids: Vec::new(),
                },
            }
        );
    }

    #[tokio::test]
    async fn local_probe_reports_invalid_config_default_without_preexisting_sqlite_file() {
        let codex_home = tempdir().expect("tempdir");
        let config = test_config(codex_home.path()).await;
        let state_path = state_db_path(config.sqlite_home.as_path());
        assert!(!state_path.exists());

        let probe = probe_local_startup_access(&config)
            .await
            .expect("probe local startup access");

        assert_eq!(
            probe,
            StartupProbe::PooledInvalidDefault {
                remote: false,
                notice: StartupNoticeData {
                    issue_kind: StartupNoticeIssueKind::InvalidExplicitDefault,
                    issue_source: StartupNoticeIssueSource::ConfigDefault,
                    candidate_pool_ids: Vec::new(),
                },
            }
        );
        assert!(state_path.exists());
    }

    #[tokio::test]
    async fn local_probe_reports_suppressed_pool_as_paused() {
        let codex_home = tempdir().expect("tempdir");
        let config = test_config(codex_home.path()).await;
        let runtime =
            StateRuntime::init(config.sqlite_home.clone(), config.model_provider_id.clone())
                .await
                .expect("initialize runtime");

        seed_account(runtime.as_ref(), "acct-1", true)
            .await
            .expect("seed enabled account");
        runtime
            .write_account_startup_selection(AccountStartupSelectionUpdate {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-1".to_string()),
                suppressed: true,
            })
            .await
            .expect("write suppressed selection");

        let probe = probe_local_startup_access(&config)
            .await
            .expect("probe local startup access");

        assert_eq!(probe, StartupProbe::PooledSuppressed { remote: false });
    }

    #[tokio::test]
    async fn local_probe_reports_visible_pool_even_without_predicted_account() {
        let codex_home = tempdir().expect("tempdir");
        let config = test_config(codex_home.path()).await;
        let runtime =
            StateRuntime::init(config.sqlite_home.clone(), config.model_provider_id.clone())
                .await
                .expect("initialize runtime");

        seed_account(runtime.as_ref(), "acct-disabled", false)
            .await
            .expect("seed disabled preferred account");
        seed_account(runtime.as_ref(), "acct-enabled", true)
            .await
            .expect("seed enabled backup account");
        runtime
            .write_account_startup_selection(AccountStartupSelectionUpdate {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-disabled".to_string()),
                suppressed: false,
            })
            .await
            .expect("write visible selection");

        let preview = runtime
            .preview_account_startup_selection(Some("pool-main"))
            .await
            .expect("preview startup selection");
        assert_eq!(preview.predicted_account_id, None);
        assert!(!preview.suppressed);

        let probe = probe_local_startup_access(&config)
            .await
            .expect("probe local startup access");

        assert_eq!(probe, StartupProbe::PooledAvailable { remote: false });
    }

    #[tokio::test]
    async fn local_probe_reports_available_when_effective_pool_has_no_enabled_accounts() {
        let codex_home = tempdir().expect("tempdir");
        let config = test_config(codex_home.path()).await;
        let runtime =
            StateRuntime::init(config.sqlite_home.clone(), config.model_provider_id.clone())
                .await
                .expect("initialize runtime");

        seed_account(runtime.as_ref(), "acct-disabled", false)
            .await
            .expect("seed disabled account");
        runtime
            .write_account_startup_selection(AccountStartupSelectionUpdate {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-disabled".to_string()),
                suppressed: false,
            })
            .await
            .expect("write selection");

        let probe = probe_local_startup_access(&config)
            .await
            .expect("probe local startup access");

        assert_eq!(probe, StartupProbe::PooledAvailable { remote: false });
    }

    #[test]
    fn remote_probe_maps_suppressed_surface_to_paused() {
        let probe = remote_startup_probe_from_response(AccountLeaseReadResponse {
            active: false,
            suppressed: true,
            account_id: None,
            pool_id: Some("pool-main".to_string()),
            lease_id: None,
            lease_epoch: None,
            lease_acquired_at: None,
            health_state: None,
            switch_reason: None,
            suppression_reason: Some("durablySuppressed".to_string()),
            transport_reset_generation: None,
            last_remote_context_reset_turn_id: None,
            min_switch_interval_secs: None,
            proactive_switch_pending: None,
            proactive_switch_suppressed: None,
            proactive_switch_allowed_at: None,
            next_eligible_at: None,
            effective_pool_resolution_source: Some("persistedSelection".to_string()),
            configured_default_pool_id: None,
            persisted_default_pool_id: Some("pool-main".to_string()),
        });

        assert_eq!(probe, StartupProbe::PooledSuppressed { remote: true });
    }

    #[test]
    fn remote_probe_maps_visible_surface_to_pooled_available() {
        let probe = remote_startup_probe_from_response(AccountLeaseReadResponse {
            active: false,
            suppressed: false,
            account_id: None,
            pool_id: Some("pool-main".to_string()),
            lease_id: None,
            lease_epoch: None,
            lease_acquired_at: None,
            health_state: None,
            switch_reason: Some("noEligibleAccount".to_string()),
            suppression_reason: None,
            transport_reset_generation: None,
            last_remote_context_reset_turn_id: None,
            min_switch_interval_secs: None,
            proactive_switch_pending: None,
            proactive_switch_suppressed: None,
            proactive_switch_allowed_at: None,
            next_eligible_at: None,
            effective_pool_resolution_source: Some("persistedSelection".to_string()),
            configured_default_pool_id: None,
            persisted_default_pool_id: Some("pool-main".to_string()),
        });

        assert_eq!(probe, StartupProbe::PooledAvailable { remote: true });
    }

    #[test]
    fn remote_probe_maps_empty_response_to_unavailable() {
        let probe = remote_startup_probe_from_response(AccountLeaseReadResponse {
            active: false,
            suppressed: false,
            account_id: None,
            pool_id: None,
            lease_id: None,
            lease_epoch: None,
            lease_acquired_at: None,
            health_state: None,
            switch_reason: None,
            suppression_reason: None,
            transport_reset_generation: None,
            last_remote_context_reset_turn_id: None,
            min_switch_interval_secs: None,
            proactive_switch_pending: None,
            proactive_switch_suppressed: None,
            proactive_switch_allowed_at: None,
            next_eligible_at: None,
            effective_pool_resolution_source: None,
            configured_default_pool_id: None,
            persisted_default_pool_id: None,
        });

        assert_eq!(probe, StartupProbe::Unavailable);
    }

    #[tokio::test]
    async fn startup_probe_failure_falls_back_to_needs_login() {
        let decision = resolve_startup_prompt_decision_with_probe(
            LoginStatus::NotAuthenticated,
            true,
            false,
            Err(anyhow!("probe failed")),
        )
        .await
        .expect("probe failure should not bubble");

        assert_eq!(decision, StartupPromptDecision::NeedsLogin);
    }
}
