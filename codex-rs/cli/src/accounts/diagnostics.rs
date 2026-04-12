use codex_core::config::Config;
use codex_state::AccountPoolDiagnostic;
use codex_state::AccountStartupSelectionPreview;
use codex_state::StateRuntime;

pub(crate) struct AccountsCurrentDiagnostic {
    pub account_pool_override_id: Option<String>,
    pub preview: AccountStartupSelectionPreview,
}

pub(crate) struct AccountsStatusDiagnostic {
    pub account_pool_override_id: Option<String>,
    pub configured_pool_count: usize,
    pub preview: AccountStartupSelectionPreview,
    pub pool: Option<AccountPoolDiagnostic>,
}

pub(crate) async fn read_current_diagnostic(
    runtime: &StateRuntime,
    config: &Config,
    account_pool_override_id: Option<&str>,
) -> anyhow::Result<AccountsCurrentDiagnostic> {
    Ok(AccountsCurrentDiagnostic {
        account_pool_override_id: account_pool_override_id.map(ToOwned::to_owned),
        preview: runtime
            .preview_account_startup_selection(configured_default_pool_id(
                config,
                account_pool_override_id,
            ))
            .await?,
    })
}

pub(crate) async fn read_status_diagnostic(
    runtime: &StateRuntime,
    config: &Config,
    account_pool_override_id: Option<&str>,
) -> anyhow::Result<AccountsStatusDiagnostic> {
    let current = read_current_diagnostic(runtime, config, account_pool_override_id).await?;
    let pool = match current.preview.effective_pool_id.as_deref() {
        Some(pool_id) => Some(
            runtime
                .read_account_pool_diagnostic(
                    pool_id,
                    current.preview.preferred_account_id.as_deref(),
                )
                .await?,
        ),
        None => None,
    };

    Ok(AccountsStatusDiagnostic {
        account_pool_override_id: current.account_pool_override_id,
        configured_pool_count: configured_pool_count(config),
        preview: current.preview,
        pool,
    })
}

fn configured_default_pool_id<'a>(
    config: &'a Config,
    account_pool_override_id: Option<&'a str>,
) -> Option<&'a str> {
    account_pool_override_id.or_else(|| {
        config
            .accounts
            .as_ref()
            .and_then(|accounts| accounts.default_pool.as_deref())
    })
}

fn configured_pool_count(config: &Config) -> usize {
    config
        .accounts
        .as_ref()
        .and_then(|accounts| accounts.pools.as_ref())
        .map_or(0, std::collections::HashMap::len)
}
