use chrono::Duration;
use codex_account_pool::LocalAccountPoolBackend;
use codex_account_pool::LocalDefaultPoolClearRequest;
use codex_account_pool::LocalDefaultPoolSetError;
use codex_account_pool::LocalDefaultPoolSetRequest;
use codex_account_pool::SharedStartupStatus;
use codex_account_pool::clear_local_default_pool;
use codex_account_pool::read_shared_startup_status;
use codex_account_pool::set_local_default_pool;
use std::sync::Arc;

use anyhow::Context;
use codex_app_server_protocol::AccountLeaseReadResponse;
use codex_app_server_protocol::AccountLeaseUpdatedNotification;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_core::AccountLeaseRuntimeReason;
use codex_core::AccountLeaseRuntimeSnapshot;
use codex_core::config::Config;
use codex_state::AccountHealthState;
use codex_state::AccountStartupAvailability;
use codex_state::AccountStartupEligibility;
use codex_state::AccountStartupSelectionPreview;
use codex_state::AccountStartupSelectionUpdate;
use codex_state::AccountStartupStatus;
use codex_state::StateRuntime;

use crate::account_startup_snapshot::effective_pool_resolution_source_to_wire_string;
use crate::account_startup_snapshot::snapshot_from_startup_status;
use crate::account_startup_snapshot::unavailable_snapshot;
use crate::error_code::INTERNAL_ERROR_CODE;
use crate::error_code::INVALID_PARAMS_ERROR_CODE;

pub(crate) async fn pooled_mode_is_enabled(config: &Config) -> Result<bool, JSONRPCErrorError> {
    Ok(
        read_account_lease_startup_context_for_runtime_admission(config)
            .await?
            .is_some(),
    )
}

pub(crate) async fn account_lease_updated_notification_from_runtime_snapshot(
    config: &Config,
    live_snapshot: &AccountLeaseRuntimeSnapshot,
) -> Result<AccountLeaseUpdatedNotification, JSONRPCErrorError> {
    let startup_context = read_account_lease_startup_context(config).await?;

    Ok(
        account_lease_response_from_runtime_snapshot(live_snapshot, Some(&startup_context.startup))
            .into(),
    )
}

pub(crate) async fn read_account_lease(
    config: &Config,
    live_snapshot: Option<AccountLeaseRuntimeSnapshot>,
) -> Result<AccountLeaseReadResponse, JSONRPCErrorError> {
    let startup_context = read_account_lease_startup_context(config).await?;
    if let Some(live_snapshot) = live_snapshot {
        return Ok(account_lease_response_from_runtime_snapshot(
            &live_snapshot,
            Some(&startup_context.startup),
        ));
    }

    if startup_context.startup.startup_availability == AccountStartupAvailability::Unavailable {
        return Ok(empty_account_lease_response());
    }

    Ok(account_lease_response_from_startup_status(
        startup_context.startup,
    ))
}

pub(crate) async fn resume_account_lease(
    config: &Config,
    live_snapshot: Option<AccountLeaseRuntimeSnapshot>,
) -> Result<AccountLeaseUpdatedNotification, JSONRPCErrorError> {
    let state_db = init_state_db(config).await?;

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

    let startup_context =
        read_account_lease_startup_context_with_state_db(config, state_db).await?;

    if let Some(live_snapshot) = live_snapshot {
        return Ok(account_lease_response_from_runtime_snapshot(
            &live_snapshot,
            Some(&startup_context.startup),
        )
        .into());
    }

    if startup_context.startup.startup_availability == AccountStartupAvailability::Unavailable {
        return Ok(empty_account_lease_response().into());
    }

    Ok(account_lease_response_from_startup_status(startup_context.startup).into())
}

pub(crate) async fn set_account_pool_default(
    config: &Config,
    live_snapshot: Option<AccountLeaseRuntimeSnapshot>,
    pool_id: String,
) -> Result<Option<AccountLeaseUpdatedNotification>, JSONRPCErrorError> {
    let state_db = init_state_db(config).await?;
    let outcome = set_local_default_pool(
        state_db.as_ref(),
        LocalDefaultPoolSetRequest {
            pool_id,
            configured_default_pool_id: configured_default_pool_id(config).map(ToOwned::to_owned),
        },
    )
    .await
    .map_err(local_default_pool_set_error)?;
    if !outcome.state_changed {
        return Ok(None);
    }

    let startup_context =
        read_account_lease_startup_context_with_state_db(config, state_db).await?;
    Ok(Some(account_lease_notification_after_startup_mutation(
        live_snapshot,
        startup_context.startup,
    )))
}

