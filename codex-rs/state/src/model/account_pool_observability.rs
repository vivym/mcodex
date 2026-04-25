use chrono::DateTime;
use chrono::Utc;
use serde_json::Value;

/// Read model for one pooled-account snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolSnapshotRecord {
    pub pool_id: String,
    pub summary: AccountPoolSummaryRecord,
    pub refreshed_at: DateTime<Utc>,
}

/// Summary counts derived from current persisted account-pool facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolSummaryRecord {
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

/// Cursor query for listing surfaced pooled accounts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccountPoolAccountsListQuery {
    pub pool_id: String,
    pub account_id: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub states: Option<Vec<String>>,
    pub account_kinds: Option<Vec<String>>,
}

/// One surfaced pooled account row.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountPoolAccountRecord {
    pub account_id: String,
    pub backend_account_ref: Option<String>,
    pub account_kind: String,
    pub enabled: bool,
    pub health_state: Option<String>,
    pub operational_state: Option<String>,
    pub allocatable: Option<bool>,
    pub status_reason_code: Option<String>,
    pub status_message: Option<String>,
    pub current_lease: Option<AccountPoolLeaseRecord>,
    pub quota: Option<AccountPoolQuotaRecord>,
    pub quotas: Vec<AccountPoolQuotaFamilyRecord>,
    pub selection: Option<AccountPoolSelectionRecord>,
    pub updated_at: DateTime<Utc>,
}

/// Page of surfaced pooled-account rows.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountPoolAccountsPage {
    pub data: Vec<AccountPoolAccountRecord>,
    pub next_cursor: Option<String>,
}

/// Current lease facts attached to an account read row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolLeaseRecord {
    pub lease_id: String,
    pub lease_epoch: u64,
    pub holder_instance_id: String,
    pub acquired_at: DateTime<Utc>,
    pub renewed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Quota facts surfaced on an account row when available.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountPoolQuotaRecord {
    pub remaining_percent: Option<f64>,
    pub resets_at: Option<DateTime<Utc>>,
    pub observed_at: DateTime<Utc>,
}

/// Quota facts for one durable limit family surfaced on an account row.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountPoolQuotaFamilyRecord {
    pub limit_id: String,
    pub primary: AccountPoolQuotaWindowRecord,
    pub secondary: AccountPoolQuotaWindowRecord,
    pub exhausted_windows: String,
    pub predicted_blocked_until: Option<DateTime<Utc>>,
    pub next_probe_after: Option<DateTime<Utc>>,
    pub observed_at: DateTime<Utc>,
}

/// Window-level quota facts for a quota family.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountPoolQuotaWindowRecord {
    pub used_percent: Option<f64>,
    pub resets_at: Option<DateTime<Utc>>,
}

/// Startup-selection facts surfaced on an account row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolSelectionRecord {
    pub eligible: bool,
    pub next_eligible_at: Option<DateTime<Utc>>,
    pub preferred: bool,
    pub suppressed: bool,
}

/// Cursor query for listing stored account-pool events.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccountPoolEventsListQuery {
    pub pool_id: String,
    pub account_id: Option<String>,
    pub types: Option<Vec<String>>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

/// One stored pooled-account event row.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountPoolEventRecord {
    pub event_id: String,
    pub occurred_at: DateTime<Utc>,
    pub pool_id: String,
    pub account_id: Option<String>,
    pub lease_id: Option<String>,
    pub holder_instance_id: Option<String>,
    pub event_type: String,
    pub reason_code: Option<String>,
    pub message: String,
    pub details_json: Option<Value>,
}

/// Page of stored account-pool events.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountPoolEventsPage {
    pub data: Vec<AccountPoolEventRecord>,
    pub next_cursor: Option<String>,
}

/// Ordering anchor for descending event pagination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolEventsCursor {
    pub occurred_at: i64,
    pub event_id: String,
}

/// Read model for current pool diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolDiagnosticsRecord {
    pub pool_id: String,
    pub generated_at: DateTime<Utc>,
    pub status: String,
    pub issues: Vec<AccountPoolIssueRecord>,
}

/// One operator-facing diagnostics issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolIssueRecord {
    pub severity: String,
    pub reason_code: String,
    pub message: String,
    pub account_id: Option<String>,
    pub holder_instance_id: Option<String>,
    pub next_relevant_at: Option<DateTime<Utc>>,
}
