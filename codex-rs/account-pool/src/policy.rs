use anyhow::Result;
use anyhow::bail;

use crate::backend::AccountPoolBackend;
use crate::types::SelectionRequest;
use crate::types::SelectionResult;

/// Selects the startup account from the available pool.
pub fn select_startup_account(
    pool: &impl AccountPoolBackend,
    _request: SelectionRequest,
) -> Result<SelectionResult> {
    let accounts = pool.accounts();
    let Some(first) = accounts.first() else {
        bail!("no accounts available");
    };

    if accounts.iter().any(|account| account.kind != first.kind) {
        bail!("mixed account kinds are not supported");
    }

    let selected = accounts
        .iter()
        .find(|account| account.healthy)
        .unwrap_or(first);

    Ok(SelectionResult {
        account_id: selected.account_id.clone(),
    })
}
