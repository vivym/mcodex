use codex_account_pool::AccountPoolConfig;
use codex_account_pool::LocalAccountPoolBackend;
use codex_account_pool::SharedStartupStatus;
use codex_account_pool::read_shared_startup_status;
use codex_core::config::Config;
use codex_state::AccountPoolDiagnostic;
use codex_state::AccountStartupResolutionIssueSource;
use codex_state::StateRuntime;
use std::collections::HashSet;
use std::sync::Arc;

use crate::accounts::observability::read_status_pool_observability;
use crate::accounts::observability_types::StatusPoolObservabilityView;

pub(crate) struct AccountsCurrentDiagnostic {
    pub account_pool_override_id: Option<String>,
    pub startup: SharedStartupStatus,
}

pub(crate) struct AccountsStatusDiagnostic {
    pub account_pool_override_id: Option<String>,
    pub configured_pool_count: usize,
    pub registered_pool_count: usize,
    pub startup: SharedStartupStatus,
    pub pool: Option<AccountPoolDiagnostic>,
    pub pool_observability: Option<StatusPoolObservabilityView>,
}

pub(crate) async fn read_current_diagnostic(
    runtime: &Arc<StateRuntime>,
    config: &Config,
    account_pool_override_id: Option<&str>,
) -> anyhow::Result<AccountsCurrentDiagnostic> {
    Ok(AccountsCurrentDiagnostic {
        account_pool_override_id: account_pool_override_id.map(ToOwned::to_owned),
        startup: read_accounts_startup_status(runtime, config, account_pool_override_id).await?,
    })
}

pub(crate) async fn read_status_diagnostic(
    runtime: &Arc<StateRuntime>,
    config: &Config,
    account_pool_override_id: Option<&str>,
) -> anyhow::Result<AccountsStatusDiagnostic> {
    let current = read_current_diagnostic(runtime, config, account_pool_override_id).await?;
    let status_pool_id = current
        .startup
        .startup
        .preview
        .effective_pool_id
        .clone()
        .or_else(|| {
            current
                .startup
                .startup
                .startup_resolution_issue
                .as_ref()
                .filter(|issue| issue.source == AccountStartupResolutionIssueSource::Override)
                .and_then(|issue| issue.pool_id.clone())
        });
    let pool = match current.startup.startup.preview.effective_pool_id.as_deref() {
        Some(pool_id) => Some(
            runtime
                .read_account_pool_diagnostic(
                    pool_id,
                    current
                        .startup
                        .startup
                        .preview
                        .preferred_account_id
                        .as_deref(),
                )
                .await?,
        ),
        None => None,
    };
    let pool_observability =
        read_status_pool_observability(runtime, config, status_pool_id.as_deref()).await;

    Ok(AccountsStatusDiagnostic {
        account_pool_override_id: current.account_pool_override_id,
        configured_pool_count: configured_pool_count(config),
        registered_pool_count: registered_pool_count(runtime).await?,
        startup: current.startup,
        pool,
        pool_observability,
    })
}

pub(crate) async fn read_accounts_startup_status(
    runtime: &Arc<StateRuntime>,
    config: &Config,
    account_pool_override_id: Option<&str>,
) -> anyhow::Result<SharedStartupStatus> {
    let lease_ttl_secs = config
        .accounts
        .as_ref()
        .and_then(|accounts| accounts.lease_ttl_secs)
        .unwrap_or(AccountPoolConfig::default().lease_ttl_secs);
    let lease_ttl = AccountPoolConfig {
        lease_ttl_secs,
        ..AccountPoolConfig::default()
    }
    .lease_ttl_duration();
    let backend = LocalAccountPoolBackend::new(Arc::clone(runtime), lease_ttl);
    read_shared_startup_status(
        &backend,
        configured_default_pool_id(config),
        account_pool_override_id,
    )
    .await
}

fn configured_default_pool_id(config: &Config) -> Option<&str> {
    config
        .accounts
        .as_ref()
        .and_then(|accounts| accounts.default_pool.as_deref())
}

fn configured_pool_count(config: &Config) -> usize {
    config
        .accounts
        .as_ref()
        .and_then(|accounts| accounts.pools.as_ref())
        .map_or(0, std::collections::HashMap::len)
}

async fn registered_pool_count(runtime: &StateRuntime) -> anyhow::Result<usize> {
    Ok(runtime
        .list_account_pool_memberships(None)
        .await?
        .into_iter()
        .map(|membership| membership.pool_id)
        .collect::<HashSet<_>>()
        .len())
}
