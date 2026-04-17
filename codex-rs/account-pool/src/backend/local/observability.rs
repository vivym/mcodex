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
                states: request.states,
                account_kinds: request.account_kinds,
            })
            .await
    }

    async fn list_events(
        &self,
        request: AccountPoolEventsListRequest,
    ) -> anyhow::Result<AccountPoolEventsPage> {
        self.runtime
            .list_account_pool_events(codex_state::AccountPoolEventsListQuery {
                pool_id: request.pool_id,
                account_id: request.account_id,
                types: request.types,
                cursor: request.cursor,
                limit: request.limit,
            })
            .await
    }

    async fn read_diagnostics(
        &self,
        request: AccountPoolDiagnosticsReadRequest,
    ) -> anyhow::Result<AccountPoolDiagnostics> {
        self.runtime
            .read_account_pool_diagnostics(&request.pool_id)
            .await
    }
}
