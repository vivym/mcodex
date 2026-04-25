use anyhow::Result;
use anyhow::bail;

use crate::backend::AccountPoolBackend;
use crate::quota::SelectionAction;
use crate::quota_selection::build_selection_plan;
use crate::types::SelectionRequest;
use crate::types::SelectionResult;

/// Selects the startup account from the available pool.
pub fn select_startup_account(
    pool: &impl AccountPoolBackend,
    request: SelectionRequest,
) -> Result<SelectionResult> {
    let accounts = pool.accounts();
    let Some(first) = accounts.first() else {
        bail!("no accounts available");
    };

    if accounts.iter().any(|account| account.kind != first.kind) {
        bail!("mixed account kinds are not supported");
    }

    let mut selector = build_selection_plan(request);
    for account in accounts.iter().enumerate().map(|(index, account)| {
        let mut account = account.clone();
        if account.pool_position == 0 {
            account.pool_position = index;
        }
        account
    }) {
        selector = selector.with_candidate(account);
    }

    match selector.run().terminal_action {
        SelectionAction::Select(account_id) | SelectionAction::Probe(account_id) => {
            Ok(SelectionResult { account_id })
        }
        SelectionAction::StayOnCurrent => Ok(SelectionResult {
            account_id: first.account_id.clone(),
        }),
        SelectionAction::NoCandidate => bail!("no candidate available"),
    }
}
