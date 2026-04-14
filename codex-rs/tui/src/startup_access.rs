#![allow(dead_code)]

use crate::LoginStatus;
use crate::app_server_session::AppServerSession;
use anyhow::Result;
use anyhow::anyhow;
use codex_core::config::Config;
use codex_state::StateRuntime;
use codex_state::state_db_path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartupProbe {
    Unavailable,
    PooledAvailable { remote: bool },
    PooledSuppressed { remote: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartupPromptDecision {
    NeedsLogin,
    PooledOnlyNotice,
    PooledAccessPausedNotice,
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
    let state_path = state_db_path(config.sqlite_home.as_path());
    if configured_default_pool_id(config).is_none() && !state_path.exists() {
        return Ok(StartupProbe::Unavailable);
    }

    let runtime =
        StateRuntime::init(config.sqlite_home.clone(), config.model_provider_id.clone()).await?;
    let preview = runtime
        .preview_account_startup_selection(configured_default_pool_id(config))
        .await?;
    let Some(pool_id) = preview.effective_pool_id.as_deref() else {
        return Ok(StartupProbe::Unavailable);
    };
    let diagnostic = runtime
        .read_account_pool_diagnostic(pool_id, preview.preferred_account_id.as_deref())
        .await?;

    if !diagnostic.accounts.iter().any(|account| account.enabled) {
        return Ok(StartupProbe::Unavailable);
    }

    if preview.suppressed {
        Ok(StartupProbe::PooledSuppressed { remote: false })
    } else {
        Ok(StartupProbe::PooledAvailable { remote: false })
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

    if response.pool_id.is_some() {
        StartupProbe::PooledAvailable { remote: true }
    } else {
        StartupProbe::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use codex_app_server_protocol::AccountLeaseReadResponse;
    use codex_app_server_protocol::AuthMode as AppServerAuthMode;
    use pretty_assertions::assert_eq;

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
    fn startup_decision_honors_hidden_notice_without_redefining_login() {
        let decision = decide_startup_access(
            LoginStatus::NotAuthenticated,
            true,
            true,
            StartupProbe::PooledAvailable { remote: false },
        );

        assert_eq!(decision, StartupPromptDecision::NoPrompt);
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
            health_state: None,
            switch_reason: None,
            suppression_reason: Some("durablySuppressed".to_string()),
            transport_reset_generation: None,
            last_remote_context_reset_turn_id: None,
            next_eligible_at: None,
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
            health_state: None,
            switch_reason: Some("noEligibleAccount".to_string()),
            suppression_reason: None,
            transport_reset_generation: None,
            last_remote_context_reset_turn_id: None,
            next_eligible_at: None,
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
            health_state: None,
            switch_reason: None,
            suppression_reason: None,
            transport_reset_generation: None,
            last_remote_context_reset_turn_id: None,
            next_eligible_at: None,
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
