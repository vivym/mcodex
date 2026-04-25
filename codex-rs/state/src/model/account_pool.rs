use anyhow::Result;
use chrono::DateTime;
use chrono::Utc;
use std::fmt;

/// Persisted lease for an account in a local pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountLeaseRecord {
    pub lease_id: String,
    pub pool_id: String,
    pub account_id: String,
    pub holder_instance_id: String,
    pub lease_epoch: i64,
    pub acquired_at: DateTime<Utc>,
    pub renewed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub released_at: Option<DateTime<Utc>>,
}

impl AccountLeaseRecord {
    pub fn lease_key(&self) -> LeaseKey {
        LeaseKey {
            lease_id: self.lease_id.clone(),
            account_id: self.account_id.clone(),
            lease_epoch: self.lease_epoch,
        }
    }
}

/// Stable identity for renewing an existing lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseKey {
    pub lease_id: String,
    pub account_id: String,
    pub lease_epoch: i64,
}

/// Result of attempting to renew an existing lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseRenewal {
    Renewed(AccountLeaseRecord),
    Missing,
}

/// Persistent health status recorded for a pooled account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolHealthState {
    pub account_id: String,
    pub pool_id: String,
    pub health_state: AccountHealthState,
    pub last_health_event_sequence: i64,
    pub last_health_event_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Read-only diagnostics for a local account pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolDiagnostic {
    pub pool_id: String,
    pub accounts: Vec<AccountPoolAccountDiagnostic>,
    pub next_eligible_at: Option<DateTime<Utc>>,
}

/// Read-only diagnostics for one account in a local account pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolAccountDiagnostic {
    pub account_id: String,
    pub pool_id: String,
    pub source: Option<AccountSource>,
    pub enabled: bool,
    pub healthy: bool,
    pub active_lease: Option<AccountLeaseRecord>,
    pub health_state: Option<AccountHealthState>,
    pub eligibility: AccountStartupEligibility,
    pub next_eligible_at: Option<DateTime<Utc>>,
}

/// Monotonic health event recorded against an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountHealthEvent {
    pub account_id: String,
    pub pool_id: String,
    pub health_state: AccountHealthState,
    pub sequence_number: i64,
    pub observed_at: DateTime<Utc>,
}

/// Coarse account health stored in local pooled state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountHealthState {
    Healthy,
    RateLimited,
    Unauthorized,
}

impl AccountHealthState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::RateLimited => "rate_limited",
            Self::Unauthorized => "unauthorized",
        }
    }
}

impl TryFrom<&str> for AccountHealthState {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        match value {
            "healthy" => Ok(Self::Healthy),
            "rate_limited" => Ok(Self::RateLimited),
            "unauthorized" => Ok(Self::Unauthorized),
            other => Err(anyhow::anyhow!("unknown account health state: {other}")),
        }
    }
}

/// Imported default account from legacy single-account auth state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyAccountImport {
    pub account_id: String,
}

/// Startup selection state persisted across launches.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccountStartupSelectionState {
    pub default_pool_id: Option<String>,
    pub preferred_account_id: Option<String>,
    pub suppressed: bool,
}

/// Preview of what a fresh runtime would select from persisted startup state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStartupSelectionPreview {
    pub effective_pool_id: Option<String>,
    pub preferred_account_id: Option<String>,
    pub suppressed: bool,
    pub predicted_account_id: Option<String>,
    pub eligibility: AccountStartupEligibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectivePoolResolutionSource {
    Override,
    ConfigDefault,
    PersistedSelection,
    SingleVisiblePool,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStartupAvailability {
    Available,
    Suppressed,
    MultiplePoolsRequireDefault,
    InvalidExplicitDefault,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStartupResolutionIssueKind {
    MultiplePoolsRequireDefault,
    OverridePoolUnavailable,
    ConfigDefaultPoolUnavailable,
    PersistedDefaultPoolUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStartupResolutionIssueSource {
    Override,
    ConfigDefault,
    PersistedSelection,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStartupCandidatePool {
    pub pool_id: String,
    pub display_name: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStartupResolutionIssue {
    pub kind: AccountStartupResolutionIssueKind,
    pub source: AccountStartupResolutionIssueSource,
    pub pool_id: Option<String>,
    pub candidate_pool_count: Option<usize>,
    pub candidate_pools: Option<Vec<AccountStartupCandidatePool>>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStartupStatus {
    pub preview: AccountStartupSelectionPreview,
    pub configured_default_pool_id: Option<String>,
    pub persisted_default_pool_id: Option<String>,
    pub effective_pool_resolution_source: EffectivePoolResolutionSource,
    pub startup_availability: AccountStartupAvailability,
    pub startup_resolution_issue: Option<AccountStartupResolutionIssue>,
    pub candidate_pools: Vec<AccountStartupCandidatePool>,
}

/// Eligibility result for fresh-runtime startup selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountStartupEligibility {
    Suppressed,
    MissingPool,
    PreferredAccountSelected,
    AutomaticAccountSelected,
    PreferredAccountMissing,
    PreferredAccountInOtherPool { actual_pool_id: String },
    PreferredAccountDisabled,
    PreferredAccountUnhealthy,
    PreferredAccountBusy,
    NoEligibleAccount,
}

/// Provenance recorded for an account in the local registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountSource {
    Migrated,
}

impl AccountSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Migrated => "migrated",
        }
    }
}

impl TryFrom<&str> for AccountSource {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        match value {
            "migrated" => Ok(Self::Migrated),
            other => Err(anyhow::anyhow!("unknown account source: {other}")),
        }
    }
}

