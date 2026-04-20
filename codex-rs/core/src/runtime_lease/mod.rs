//! Runtime-scoped account-pool lease ownership.
//!
//! Pooled lease choice is runtime-owned. Sessions consume request-scoped
//! admissions from this module and keep only session-local transport continuity.

mod admission;
mod authority;
mod collaboration_tree;
mod host;
mod reporting;
mod session_view;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub(crate) use admission::LeaseAdmission;
#[allow(unused_imports)]
pub(crate) use admission::LeaseAdmissionError;
#[allow(unused_imports)]
pub(crate) use admission::LeaseAdmissionGuard;
#[allow(unused_imports)]
pub(crate) use admission::LeaseAuthHandle;
#[allow(unused_imports)]
pub(crate) use admission::LeaseRequestContext;
#[allow(unused_imports)]
pub(crate) use admission::LeaseSnapshot;
#[allow(unused_imports)]
pub(crate) use admission::RequestBoundaryKind;
#[allow(unused_imports)]
pub(crate) use authority::RuntimeLeaseAuthority;
#[allow(unused_imports)]
pub(crate) use collaboration_tree::CollaborationTreeBindingHandle;
#[allow(unused_imports)]
pub(crate) use collaboration_tree::CollaborationTreeId;
pub(crate) use host::RemoteContextResetRecord;
pub(crate) use host::RuntimeLeaseHost;
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use host::RuntimeLeaseHostId;
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use host::RuntimeLeaseHostMode;
#[allow(unused_imports)]
pub(crate) use host::RuntimeLeaseStartupReservation;
pub(crate) use host::retry_shutdown_release;
pub(crate) use reporting::LeaseRequestReporter;
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use session_view::SessionLeaseView;
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use session_view::SessionLeaseViewDecision;