pub(crate) async fn clear_account_pool_default(
    config: &Config,
    live_snapshot: Option<AccountLeaseRuntimeSnapshot>,
) -> Result<Option<AccountLeaseUpdatedNotification>, JSONRPCErrorError> {
    let state_db = init_state_db(config).await?;
    let outcome = clear_local_default_pool(
        state_db.as_ref(),
        LocalDefaultPoolClearRequest {
            configured_default_pool_id: configured_default_pool_id(config).map(ToOwned::to_owned),
        },
    )
    .await
    .context("clear local default account pool")
    .map_err(internal_error)?;
    if !outcome.state_changed {
        return Ok(None);
    }

    let startup_context =
        read_account_lease_startup_context_with_state_db(config, state_db).await?;
    Ok(Some(account_lease_notification_after_startup_mutation(
        live_snapshot,
        startup_context.startup,
    )))
}

pub(crate) async fn suppress_account_lease_on_logout(
    config: &Config,
) -> Result<Option<AccountLeaseUpdatedNotification>, JSONRPCErrorError> {
    let state_db = init_state_db(config).await?;

    let selection = state_db
        .read_account_startup_selection()
        .await
        .context("read account startup selection")
        .map_err(internal_error)?;
    let startup_context =
        read_account_lease_startup_context_with_state_db(config, Arc::clone(&state_db)).await?;
    let has_startup_selection = selection.default_pool_id.is_some()
        || selection.preferred_account_id.is_some()
        || selection.suppressed;
    if !startup_context.pooled_applicable && !has_startup_selection {
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

    let startup_context =
        read_account_lease_startup_context_with_state_db(config, state_db.clone()).await?;
    if startup_context.pooled_applicable
        || startup_context.startup.startup_availability != AccountStartupAvailability::Unavailable
    {
        return Ok(Some(
            account_lease_response_from_startup_status(startup_context.startup).into(),
        ));
    }

    let preview = state_db
        .preview_account_startup_selection(configured_default_pool_id(config))
        .await
        .context("preview suppressed account startup selection")
        .map_err(internal_error)?;

    Ok(Some(account_lease_response_from_preview(preview).into()))
}

fn account_lease_notification_after_startup_mutation(
    live_snapshot: Option<AccountLeaseRuntimeSnapshot>,
    startup: AccountStartupStatus,
) -> AccountLeaseUpdatedNotification {
    if let Some(live_snapshot) = live_snapshot {
        return account_lease_response_from_runtime_snapshot(&live_snapshot, Some(&startup)).into();
    }

    if startup.startup_availability == AccountStartupAvailability::Unavailable {
        return empty_account_lease_response().into();
    }

    account_lease_response_from_startup_status(startup).into()
}

async fn init_state_db(config: &Config) -> Result<Arc<StateRuntime>, JSONRPCErrorError> {
    StateRuntime::init(config.sqlite_home.clone(), config.model_provider_id.clone())
        .await
        .context("initialize account lease state db")
        .map_err(internal_error)
}

async fn read_account_lease_startup_context(
    config: &Config,
) -> Result<SharedStartupStatus, JSONRPCErrorError> {
    let state_db = init_state_db(config).await?;
    read_account_lease_startup_context_with_state_db(config, state_db).await
}

async fn read_account_lease_startup_context_with_state_db(
    config: &Config,
    state_db: Arc<StateRuntime>,
) -> Result<SharedStartupStatus, JSONRPCErrorError> {
    read_account_lease_startup_context_inner(config, state_db).await
}

async fn read_account_lease_startup_context_for_runtime_admission(
    config: &Config,
) -> Result<Option<SharedStartupStatus>, JSONRPCErrorError> {
    let state_db = init_state_db(config).await?;
    let shared_status = read_account_lease_startup_context_inner(config, state_db).await?;
    if !shared_status.pooled_applicable {
        return Ok(None);
    }

    Ok(Some(shared_status))
}

async fn read_account_lease_startup_context_inner(
    config: &Config,
    state_db: Arc<StateRuntime>,
) -> Result<SharedStartupStatus, JSONRPCErrorError> {
    let lease_ttl_secs = config
        .accounts
        .as_ref()
        .and_then(|accounts| accounts.lease_ttl_secs)
        .unwrap_or(300);
    let backend = LocalAccountPoolBackend::new(
        Arc::clone(&state_db),
        Duration::seconds(lease_ttl_secs as i64),
    );
    let shared_status =
        read_shared_startup_status(&backend, configured_default_pool_id(config), None)
            .await
            .context("read shared account startup status")
            .map_err(internal_error)?;
    Ok(shared_status)
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
        effective_pool_resolution_source: None,
        configured_default_pool_id: None,
        persisted_default_pool_id: None,
        startup: unavailable_snapshot(),
    }
}

