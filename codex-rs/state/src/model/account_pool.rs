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

    pub(crate) fn is_healthy(self) -> bool {
        match self {
            Self::Healthy => true,
            Self::RateLimited | Self::Unauthorized => false,
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

pub(crate) fn epoch_seconds_to_datetime(value: i64) -> Result<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp(value, 0)
        .ok_or_else(|| anyhow::anyhow!("invalid unix timestamp: {value}"))
}
