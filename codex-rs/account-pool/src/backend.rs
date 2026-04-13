use crate::types::AccountRecord;
use crate::types::LeaseGrant;
use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use codex_login::ChatgptManagedRegistrationTokens;
use codex_state::AccountHealthEvent;
use codex_state::AccountLeaseError;
use codex_state::AccountStartupSelectionState;
use codex_state::LeaseKey;
use codex_state::LeaseRenewal;
use codex_state::RegisteredAccountRecord;
use codex_state::RegisteredAccountUpsert;

pub mod local;

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
