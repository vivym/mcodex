use std::sync::Arc;

use anyhow::Context;
use codex_app_server_protocol::AccountLeaseReadResponse;
use codex_app_server_protocol::AccountLeaseUpdatedNotification;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_core::AccountLeaseRuntimeReason;
use codex_core::AccountLeaseRuntimeSnapshot;
use codex_core::config::Config;
use codex_state::AccountHealthState;
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
    live_snapshot: Option<AccountLeaseRuntimeSnapshot>,
) -> Result<AccountLeaseReadResponse, JSONRPCErrorError> {
    if let Some(live_snapshot) = live_snapshot {
        return Ok(account_lease_response_from_runtime_snapshot(&live_snapshot));
    }

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

pub(crate) fn account_lease_updated_notification_from_runtime_snapshot(
    live_snapshot: &AccountLeaseRuntimeSnapshot,
) -> AccountLeaseUpdatedNotification {
    account_lease_response_from_runtime_snapshot(live_snapshot).into()
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
        lease_acquired_at: None,
        health_state: health_state_for_preview(&preview.eligibility, preview.predicted_account_id),
        switch_reason,
        suppression_reason,
        transport_reset_generation: None,
        last_remote_context_reset_turn_id: None,
        min_switch_interval_secs: None,
        proactive_switch_pending: None,
        proactive_switch_suppressed: None,
        proactive_switch_allowed_at: None,
        next_eligible_at: None,
    }
}

fn account_lease_response_from_runtime_snapshot(
    live_snapshot: &AccountLeaseRuntimeSnapshot,
) -> AccountLeaseReadResponse {
    AccountLeaseReadResponse {
        active: live_snapshot.active,
        suppressed: live_snapshot.suppressed,
        account_id: live_snapshot.account_id.clone(),
        pool_id: live_snapshot.pool_id.clone(),
        lease_id: live_snapshot.lease_id.clone(),
        lease_epoch: live_snapshot
            .lease_epoch
            .and_then(|epoch| u64::try_from(epoch).ok()),
        lease_acquired_at: live_snapshot
            .lease_acquired_at
            .map(|timestamp| timestamp.timestamp()),
        health_state: live_snapshot_health_state(live_snapshot.health_state),
        switch_reason: live_snapshot
            .switch_reason
            .map(runtime_reason_to_wire_string),
        suppression_reason: live_snapshot
            .suppression_reason
            .map(runtime_reason_to_wire_string),
        transport_reset_generation: live_snapshot.transport_reset_generation,
        last_remote_context_reset_turn_id: live_snapshot.last_remote_context_reset_turn_id.clone(),
        min_switch_interval_secs: live_snapshot.min_switch_interval_secs,
        proactive_switch_pending: live_snapshot.proactive_switch_pending,
        proactive_switch_suppressed: live_snapshot.proactive_switch_suppressed,
        proactive_switch_allowed_at: live_snapshot
            .proactive_switch_allowed_at
            .map(|timestamp| timestamp.timestamp()),
        next_eligible_at: live_snapshot
            .next_eligible_at
            .map(|timestamp| timestamp.timestamp()),
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
        AccountStartupEligibility::PreferredAccountDisabled => {
            (Some("preferredAccountDisabled".to_string()), None)
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
        AccountStartupEligibility::PreferredAccountDisabled => Some("unavailable".to_string()),
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

fn live_snapshot_health_state(health_state: Option<AccountHealthState>) -> Option<String> {
    match health_state {
        Some(AccountHealthState::Healthy) => Some("healthy".to_string()),
        Some(AccountHealthState::RateLimited) | Some(AccountHealthState::Unauthorized) => {
            Some("unhealthy".to_string())
        }
        None => None,
    }
}

fn runtime_reason_to_wire_string(reason: AccountLeaseRuntimeReason) -> String {
    match reason {
        AccountLeaseRuntimeReason::StartupSuppressed => "durablySuppressed".to_string(),
        AccountLeaseRuntimeReason::MissingPool => "missingPool".to_string(),
        AccountLeaseRuntimeReason::PreferredAccountSelected => {
            "preferredAccountSelected".to_string()
        }
        AccountLeaseRuntimeReason::AutomaticAccountSelected => {
            "automaticAccountSelected".to_string()
        }
        AccountLeaseRuntimeReason::PreferredAccountMissing => "preferredAccountMissing".to_string(),
        AccountLeaseRuntimeReason::PreferredAccountInOtherPool => {
            "preferredAccountInOtherPool".to_string()
        }
        AccountLeaseRuntimeReason::PreferredAccountDisabled => {
            "preferredAccountDisabled".to_string()
        }
        AccountLeaseRuntimeReason::PreferredAccountUnhealthy => {
            "preferredAccountUnhealthy".to_string()
        }
        AccountLeaseRuntimeReason::PreferredAccountBusy => "preferredAccountBusy".to_string(),
        AccountLeaseRuntimeReason::NoEligibleAccount => "noEligibleAccount".to_string(),
        AccountLeaseRuntimeReason::NonReplayableTurn => "nonReplayableTurn".to_string(),
    }
}

fn internal_error(err: anyhow::Error) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: INTERNAL_ERROR_CODE,
        message: err.to_string(),
        data: None,
    }
}
