#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PoolShowView {
    pub pool_id: String,
    pub refreshed_at: Option<String>,
    pub summary: PoolSummaryView,
    pub data: Vec<PoolAccountView>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PoolSummaryView {
    pub total_accounts: u32,
    pub active_leases: u32,
    pub available_accounts: Option<u32>,
    pub leased_accounts: Option<u32>,
    pub paused_accounts: Option<u32>,
    pub draining_accounts: Option<u32>,
    pub near_exhausted_accounts: Option<u32>,
    pub exhausted_accounts: Option<u32>,
    pub error_accounts: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PoolAccountView {
    pub account_id: String,
    pub backend_account_ref: Option<String>,
    pub account_kind: String,
    pub enabled: bool,
    pub health_state: Option<String>,
    pub operational_state: Option<String>,
    pub allocatable: Option<bool>,
    pub status_reason_code: Option<String>,
    pub status_message: Option<String>,
    pub current_lease: Option<PoolLeaseView>,
    pub quota: Option<PoolQuotaView>,
    pub selection: Option<PoolSelectionView>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PoolLeaseView {
    pub lease_id: String,
    pub lease_epoch: u64,
    pub holder_instance_id: String,
    pub acquired_at: String,
    pub renewed_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PoolQuotaView {
    pub remaining_percent: Option<f64>,
    pub resets_at: Option<String>,
    pub observed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PoolSelectionView {
    pub eligible: bool,
    pub next_eligible_at: Option<String>,
    pub preferred: bool,
    pub suppressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiagnosticsView {
    pub pool_id: String,
    pub generated_at: Option<String>,
    pub status: String,
    pub issues: Vec<DiagnosticsIssueView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiagnosticsIssueView {
    pub severity: String,
    pub reason_code: String,
    pub message: String,
    pub account_id: Option<String>,
    pub holder_instance_id: Option<String>,
    pub next_relevant_at: Option<String>,
}
