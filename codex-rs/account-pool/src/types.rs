use anyhow::Result;
use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use codex_login::auth::LeaseScopedAuthSession;
use codex_state::AccountLeaseRecord;
use codex_state::AccountQuotaStateRecord;
use codex_state::LeaseKey;
use std::fmt;
use std::sync::Arc;

use crate::quota::QuotaFamilyView;
use crate::quota::SelectionIntent;

/// Request for choosing the startup account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionRequest {
    pub now: Option<DateTime<Utc>>,
    pub pool_id: Option<String>,
    pub intent: SelectionIntent,
    pub selection_family: Option<String>,
    pub current_account_id: Option<String>,
    pub just_replaced_account_id: Option<String>,
    pub reserved_probe_target_account_id: Option<String>,
    pub proactive_threshold_percent: u8,
}

impl Default for SelectionRequest {
    fn default() -> Self {
        Self::for_intent(SelectionIntent::Startup)
    }
}

impl SelectionRequest {
    pub fn for_intent(intent: SelectionIntent) -> Self {
        Self {
            now: None,
            pool_id: None,
            intent,
            selection_family: None,
            current_account_id: None,
            just_replaced_account_id: None,
            reserved_probe_target_account_id: None,
            proactive_threshold_percent: 85,
        }
    }

    pub fn selection_family(&self) -> &str {
        self.selection_family.as_deref().unwrap_or("codex")
    }

    pub fn with_now(mut self, now: DateTime<Utc>) -> Self {
        self.now = Some(now);
        self
    }

    pub fn with_selection_family(mut self, selection_family: &str) -> Self {
        self.selection_family = Some(selection_family.to_string());
        self
    }

    pub fn with_current_account(mut self, account_id: &str) -> Self {
        self.current_account_id = Some(account_id.to_string());
        self
    }

    pub fn with_just_replaced_account(mut self, account_id: &str) -> Self {
        self.just_replaced_account_id = Some(account_id.to_string());
        self
    }

    pub fn with_reserved_probe_target(mut self, account_id: &str) -> Self {
        self.reserved_probe_target_account_id = Some(account_id.to_string());
        self
    }

    pub fn with_proactive_threshold_percent(mut self, proactive_threshold_percent: u8) -> Self {
        self.proactive_threshold_percent = proactive_threshold_percent;
        self
    }
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
#[derive(Debug, Clone, PartialEq)]
pub struct AccountRecord {
    pub account_id: String,
    pub healthy: bool,
    pub kind: AccountKind,
    pub enabled: bool,
    pub selector_auth_eligible: bool,
    pub pool_position: usize,
    pub leased_to_other_holder: bool,
    pub quota: QuotaFamilyView,
}

impl AccountRecord {
    pub fn selection_quota(&self, selection_family: &str) -> Option<&AccountQuotaStateRecord> {
        self.quota.effective_quota(selection_family)
    }
}

/// Local manager configuration for lease lifecycle and switch policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolConfig {
    pub default_pool_id: Option<String>,
    pub proactive_switch_threshold_percent: u8,
    pub lease_ttl_secs: u64,
    pub heartbeat_interval_secs: u64,
    pub min_switch_interval_secs: u64,
}

impl Default for AccountPoolConfig {
    fn default() -> Self {
        Self {
            default_pool_id: None,
            proactive_switch_threshold_percent: 85,
            lease_ttl_secs: 300,
            heartbeat_interval_secs: 60,
            min_switch_interval_secs: 0,
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

    pub fn lease_ttl_duration(&self) -> Duration {
        Duration::seconds(self.lease_ttl_secs as i64)
    }

    pub fn min_switch_interval_duration(&self) -> Duration {
        Duration::seconds(self.min_switch_interval_secs as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::AccountPoolConfig;
    use chrono::Duration;
    use pretty_assertions::assert_eq;

    #[test]
    fn validate_allows_min_switch_interval_at_or_above_lease_ttl() {
        let config = AccountPoolConfig {
            lease_ttl_secs: 300,
            heartbeat_interval_secs: 60,
            min_switch_interval_secs: 300,
            ..AccountPoolConfig::default()
        };

        config.validate().expect("config should be valid");
    }

    #[test]
    fn min_switch_interval_duration_uses_configured_seconds() {
        let config = AccountPoolConfig {
            min_switch_interval_secs: 90,
            ..AccountPoolConfig::default()
        };

        assert_eq!(config.min_switch_interval_duration(), Duration::seconds(90));
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

    pub fn acquired_at(&self) -> DateTime<Utc> {
        self.record.acquired_at
    }

    pub fn remaining_ttl(&self, now: DateTime<Utc>) -> Duration {
        self.record.expires_at - now
    }
}

/// Lease grant that carries the lease snapshot and a lease-scoped auth session.
#[derive(Clone)]
pub struct LeaseGrant {
    pub lease_key: LeaseKey,
    pub account_id: String,
    pub pool_id: String,
    pub auth_session: Arc<dyn LeaseScopedAuthSession>,
    pub expires_at: DateTime<Utc>,
    pub next_eligible_at: Option<DateTime<Utc>>,
    acquired_at: DateTime<Utc>,
    holder_instance_id: String,
}

impl fmt::Debug for LeaseGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeaseGrant")
            .field("lease_key", &self.lease_key)
            .field("account_id", &self.account_id)
            .field("pool_id", &self.pool_id)
            .field("expires_at", &self.expires_at)
            .field("next_eligible_at", &self.next_eligible_at)
            .field("acquired_at", &self.acquired_at)
            .finish_non_exhaustive()
    }
}

impl LeaseGrant {
    pub(crate) fn from_record(
        record: AccountLeaseRecord,
        auth_session: Arc<dyn LeaseScopedAuthSession>,
        next_eligible_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            lease_key: record.lease_key(),
            account_id: record.account_id,
            pool_id: record.pool_id,
            auth_session,
            expires_at: record.expires_at,
            next_eligible_at,
            acquired_at: record.acquired_at,
            holder_instance_id: record.holder_instance_id,
        }
    }

    pub fn key(&self) -> LeaseKey {
        self.lease_key.clone()
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    pub fn pool_id(&self) -> &str {
        &self.pool_id
    }

    pub fn lease_epoch(&self) -> i64 {
        self.lease_key.lease_epoch
    }

    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }

    pub fn acquired_at(&self) -> DateTime<Utc> {
        self.acquired_at
    }

    pub fn remaining_ttl(&self, now: DateTime<Utc>) -> Duration {
        self.expires_at - now
    }

    pub(crate) fn with_record(mut self, record: AccountLeaseRecord) -> Self {
        self.lease_key = record.lease_key();
        self.account_id = record.account_id;
        self.pool_id = record.pool_id;
        self.expires_at = record.expires_at;
        self.acquired_at = record.acquired_at;
        self.holder_instance_id = record.holder_instance_id;
        self
    }

    pub(crate) fn with_lease_epoch(mut self, lease_epoch: i64) -> Self {
        self.lease_key.lease_epoch = lease_epoch;
        self
    }

    pub(crate) fn leased_account(&self) -> LeasedAccount {
        LeasedAccount::new(AccountLeaseRecord {
            lease_id: self.lease_key.lease_id.clone(),
            pool_id: self.pool_id.clone(),
            account_id: self.account_id.clone(),
            holder_instance_id: self.holder_instance_id.clone(),
            lease_epoch: self.lease_key.lease_epoch,
            acquired_at: self.acquired_at,
            renewed_at: self.acquired_at,
            expires_at: self.expires_at,
            released_at: None,
        })
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
