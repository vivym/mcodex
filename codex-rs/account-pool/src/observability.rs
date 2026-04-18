use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use serde_json::Value;

mod conversions;

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
    pub states: Option<Vec<AccountOperationalState>>,
    pub account_kinds: Option<Vec<String>>,
}

/// Read request for pooled-account event history.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccountPoolEventsListRequest {
    pub pool_id: String,
    pub account_id: Option<String>,
    pub types: Option<Vec<AccountPoolEventType>>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

/// Read request for current pooled-account diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolDiagnosticsReadRequest {
    pub pool_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountOperationalState {
    Available,
    Leased,
    Paused,
    Draining,
    CoolingDown,
    NearExhausted,
    Exhausted,
    Error,
}

impl AccountOperationalState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Leased => "leased",
            Self::Paused => "paused",
            Self::Draining => "draining",
            Self::CoolingDown => "coolingDown",
            Self::NearExhausted => "nearExhausted",
            Self::Exhausted => "exhausted",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountPoolEventType {
    LeaseAcquired,
    LeaseRenewed,
    LeaseReleased,
    LeaseAcquireFailed,
    ProactiveSwitchSelected,
    ProactiveSwitchSuppressed,
    QuotaObserved,
    QuotaNearExhausted,
    QuotaExhausted,
    AccountPaused,
    AccountResumed,
    AccountDrainingStarted,
    AccountDrainingCleared,
    AuthFailed,
    CooldownStarted,
    CooldownCleared,
}

impl AccountPoolEventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LeaseAcquired => "leaseAcquired",
            Self::LeaseRenewed => "leaseRenewed",
            Self::LeaseReleased => "leaseReleased",
            Self::LeaseAcquireFailed => "leaseAcquireFailed",
            Self::ProactiveSwitchSelected => "proactiveSwitchSelected",
            Self::ProactiveSwitchSuppressed => "proactiveSwitchSuppressed",
            Self::QuotaObserved => "quotaObserved",
            Self::QuotaNearExhausted => "quotaNearExhausted",
            Self::QuotaExhausted => "quotaExhausted",
            Self::AccountPaused => "accountPaused",
            Self::AccountResumed => "accountResumed",
            Self::AccountDrainingStarted => "accountDrainingStarted",
            Self::AccountDrainingCleared => "accountDrainingCleared",
            Self::AuthFailed => "authFailed",
            Self::CooldownStarted => "cooldownStarted",
            Self::CooldownCleared => "cooldownCleared",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountPoolReasonCode {
    DurablySuppressed,
    MissingPool,
    PreferredAccountSelected,
    AutomaticAccountSelected,
    PreferredAccountMissing,
    PreferredAccountInOtherPool,
    PreferredAccountDisabled,
    PreferredAccountUnhealthy,
    PreferredAccountBusy,
    ManualPause,
    ManualDrain,
    QuotaNearExhausted,
    QuotaExhausted,
    AuthFailure,
    CooldownActive,
    MinimumSwitchInterval,
    NoEligibleAccount,
    LeaseHeldByAnotherInstance,
    NonReplayableTurn,
    Unknown,
}

impl AccountPoolReasonCode {
    fn from_wire_value(value: &str) -> Self {
        match value {
            "durablySuppressed" => Self::DurablySuppressed,
            "missingPool" => Self::MissingPool,
            "preferredAccountSelected" => Self::PreferredAccountSelected,
            "automaticAccountSelected" => Self::AutomaticAccountSelected,
            "preferredAccountMissing" => Self::PreferredAccountMissing,
            "preferredAccountInOtherPool" => Self::PreferredAccountInOtherPool,
            "preferredAccountDisabled" => Self::PreferredAccountDisabled,
            "preferredAccountUnhealthy" => Self::PreferredAccountUnhealthy,
            "preferredAccountBusy" => Self::PreferredAccountBusy,
            "manualPause" => Self::ManualPause,
            "manualDrain" => Self::ManualDrain,
            "quotaNearExhausted" => Self::QuotaNearExhausted,
            "quotaExhausted" => Self::QuotaExhausted,
            "authFailure" => Self::AuthFailure,
            "cooldownActive" => Self::CooldownActive,
            "minimumSwitchInterval" => Self::MinimumSwitchInterval,
            "noEligibleAccount" => Self::NoEligibleAccount,
            "leaseHeldByAnotherInstance" => Self::LeaseHeldByAnotherInstance,
            "nonReplayableTurn" => Self::NonReplayableTurn,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountPoolDiagnosticsStatus {
    Healthy,
    Degraded,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountPoolDiagnosticsSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolSnapshot {
    pub pool_id: String,
    pub summary: AccountPoolSummary,
    pub refreshed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolSummary {
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
pub struct AccountPoolAccount {
    pub account_id: String,
    pub backend_account_ref: Option<String>,
    pub account_kind: String,
    pub enabled: bool,
    pub health_state: Option<String>,
    pub operational_state: Option<AccountOperationalState>,
    pub allocatable: Option<bool>,
    pub status_reason_code: Option<AccountPoolReasonCode>,
    pub status_message: Option<String>,
    pub current_lease: Option<AccountPoolLease>,
    pub quota: Option<AccountPoolQuota>,
    pub selection: Option<AccountPoolSelection>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountPoolAccountsPage {
    pub data: Vec<AccountPoolAccount>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolLease {
    pub lease_id: String,
    pub lease_epoch: u64,
    pub holder_instance_id: String,
    pub acquired_at: DateTime<Utc>,
    pub renewed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountPoolQuota {
    pub remaining_percent: Option<f64>,
    pub resets_at: Option<DateTime<Utc>>,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolSelection {
    pub eligible: bool,
    pub next_eligible_at: Option<DateTime<Utc>>,
    pub preferred: bool,
    pub suppressed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountPoolEvent {
    pub event_id: String,
    pub occurred_at: DateTime<Utc>,
    pub pool_id: String,
    pub account_id: Option<String>,
    pub lease_id: Option<String>,
    pub holder_instance_id: Option<String>,
    pub event_type: AccountPoolEventType,
    pub reason_code: Option<AccountPoolReasonCode>,
    pub message: String,
    pub details_json: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountPoolEventsPage {
    pub data: Vec<AccountPoolEvent>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolDiagnostics {
    pub pool_id: String,
    pub generated_at: DateTime<Utc>,
    pub status: AccountPoolDiagnosticsStatus,
    pub issues: Vec<AccountPoolIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolIssue {
    pub severity: AccountPoolDiagnosticsSeverity,
    pub reason_code: AccountPoolReasonCode,
    pub message: String,
    pub account_id: Option<String>,
    pub holder_instance_id: Option<String>,
    pub next_relevant_at: Option<DateTime<Utc>>,
}

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
