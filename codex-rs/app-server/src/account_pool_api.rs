use anyhow::Context;
use codex_account_pool as pool;
use codex_app_server_protocol::AccountPoolAccountsListParams;
use codex_app_server_protocol::AccountPoolAccountsListResponse;
use codex_app_server_protocol::AccountPoolBackendKind;
use codex_app_server_protocol::AccountPoolDiagnosticsReadParams;
use codex_app_server_protocol::AccountPoolDiagnosticsReadResponse;
use codex_app_server_protocol::AccountPoolEventsListParams;
use codex_app_server_protocol::AccountPoolEventsListResponse;
use codex_app_server_protocol::AccountPoolPolicyResponse;
use codex_app_server_protocol::AccountPoolReadParams;
use codex_app_server_protocol::AccountPoolReadResponse;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_config::types::AccountAllocationModeToml;
use codex_core::config::Config;
use codex_state::StateRuntime;
use pool::AccountPoolObservabilityReader;
use pool::LocalAccountPoolBackend;

use crate::error_code::INTERNAL_ERROR_CODE;
use crate::error_code::INVALID_PARAMS_ERROR_CODE;
use crate::error_code::NOT_FOUND_ERROR_CODE;

mod conversions;

use conversions::account_operational_state_to_pool_state;
use conversions::account_pool_account_response;
use conversions::account_pool_diagnostics_issue_response;
use conversions::account_pool_diagnostics_status_response;
use conversions::account_pool_event_response;
use conversions::account_pool_event_type_to_pool_event_type;
use conversions::account_pool_summary_response;

pub(crate) async fn read_account_pool(
    config: &Config,
    params: AccountPoolReadParams,
) -> Result<AccountPoolReadResponse, JSONRPCErrorError> {
    ensure_known_pool(config, &params.pool_id)?;

    let Some(reader) = maybe_local_observability_reader(config).await? else {
        return Err(pool_not_found(&params.pool_id));
    };

    let snapshot = reader
        .read_pool(pool::AccountPoolReadRequest {
            pool_id: params.pool_id.clone(),
        })
        .await
        .context("read account pool snapshot")
        .map_err(internal_error)?;

    Ok(AccountPoolReadResponse {
        pool_id: snapshot.pool_id,
        backend: AccountPoolBackendKind::Local,
        summary: account_pool_summary_response(snapshot.summary),
        policy: account_pool_policy_response(config, &params.pool_id),
        refreshed_at: snapshot.refreshed_at.timestamp(),
    })
}

pub(crate) async fn list_account_pool_accounts(
    config: &Config,
    params: AccountPoolAccountsListParams,
) -> Result<AccountPoolAccountsListResponse, JSONRPCErrorError> {
    ensure_known_pool(config, &params.pool_id)?;
    validate_account_cursor(params.cursor.as_deref())?;

    let Some(reader) = maybe_local_observability_reader(config).await? else {
        return Err(pool_not_found(&params.pool_id));
    };

    let page = reader
        .list_accounts(pool::AccountPoolAccountsListRequest {
            pool_id: params.pool_id,
            cursor: params.cursor,
            limit: params.limit,
            states: params.states.map(|states| {
                states
                    .into_iter()
                    .map(account_operational_state_to_pool_state)
                    .collect()
            }),
            account_kinds: params.account_kinds,
        })
        .await
        .context("list account pool accounts")
        .map_err(internal_error)?;

    Ok(AccountPoolAccountsListResponse {
        data: page
            .data
            .into_iter()
            .map(account_pool_account_response)
            .collect(),
        next_cursor: page.next_cursor,
    })
}

pub(crate) async fn list_account_pool_events(
    config: &Config,
    params: AccountPoolEventsListParams,
) -> Result<AccountPoolEventsListResponse, JSONRPCErrorError> {
    ensure_known_pool(config, &params.pool_id)?;
    validate_event_cursor(params.cursor.as_deref())?;

    let Some(reader) = maybe_local_observability_reader(config).await? else {
        return Err(pool_not_found(&params.pool_id));
    };

    let page = reader
        .list_events(pool::AccountPoolEventsListRequest {
            pool_id: params.pool_id,
            account_id: params.account_id,
            types: params.types.map(|types| {
                types
                    .into_iter()
                    .map(account_pool_event_type_to_pool_event_type)
                    .collect()
            }),
            cursor: params.cursor,
            limit: params.limit,
        })
        .await
        .context("list account pool events")
        .map_err(internal_error)?;

    Ok(AccountPoolEventsListResponse {
        data: page
            .data
            .into_iter()
            .map(account_pool_event_response)
            .collect(),
        next_cursor: page.next_cursor,
    })
}

