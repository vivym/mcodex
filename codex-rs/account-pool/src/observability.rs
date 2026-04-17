use async_trait::async_trait;
use codex_state::AccountPoolAccountsPage as StateAccountPoolAccountsPage;
use codex_state::AccountPoolDiagnosticsRecord as StateAccountPoolDiagnostics;
use codex_state::AccountPoolEventsPage as StateAccountPoolEventsPage;
use codex_state::AccountPoolSnapshotRecord as StateAccountPoolSnapshot;

/// Read request for a pooled-account snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolReadRequest {
    pub pool_id: String,
}

/// Read request for pooled-account rows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccountPoolAccountsListRequest {
    pub pool_id: String,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub states: Option<Vec<String>>,
    pub account_kinds: Option<Vec<String>>,
}

/// Read request for pooled-account event history.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccountPoolEventsListRequest {
    pub pool_id: String,
    pub account_id: Option<String>,
    pub types: Option<Vec<String>>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

/// Read request for current pooled-account diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolDiagnosticsReadRequest {
    pub pool_id: String,
}

pub type AccountPoolSnapshot = StateAccountPoolSnapshot;
pub type AccountPoolAccountsPage = StateAccountPoolAccountsPage;
pub type AccountPoolEventsPage = StateAccountPoolEventsPage;
pub type AccountPoolDiagnostics = StateAccountPoolDiagnostics;

/// Backend-neutral pooled observability reads.
#[async_trait]
pub trait AccountPoolObservabilityReader: Send + Sync {
    async fn read_pool(
        &self,
        request: AccountPoolReadRequest,
    ) -> anyhow::Result<AccountPoolSnapshot>;

    async fn list_accounts(
        &self,
        request: AccountPoolAccountsListRequest,
    ) -> anyhow::Result<AccountPoolAccountsPage>;

    async fn list_events(
        &self,
        request: AccountPoolEventsListRequest,
    ) -> anyhow::Result<AccountPoolEventsPage>;

    async fn read_diagnostics(
        &self,
        request: AccountPoolDiagnosticsReadRequest,
    ) -> anyhow::Result<AccountPoolDiagnostics>;
}