fn account_lease_response_from_startup_status(
    startup: AccountStartupStatus,
) -> AccountLeaseReadResponse {
    let startup_snapshot = snapshot_from_startup_status(&startup);
    let preview = startup.preview;
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
        effective_pool_resolution_source: Some(
            effective_pool_resolution_source_to_wire_string(
                startup.effective_pool_resolution_source,
            )
            .to_string(),
        ),
        configured_default_pool_id: startup.configured_default_pool_id,
        persisted_default_pool_id: startup.persisted_default_pool_id,
        startup: startup_snapshot,
    }
}

fn account_lease_response_from_runtime_snapshot(
    live_snapshot: &AccountLeaseRuntimeSnapshot,
    startup: Option<&AccountStartupStatus>,
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
        effective_pool_resolution_source: startup.map(|startup| {
            effective_pool_resolution_source_to_wire_string(
                startup.effective_pool_resolution_source,
            )
            .to_string()
        }),
        configured_default_pool_id: startup
            .and_then(|startup| startup.configured_default_pool_id.clone()),
        persisted_default_pool_id: startup
            .and_then(|startup| startup.persisted_default_pool_id.clone()),
        startup: startup.map_or_else(unavailable_snapshot, snapshot_from_startup_status),
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
        effective_pool_resolution_source: None,
        configured_default_pool_id: None,
        persisted_default_pool_id: None,
        startup: unavailable_snapshot(),
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

fn local_default_pool_set_error(err: anyhow::Error) -> JSONRPCErrorError {
    if matches!(
        err.downcast_ref::<LocalDefaultPoolSetError>(),
        Some(LocalDefaultPoolSetError::PoolNotVisible { .. })
    ) {
        return JSONRPCErrorError {
            code: INVALID_PARAMS_ERROR_CODE,
            message: err.to_string(),
            data: None,
        };
    }

    internal_error(err.context("set local default account pool"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_core::AccountLeaseRuntimeReason;
    use codex_state::EffectivePoolResolutionSource;
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;

    #[test]
    fn local_default_pool_set_error_does_not_map_untyped_message_to_invalid_params() {
        let error = local_default_pool_set_error(anyhow::anyhow!(
            "database failed while checking whether pool missing-pool is not visible in local startup inventory"
        ));

        assert_eq!(error.code, INTERNAL_ERROR_CODE);
    }

    #[test]
    fn runtime_response_keeps_live_fields_when_startup_suppression_is_cleared() {
        let live_snapshot = AccountLeaseRuntimeSnapshot {
            active: false,
            suppressed: true,
            account_id: None,
            pool_id: Some("legacy-default".to_string()),
            lease_id: None,
            lease_epoch: None,
            runtime_generation: None,
            lease_acquired_at: None,
            health_state: None,
            switch_reason: None,
            suppression_reason: Some(AccountLeaseRuntimeReason::StartupSuppressed),
            transport_reset_generation: None,
            last_remote_context_reset_turn_id: None,
            min_switch_interval_secs: None,
            proactive_switch_pending: None,
            proactive_switch_suppressed: None,
            proactive_switch_allowed_at: None,
            next_eligible_at: None,
        };
        let startup = AccountStartupStatus {
            preview: AccountStartupSelectionPreview {
                effective_pool_id: Some("legacy-default".to_string()),
                preferred_account_id: None,
                suppressed: false,
                predicted_account_id: Some("acct-1".to_string()),
                eligibility: AccountStartupEligibility::AutomaticAccountSelected,
            },
            configured_default_pool_id: Some("legacy-default".to_string()),
            persisted_default_pool_id: None,
            effective_pool_resolution_source: EffectivePoolResolutionSource::ConfigDefault,
            startup_availability: codex_state::AccountStartupAvailability::Available,
            startup_resolution_issue: None,
            candidate_pools: Vec::new(),
        };

        let response = account_lease_response_from_runtime_snapshot(&live_snapshot, Some(&startup));

        assert_eq!(response.suppressed, true);
        assert_eq!(
            response.suppression_reason.as_deref(),
            Some("durablySuppressed")
        );
        assert_eq!(response.account_id, None);
        assert_eq!(response.pool_id.as_deref(), Some("legacy-default"));
        assert_eq!(
            response.effective_pool_resolution_source.as_deref(),
            Some("configDefault")
        );
        assert_eq!(
            response.startup.effective_pool_id.as_deref(),
            Some("legacy-default")
        );
        assert_eq!(
            response.startup.selection_eligibility,
            "automaticAccountSelected"
        );
    }

    #[tokio::test]
    async fn runtime_update_notification_keeps_live_fields_when_startup_suppression_is_cleared()
    -> anyhow::Result<()> {
        let codex_home = tempfile::tempdir()?;
        let mut config =
            codex_core::config::Config::load_default_with_cli_overrides_for_codex_home(
                codex_home.path().to_path_buf(),
                Vec::new(),
            )?;
        config.accounts = Some(codex_config::types::AccountsConfigToml {
            backend: None,
            default_pool: Some("pool-main".to_string()),
            proactive_switch_threshold_percent: None,
            lease_ttl_secs: None,
            heartbeat_interval_secs: None,
            min_switch_interval_secs: None,
            allocation_mode: None,
            pools: Some(HashMap::from([(
                "pool-main".to_string(),
                codex_config::types::AccountPoolDefinitionToml {
                    allow_context_reuse: None,
                    account_kinds: None,
                },
            )])),
        });
        let state_db = init_state_db(&config).await.map_err(|err| {
            anyhow::anyhow!(
                "failed to initialize test account lease state db: {}",
                err.message
            )
        })?;
        state_db
            .upsert_account_registry_entry(codex_state::AccountRegistryEntryUpdate {
                account_id: "acct-1".to_string(),
                pool_id: "pool-main".to_string(),
                position: 0,
                account_kind: "chatgpt".to_string(),
                backend_family: "local".to_string(),
                workspace_id: None,
                enabled: true,
                healthy: true,
            })
            .await?;
        state_db
            .write_account_startup_selection(AccountStartupSelectionUpdate {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-1".to_string()),
                suppressed: false,
            })
            .await?;
        let live_snapshot = AccountLeaseRuntimeSnapshot {
            active: false,
            suppressed: true,
            account_id: None,
            pool_id: None,
            lease_id: None,
            lease_epoch: None,
            runtime_generation: None,
            lease_acquired_at: None,
            health_state: None,
            switch_reason: None,
            suppression_reason: Some(AccountLeaseRuntimeReason::StartupSuppressed),
            transport_reset_generation: None,
            last_remote_context_reset_turn_id: None,
            min_switch_interval_secs: None,
            proactive_switch_pending: None,
            proactive_switch_suppressed: None,
            proactive_switch_allowed_at: None,
            next_eligible_at: None,
        };

        let notification =
            account_lease_updated_notification_from_runtime_snapshot(&config, &live_snapshot)
                .await
                .map_err(|err| anyhow::anyhow!(err.message))?;

        assert_eq!(notification.suppressed, true);
        assert_eq!(notification.account_id, None);
        assert_eq!(notification.pool_id, None);
        assert_eq!(
            notification.startup.effective_pool_id.as_deref(),
            Some("pool-main")
        );
        assert_eq!(
            notification.startup.selection_eligibility,
            "preferredAccountSelected"
        );
        Ok(())
    }
}