pub(crate) async fn read_account_pool_diagnostics(
    config: &Config,
    params: AccountPoolDiagnosticsReadParams,
) -> Result<AccountPoolDiagnosticsReadResponse, JSONRPCErrorError> {
    ensure_known_pool(config, &params.pool_id)?;

    let Some(reader) = maybe_local_observability_reader(config).await? else {
        return Err(pool_not_found(&params.pool_id));
    };

    let diagnostics = reader
        .read_diagnostics(pool::AccountPoolDiagnosticsReadRequest {
            pool_id: params.pool_id,
        })
        .await
        .context("read account pool diagnostics")
        .map_err(internal_error)?;

    Ok(AccountPoolDiagnosticsReadResponse {
        pool_id: diagnostics.pool_id,
        generated_at: diagnostics.generated_at.timestamp(),
        status: account_pool_diagnostics_status_response(diagnostics.status),
        issues: diagnostics
            .issues
            .into_iter()
            .map(account_pool_diagnostics_issue_response)
            .collect(),
    })
}

async fn maybe_local_observability_reader(
    config: &Config,
) -> Result<Option<LocalAccountPoolBackend>, JSONRPCErrorError> {
    if config.accounts.is_none() {
        return Ok(None);
    }

    let runtime = StateRuntime::init(config.sqlite_home.clone(), config.model_provider_id.clone())
        .await
        .context("initialize account pool state db")
        .map_err(internal_error)?;
    Ok(Some(LocalAccountPoolBackend::new(
        runtime,
        configured_lease_ttl(config),
    )))
}

fn configured_lease_ttl(config: &Config) -> chrono::Duration {
    let lease_ttl_secs = config
        .accounts
        .as_ref()
        .and_then(|accounts| accounts.lease_ttl_secs)
        .unwrap_or(300);
    chrono::Duration::seconds(lease_ttl_secs as i64)
}

fn ensure_known_pool(config: &Config, pool_id: &str) -> Result<(), JSONRPCErrorError> {
    if pool_is_configured(config, pool_id) {
        Ok(())
    } else {
        Err(pool_not_found(pool_id))
    }
}

fn pool_is_configured(config: &Config, pool_id: &str) -> bool {
    let Some(accounts) = config.accounts.as_ref() else {
        return false;
    };

    accounts.default_pool.as_deref() == Some(pool_id)
        || accounts
            .pools
            .as_ref()
            .is_some_and(|pools| pools.contains_key(pool_id))
}

fn validate_account_cursor(cursor: Option<&str>) -> Result<(), JSONRPCErrorError> {
    validate_cursor_anchor(cursor, "account cursor")
}

fn validate_event_cursor(cursor: Option<&str>) -> Result<(), JSONRPCErrorError> {
    validate_cursor_anchor(cursor, "account pool event cursor")
}

fn validate_cursor_anchor(
    cursor: Option<&str>,
    cursor_kind: &str,
) -> Result<(), JSONRPCErrorError> {
    let Some(cursor) = cursor else {
        return Ok(());
    };
    let valid = match cursor.split_once(':') {
        Some((position, item_id)) => {
            !position.is_empty() && !item_id.is_empty() && position.parse::<i64>().is_ok()
        }
        None => false,
    };
    if valid {
        Ok(())
    } else {
        Err(invalid_params(format!("invalid {cursor_kind}")))
    }
}

fn account_pool_policy_response(config: &Config, pool_id: &str) -> AccountPoolPolicyResponse {
    let accounts = config.accounts.as_ref();
    let allocation_mode = match accounts.and_then(|accounts| accounts.allocation_mode) {
        Some(AccountAllocationModeToml::Exclusive) | None => "exclusive",
    };
    let allow_context_reuse = accounts
        .and_then(|accounts| accounts.pools.as_ref())
        .and_then(|pools| pools.get(pool_id))
        .and_then(|pool| pool.allow_context_reuse)
        .unwrap_or(true);

    AccountPoolPolicyResponse {
        allocation_mode: allocation_mode.to_string(),
        allow_context_reuse,
        proactive_switch_threshold_percent: accounts
            .and_then(|accounts| accounts.proactive_switch_threshold_percent),
        min_switch_interval_secs: accounts.and_then(|accounts| accounts.min_switch_interval_secs),
    }
}

fn internal_error(err: anyhow::Error) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: INTERNAL_ERROR_CODE,
        message: err.to_string(),
        data: None,
    }
}

fn invalid_params(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: INVALID_PARAMS_ERROR_CODE,
        message,
        data: None,
    }
}

fn pool_not_found(pool_id: &str) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: NOT_FOUND_ERROR_CODE,
        message: format!("account pool not found: {pool_id}"),
        data: None,
    }
}
