use anyhow::Result;
use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use codex_state::AccountLeaseRecord;
use codex_state::LeaseKey;

/// Request for choosing the startup account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionRequest {
    pub now: Option<DateTime<Utc>>,
    pub pool_id: Option<String>,
}

/// Result of choosing the startup account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionResult {
    pub account_id: String,
}

/// Kind of account available to the startup selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountKind {
    ChatGpt,
    ManualOnly,
}

/// Minimal account record used by the policy test scaffold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRecord {
    pub account_id: String,
    pub healthy: bool,
    pub kind: AccountKind,
}

/// Local manager configuration for lease lifecycle and switch policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolConfig {
    pub default_pool_id: Option<String>,
    pub proactive_switch_threshold_percent: u8,
    pub lease_ttl_secs: u64,
    pub heartbeat_interval_secs: u64,
}

impl Default for AccountPoolConfig {
    fn default() -> Self {
        Self {
            default_pool_id: None,
            proactive_switch_threshold_percent: 85,
            lease_ttl_secs: 300,
            heartbeat_interval_secs: 60,
        }
    }
}

impl AccountPoolConfig {
    pub fn validate(&self) -> Result<()> {
        if self.lease_ttl_secs <= self.heartbeat_interval_secs {
            anyhow::bail!(
                "accounts.lease_ttl_secs must be greater than accounts.heartbeat_interval_secs"
            );
        }

        if self.derived_pre_turn_safety_margin().num_seconds() >= self.lease_ttl_secs as i64 {
            anyhow::bail!(
                "derived account lease safety margin must be less than accounts.lease_ttl_secs"
            );
        }

        Ok(())
    }

    pub fn derived_pre_turn_safety_margin(&self) -> Duration {
        let safety_margin_secs = self.heartbeat_interval_secs.saturating_mul(2);
        Duration::seconds(safety_margin_secs as i64)
    }
}

/// Active lease bound to the current runtime instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeasedAccount {
    record: AccountLeaseRecord,
}

impl LeasedAccount {
    pub fn new(record: AccountLeaseRecord) -> Self {
        Self { record }
    }

    pub fn key(&self) -> LeaseKey {
        self.record.lease_key()
    }

    pub fn account_id(&self) -> &str {
        &self.record.account_id
    }

    pub fn pool_id(&self) -> &str {
        &self.record.pool_id
    }

    pub fn lease_epoch(&self) -> i64 {
        self.record.lease_epoch
    }

    pub fn expires_at(&self) -> DateTime<Utc> {
        self.record.expires_at
    }

    pub fn remaining_ttl(&self, now: DateTime<Utc>) -> Duration {
        self.record.expires_at - now
    }

    pub(crate) fn record(&self) -> &AccountLeaseRecord {
        &self.record
    }

    pub(crate) fn with_record(record: AccountLeaseRecord) -> Self {
        Self { record }
    }
}

/// Snapshot of usage near account limits.
#[derive(Debug, Clone, PartialEq)]
pub struct RateLimitSnapshot {
    pub used_percent: f64,
    pub observed_at: DateTime<Utc>,
}

impl RateLimitSnapshot {
    pub fn new(used_percent: f64, observed_at: DateTime<Utc>) -> Self {
        Self {
            used_percent,
            observed_at,
        }
    }
}

/// Structured event for hard usage-limit failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageLimitEvent {
    pub observed_at: DateTime<Utc>,
}

impl UsageLimitEvent {
    pub fn new(observed_at: DateTime<Utc>) -> Self {
        Self { observed_at }
    }
}

/// Whether a health event was applied to the active lease lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthEventDisposition {
    Applied,
    IgnoredAsStale,
}

/// Inputs required to decide whether remote context can be carried across accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextReuseRequest {
    pub allow_context_reuse: bool,
    pub explicit_context_reuse_consent: bool,
    pub same_workspace: bool,
    pub same_backend_family: bool,
    pub transport_portable: bool,
}

/// Decision for cross-account remote context handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextReuseDecision {
    ReuseRemoteContext,
    ResetRemoteContext,
}
