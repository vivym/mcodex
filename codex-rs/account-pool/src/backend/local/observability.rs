use crate::observability::AccountPoolAccountsListRequest;
use crate::observability::AccountPoolAccountsPage;
use crate::observability::AccountPoolDiagnostics;
use crate::observability::AccountPoolDiagnosticsReadRequest;
use crate::observability::AccountPoolEventsListRequest;
use crate::observability::AccountPoolEventsPage;
use crate::observability::AccountPoolObservabilityReader;
use crate::observability::AccountPoolReadRequest;
use crate::observability::AccountPoolSnapshot;
use async_trait::async_trait;

use super::LocalAccountPoolBackend;

#[async_trait]
impl AccountPoolObservabilityReader for LocalAccountPoolBackend {
    async fn read_pool(
        &self,
        request: AccountPoolReadRequest,
    ) -> anyhow::Result<AccountPoolSnapshot> {
        self.runtime
            .read_account_pool_snapshot(&request.pool_id)
            .await
            .and_then(AccountPoolSnapshot::try_from)
    }

    async fn list_accounts(
        &self,
        request: AccountPoolAccountsListRequest,
    ) -> anyhow::Result<AccountPoolAccountsPage> {
        self.runtime
            .list_account_pool_accounts(codex_state::AccountPoolAccountsListQuery {
                pool_id: request.pool_id,
                cursor: request.cursor,
                limit: request.limit,
                states: request.states.map(|states| {
                    states
                        .into_iter()
                        .map(|state| state.as_str().to_string())
                        .collect()
                }),
                account_kinds: request.account_kinds,
            })
            .await
            .and_then(AccountPoolAccountsPage::try_from)
    }

    async fn list_events(
        &self,
        request: AccountPoolEventsListRequest,
    ) -> anyhow::Result<AccountPoolEventsPage> {
        self.runtime
            .list_account_pool_events(codex_state::AccountPoolEventsListQuery {
                pool_id: request.pool_id,
                account_id: request.account_id,
                types: request.types.map(|types| {
                    types
                        .into_iter()
                        .map(|event_type| event_type.as_str().to_string())
                        .collect()
                }),
                cursor: request.cursor,
                limit: request.limit,
            })
            .await
            .and_then(AccountPoolEventsPage::try_from)
    }

    async fn read_diagnostics(
        &self,
        request: AccountPoolDiagnosticsReadRequest,
    ) -> anyhow::Result<AccountPoolDiagnostics> {
        self.runtime
            .read_account_pool_diagnostics(&request.pool_id)
            .await
            .and_then(AccountPoolDiagnostics::try_from)
    }
}
