use crate::types::AccountRecord;
use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use codex_state::AccountHealthEvent;
use codex_state::AccountLeaseError;
use codex_state::AccountLeaseRecord;
use codex_state::AccountStartupSelectionState;
use codex_state::LeaseKey;
use codex_state::LeaseRenewal;
use codex_state::LegacyAccountImport;

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
pub trait AccountPoolLeaseBackend: Send + Sync {
    /// Acquire (or rehydrate) the current holder lease for a pool.
    async fn acquire_lease(
        &self,
        pool_id: &str,
        holder_instance_id: &str,
    ) -> std::result::Result<AccountLeaseRecord, AccountLeaseError>;

    /// Renew the lease if it is still active.
    async fn renew_lease(
        &self,
        lease: &LeaseKey,
        now: DateTime<Utc>,
    ) -> anyhow::Result<LeaseRenewal>;

    /// Record a lease-scoped account health event.
    async fn record_health_event(&self, event: AccountHealthEvent) -> anyhow::Result<()>;

    /// Read persisted startup selection state.
    async fn read_startup_selection(&self) -> anyhow::Result<AccountStartupSelectionState>;

    /// Import a legacy default account into pooled state.
    async fn import_legacy_default_account(
        &self,
        legacy_account: LegacyAccountImport,
    ) -> anyhow::Result<()>;
}
