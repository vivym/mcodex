use crate::StartupPoolInventory;
use crate::StartupSelectionFacts;
use crate::types::AccountRecord;
use crate::types::LeaseGrant;
use async_trait::async_trait;
use chrono::DateTime;
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

/// Read-only account source used by the startup selection policy.
///
/// Implementations must return accounts in stable priority order for startup
/// selection so `select_startup_account` can make a deterministic choice.
pub trait AccountPoolBackend {
    /// Returns the accounts available to the selector in stable priority order.
    fn accounts(&self) -> &[AccountRecord];
}

/// Runtime state backend for local lease lifecycle operations.
#[async_trait]
pub trait AccountPoolExecutionBackend: Send + Sync {
    /// Acquire (or rehydrate) the current holder lease for a pool.
    async fn acquire_lease(
        &self,
        pool_id: &str,
        holder_instance_id: &str,
    ) -> std::result::Result<LeaseGrant, AccountLeaseError>;

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

    /// Read visible startup pools in backend-neutral form.
    async fn read_startup_pool_inventory(&self) -> anyhow::Result<StartupPoolInventory>;

    /// Read account-selection facts for a specific resolved startup pool.
    async fn read_startup_selection_facts(
        &self,
        pool_id: &str,
    ) -> anyhow::Result<StartupSelectionFacts>;

    /// Read startup selection facts annotated with effective source metadata.
    async fn read_account_startup_status(
        &self,
        configured_default_pool_id: Option<&str>,
    ) -> anyhow::Result<AccountStartupStatus>
    where
        Self: Sized,
    {
        Ok(crate::startup_status::read_shared_startup_status(
            self,
            configured_default_pool_id,
            None,
        )
        .await?
        .startup)
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
