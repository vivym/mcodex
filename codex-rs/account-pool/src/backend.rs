use crate::types::AccountRecord;
use crate::types::LeaseGrant;
use crate::types::SelectionRequest;
use async_trait::async_trait;
use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use codex_login::ChatgptManagedRegistrationTokens;
use codex_state::AccountHealthEvent;
use codex_state::AccountLeaseError;
use codex_state::AccountStartupSelectionState;
use codex_state::AccountStartupStatus;
use codex_state::LeaseKey;
use codex_state::LeaseRenewal;
use codex_state::RegisteredAccountRecord;
use codex_state::RegisteredAccountUpsert;

pub mod local;
pub use crate::observability::AccountOperationalState;
pub use crate::observability::AccountPoolAccount;
pub use crate::observability::AccountPoolAccountsListRequest;
pub use crate::observability::AccountPoolAccountsPage;
pub use crate::observability::AccountPoolDiagnostics;
pub use crate::observability::AccountPoolDiagnosticsReadRequest;
pub use crate::observability::AccountPoolDiagnosticsSeverity;
pub use crate::observability::AccountPoolDiagnosticsStatus;
pub use crate::observability::AccountPoolEvent;
pub use crate::observability::AccountPoolEventType;
pub use crate::observability::AccountPoolEventsListRequest;
pub use crate::observability::AccountPoolEventsPage;
pub use crate::observability::AccountPoolIssue;
pub use crate::observability::AccountPoolLease;
pub use crate::observability::AccountPoolObservabilityReader;
pub use crate::observability::AccountPoolQuota;
pub use crate::observability::AccountPoolReadRequest;
pub use crate::observability::AccountPoolReasonCode;
pub use crate::observability::AccountPoolSelection;
pub use crate::observability::AccountPoolSnapshot;
pub use crate::observability::AccountPoolSummary;
use crate::quota::ProbeOutcome;
use crate::quota::SelectionPlan;

/// Read-only account source used by the startup selection policy.
///
/// Implementations must return accounts in stable priority order for startup
/// selection so `select_startup_account` can make a deterministic choice.
pub trait AccountPoolBackend {
    /// Returns the accounts available to the selector in stable priority order.
    ///
    /// Callers are expected to populate `AccountRecord::quota.selection` with the
    /// requested selection-family row when present, and `quota.codex_fallback`
    /// with the compatibility fallback row when that family differs from
    /// `codex`.
    fn accounts(&self) -> &[AccountRecord];
}

/// Runtime state backend for local lease lifecycle operations.
#[async_trait]
pub trait AccountPoolExecutionBackend: Send + Sync {
    /// Build the shared quota-aware selection plan for one runtime lease attempt.
    async fn plan_runtime_selection(
        &self,
        request: &SelectionRequest,
        holder_instance_id: &str,
    ) -> anyhow::Result<(String, SelectionPlan)>;

    /// Acquire (or rehydrate) the current holder lease for a pool.
    async fn acquire_lease(
        &self,
        pool_id: &str,
        holder_instance_id: &str,
    ) -> std::result::Result<LeaseGrant, AccountLeaseError>;

    /// Read the currently active lease for a holder without running selection.
    ///
    /// Implementations should return `Some` only when the lease is still active
    /// and can be safely rehydrated by a new manager instance for the same
    /// holder.
    async fn read_active_holder_lease(
        &self,
        _holder_instance_id: &str,
    ) -> anyhow::Result<Option<LeaseGrant>> {
        Ok(None)
    }

    /// Acquire a pool lease while temporarily excluding specific account ids.
    async fn acquire_lease_excluding(
        &self,
        pool_id: &str,
        holder_instance_id: &str,
        excluded_account_ids: &[String],
    ) -> std::result::Result<LeaseGrant, AccountLeaseError> {
        let _ = excluded_account_ids;
        self.acquire_lease(pool_id, holder_instance_id).await
    }

    /// Acquire a lease for a specific account chosen by the selector.
    async fn acquire_preferred_lease(
        &self,
        pool_id: &str,
        account_id: &str,
        holder_instance_id: &str,
    ) -> std::result::Result<LeaseGrant, AccountLeaseError> {
        let _ = account_id;
        self.acquire_lease(pool_id, holder_instance_id).await
    }

    /// Reserve the probe slot for a blocked candidate by advancing `next_probe_after`.
    async fn reserve_quota_probe(
        &self,
        _account_id: &str,
        _selection_family: &str,
        _now: DateTime<Utc>,
        _reserved_for: Duration,
    ) -> anyhow::Result<bool> {
        Ok(false)
    }

    /// Renew the lease if it is still active.
    async fn renew_lease(
        &self,
        lease: &LeaseKey,
        now: DateTime<Utc>,
    ) -> anyhow::Result<LeaseRenewal>;

    /// Release the lease if it still matches the current persisted epoch.
    async fn release_lease(&self, lease: &LeaseKey, now: DateTime<Utc>) -> anyhow::Result<bool>;

    /// Record a lease-scoped account health event.
    async fn record_health_event(&self, event: AccountHealthEvent) -> anyhow::Result<()>;

    /// Read the last persisted health-event sequence for an account.
    async fn read_account_health_event_sequence(
        &self,
        account_id: &str,
    ) -> anyhow::Result<Option<i64>>;

    /// Read persisted startup selection state.
    async fn read_startup_selection(&self) -> anyhow::Result<AccountStartupSelectionState>;

    /// Read startup selection facts annotated with effective source metadata.
    async fn read_account_startup_status(
        &self,
        configured_default_pool_id: Option<&str>,
    ) -> anyhow::Result<AccountStartupStatus>;

    /// Refresh quota state for a probe lease without consuming the next user turn.
    async fn refresh_quota_probe(
        &self,
        _lease: &LeaseGrant,
        _selection_family: &str,
    ) -> anyhow::Result<Option<ProbeOutcome>> {
        Ok(None)
    }
}

/// Control-plane backend for pooled account registration and deletion.
#[async_trait]
pub trait AccountPoolControlPlane: Send + Sync {
    /// Register or update a pooled account record.
    async fn register_account(
        &self,
        request: RegisteredAccountRegistration,
    ) -> anyhow::Result<RegisteredAccountRecord>;

    /// Delete a pooled account record by account identifier.
    async fn delete_registered_account(&self, account_id: &str) -> anyhow::Result<bool>;
}

/// Self-describing request for pooled account registration.
#[derive(Debug, Clone)]
pub struct RegisteredAccountRegistration {
    pub request: RegisteredAccountUpsert,
    pub pooled_registration_tokens: Option<ChatgptManagedRegistrationTokens>,
}
