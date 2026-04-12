use std::sync::Arc;

use anyhow::Context;
use codex_app_server_protocol::AccountLeaseReadResponse;
use codex_app_server_protocol::AccountLeaseUpdatedNotification;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_core::config::Config;
use codex_state::AccountStartupEligibility;
use codex_state::AccountStartupSelectionPreview;
use codex_state::AccountStartupSelectionUpdate;
use codex_state::StateRuntime;
use codex_state::state_db_path;

use crate::error_code::INTERNAL_ERROR_CODE;

pub(crate) fn pooled_mode_is_configured(config: &Config) -> bool {
    config.accounts.is_some()
}

pub(crate) async fn read_account_lease(
    config: &Config,
) -> Result<AccountLeaseReadResponse, JSONRPCErrorError> {
    let Some(state_db) = maybe_open_state_db(config).await? else {
        return Ok(empty_account_lease_response());
    };

    let preview = state_db
        .preview_account_startup_selection(configured_default_pool_id(config))
        .await
        .context("preview account startup selection")
        .map_err(internal_error)?;

    Ok(account_lease_response_from_preview(preview))
}

pub(crate) async fn resume_account_lease(
    config: &Config,
) -> Result<AccountLeaseUpdatedNotification, JSONRPCErrorError> {
    let Some(state_db) = maybe_open_state_db(config).await? else {
        return Ok(empty_account_lease_response().into());
    };

    let selection = state_db
        .read_account_startup_selection()
        .await
        .context("read account startup selection")
        .map_err(internal_error)?;

    state_db
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: selection.default_pool_id,
            preferred_account_id: None,
            suppressed: false,
        })
        .await
        .context("clear durable account startup selection suppression")
        .map_err(internal_error)?;

    let preview = state_db
        .preview_account_startup_selection(configured_default_pool_id(config))
        .await
        .context("preview resumed account startup selection")
        .map_err(internal_error)?;

    Ok(account_lease_response_from_preview(preview).into())
}

pub(crate) async fn suppress_account_lease_on_logout(
    config: &Config,
) -> Result<Option<AccountLeaseUpdatedNotification>, JSONRPCErrorError> {
    let Some(state_db) = maybe_open_state_db(config).await? else {
        return Ok(None);
    };

    let selection = state_db
        .read_account_startup_selection()
        .await
        .context("read account startup selection")
        .map_err(internal_error)?;
    let has_startup_selection = selection.default_pool_id.is_some()
        || selection.preferred_account_id.is_some()
        || selection.suppressed;
    if !pooled_mode_is_configured(config) && !has_startup_selection {
        return Ok(None);
    }

    state_db
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: selection.default_pool_id,
            preferred_account_id: selection.preferred_account_id,
            suppressed: true,
        })
        .await
        .context("write durable suppressed account startup selection")
        .map_err(internal_error)?;

    let preview = state_db
        .preview_account_startup_selection(configured_default_pool_id(config))
        .await
        .context("preview suppressed account startup selection")
        .map_err(internal_error)?;

    Ok(Some(account_lease_response_from_preview(preview).into()))
}

async fn maybe_open_state_db(
    config: &Config,
) -> Result<Option<Arc<StateRuntime>>, JSONRPCErrorError> {
    let state_path = state_db_path(config.sqlite_home.as_path());
    if !pooled_mode_is_configured(config) && !state_path.exists() {
        return Ok(None);
    }

    StateRuntime::init(config.sqlite_home.clone(), config.model_provider_id.clone())
        .await
        .context("initialize account lease state db")
        .map(Some)
        .map_err(internal_error)
}

fn configured_default_pool_id(config: &Config) -> Option<&str> {
    config
        .accounts
        .as_ref()
        .and_then(|accounts| accounts.default_pool.as_deref())
}

fn empty_account_lease_response() -> AccountLeaseReadResponse {
    AccountLeaseReadResponse {
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
    }
}

fn account_lease_response_from_preview(
    preview: AccountStartupSelectionPreview,
) -> AccountLeaseReadResponse {
    let active = !preview.suppressed && preview.predicted_account_id.is_some();
    let (switch_reason, suppression_reason) = selection_reasons(&preview.eligibility);

    AccountLeaseReadResponse {
        active,
        suppressed: preview.suppressed,
        account_id: preview.predicted_account_id.clone(),
        pool_id: preview.effective_pool_id,
        lease_id: None,
        lease_epoch: None,
        health_state: health_state_for_preview(&preview.eligibility, preview.predicted_account_id),
        switch_reason,
        suppression_reason,
        transport_reset_generation: None,
        last_remote_context_reset_turn_id: None,
        next_eligible_at: None,
    }
}

fn selection_reasons(eligibility: &AccountStartupEligibility) -> (Option<String>, Option<String>) {
    match eligibility {
        AccountStartupEligibility::Suppressed => (None, Some("durablySuppressed".to_string())),
        AccountStartupEligibility::MissingPool => (Some("missingPool".to_string()), None),
        AccountStartupEligibility::PreferredAccountSelected => {
            (Some("preferredAccountSelected".to_string()), None)
        }
        AccountStartupEligibility::AutomaticAccountSelected => {
            (Some("automaticAccountSelected".to_string()), None)
        }
        AccountStartupEligibility::PreferredAccountMissing => {
            (Some("preferredAccountMissing".to_string()), None)
        }
        AccountStartupEligibility::PreferredAccountInOtherPool { .. } => {
            (Some("preferredAccountInOtherPool".to_string()), None)
        }
        AccountStartupEligibility::PreferredAccountUnhealthy => {
            (Some("preferredAccountUnhealthy".to_string()), None)
        }
        AccountStartupEligibility::PreferredAccountBusy => {
            (Some("preferredAccountBusy".to_string()), None)
        }
        AccountStartupEligibility::NoEligibleAccount => {
            (Some("noEligibleAccount".to_string()), None)
        }
    }
}

fn health_state_for_preview(
    eligibility: &AccountStartupEligibility,
    predicted_account_id: Option<String>,
) -> Option<String> {
    if predicted_account_id.is_some() {
        return Some("healthy".to_string());
    }

    match eligibility {
        AccountStartupEligibility::PreferredAccountUnhealthy => Some("unhealthy".to_string()),
        AccountStartupEligibility::PreferredAccountBusy => Some("busy".to_string()),
        AccountStartupEligibility::NoEligibleAccount => Some("unavailable".to_string()),
        AccountStartupEligibility::Suppressed
        | AccountStartupEligibility::MissingPool
        | AccountStartupEligibility::PreferredAccountSelected
        | AccountStartupEligibility::AutomaticAccountSelected
        | AccountStartupEligibility::PreferredAccountMissing
        | AccountStartupEligibility::PreferredAccountInOtherPool { .. } => None,
    }
}

fn internal_error(err: anyhow::Error) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: INTERNAL_ERROR_CODE,
        message: err.to_string(),
        data: None,
    }
}