/// Persisted pool membership for a known account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolMembership {
    pub account_id: String,
    pub pool_id: String,
    pub source: Option<AccountSource>,
    pub enabled: bool,
    pub healthy: bool,
}

/// Persisted registry row for one pooled account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredAccountRecord {
    pub account_id: String,
    pub backend_id: String,
    pub backend_family: String,
    pub workspace_id: Option<String>,
    pub backend_account_handle: String,
    pub account_kind: String,
    pub provider_fingerprint: String,
    pub display_name: Option<String>,
    pub source: Option<AccountSource>,
    pub enabled: bool,
    pub healthy: bool,
}

/// Membership assignment to persist for a registered account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredAccountMembership {
    pub pool_id: String,
    pub position: i64,
}

/// Full replacement update for one registered pooled account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredAccountUpsert {
    pub account_id: String,
    pub backend_id: String,
    pub backend_family: String,
    pub workspace_id: Option<String>,
    pub backend_account_handle: String,
    pub account_kind: String,
    pub provider_fingerprint: String,
    pub display_name: Option<String>,
    pub source: Option<AccountSource>,
    pub enabled: bool,
    pub healthy: bool,
    pub membership: Option<RegisteredAccountMembership>,
}

/// Crash-recovery journal entry for an in-flight registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAccountRegistration {
    pub idempotency_key: String,
    pub backend_id: String,
    pub provider_kind: String,
    pub target_pool_id: Option<String>,
    pub backend_account_handle: Option<String>,
    pub account_id: Option<String>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Creation params for a crash-recovery journal entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPendingAccountRegistration {
    pub idempotency_key: String,
    pub backend_id: String,
    pub provider_kind: String,
    pub target_pool_id: Option<String>,
    pub backend_account_handle: Option<String>,
    pub account_id: Option<String>,
}

/// State tracking one-time legacy compatibility import completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountCompatMigrationState {
    pub legacy_import_completed: bool,
}

/// Full replacement update for one stored account registry entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRegistryEntryUpdate {
    pub account_id: String,
    pub pool_id: String,
    pub position: i64,
    pub account_kind: String,
    pub backend_family: String,
    pub workspace_id: Option<String>,
    pub enabled: bool,
    pub healthy: bool,
}

/// Full replacement update for startup selection state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccountStartupSelectionUpdate {
    pub default_pool_id: Option<String>,
    pub preferred_account_id: Option<String>,
    pub suppressed: bool,
}

/// Lease acquisition failure reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountLeaseError {
    NoEligibleAccount,
    Storage(String),
}

impl fmt::Display for AccountLeaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoEligibleAccount => write!(f, "no eligible account is available"),
            Self::Storage(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for AccountLeaseError {}

pub(crate) fn datetime_to_epoch_seconds(value: DateTime<Utc>) -> i64 {
    value.timestamp()
}

pub(crate) fn datetime_to_epoch_nanos(value: DateTime<Utc>) -> i64 {
    value
        .timestamp_nanos_opt()
        .unwrap_or_else(|| panic!("timestamp out of range: {value}"))
}

pub(crate) fn epoch_seconds_to_datetime(value: i64) -> Result<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp(value, 0)
        .ok_or_else(|| anyhow::anyhow!("invalid unix timestamp: {value}"))
}

pub(crate) fn epoch_nanos_to_datetime(value: i64) -> Result<DateTime<Utc>> {
    Ok(DateTime::<Utc>::from_timestamp_nanos(value))
}
