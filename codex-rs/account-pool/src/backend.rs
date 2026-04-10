use crate::types::AccountRecord;

/// Read-only account source used by the startup selection policy.
///
/// Implementations must return accounts in stable priority order for startup
/// selection so `select_startup_account` can make a deterministic choice.
pub trait AccountPoolBackend {
    /// Returns the accounts available to the selector in stable priority order.
    fn accounts(&self) -> &[AccountRecord];
}
