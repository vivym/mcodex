//! Runtime-scoped account-pool lease ownership.
//!
//! Pooled lease choice is runtime-owned. Sessions consume request-scoped
//! admissions from this module and keep only session-local transport continuity.

mod host;

#[cfg(test)]
mod tests;

pub(crate) use host::RuntimeLeaseHost;
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use host::RuntimeLeaseHostId;
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use host::RuntimeLeaseHostMode;
pub(crate) use host::retry_shutdown_release;
