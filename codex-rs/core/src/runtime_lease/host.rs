use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RuntimeLeaseHostId(String);

#[cfg_attr(not(test), allow(dead_code))]
impl RuntimeLeaseHostId {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Display for RuntimeLeaseHostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeLeaseHostMode {
    Pooled,
    NonPooled,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug)]
pub(crate) struct RuntimeLeaseAuthorityMarker;

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug)]
struct RuntimeLeaseHostInner {
    id: RuntimeLeaseHostId,
    mode: RuntimeLeaseHostMode,
    authority: Option<Arc<RuntimeLeaseAuthorityMarker>>,
    legacy_manager_bridge_attached: AtomicBool,
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeLeaseHost(Arc<RuntimeLeaseHostInner>);

#[cfg_attr(not(test), allow(dead_code))]
impl RuntimeLeaseHost {
    pub(crate) fn pooled(id: RuntimeLeaseHostId) -> Self {
        Self(Arc::new(RuntimeLeaseHostInner {
            id,
            mode: RuntimeLeaseHostMode::Pooled,
            authority: Some(Arc::new(RuntimeLeaseAuthorityMarker)),
            legacy_manager_bridge_attached: AtomicBool::new(false),
        }))
    }

    pub(crate) fn non_pooled(id: RuntimeLeaseHostId) -> Self {
        Self(Arc::new(RuntimeLeaseHostInner {
            id,
            mode: RuntimeLeaseHostMode::NonPooled,
            authority: None,
            legacy_manager_bridge_attached: AtomicBool::new(false),
        }))
    }

    pub(crate) fn id(&self) -> RuntimeLeaseHostId {
        self.0.id.clone()
    }

    pub(crate) fn mode(&self) -> RuntimeLeaseHostMode {
        self.0.mode
    }

    pub(crate) fn attach_legacy_manager_bridge(&self) {
        self.0
            .legacy_manager_bridge_attached
            .store(true, Ordering::Release);
    }

    pub(crate) fn has_legacy_manager_bridge(&self) -> bool {
        self.0
            .legacy_manager_bridge_attached
            .load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn pooled_for_test(id: RuntimeLeaseHostId) -> Self {
        Self::pooled(id)
    }

    #[cfg(test)]
    pub(crate) fn non_pooled_for_test(id: RuntimeLeaseHostId) -> Self {
        Self::non_pooled(id)
    }

    #[cfg(test)]
    pub(crate) fn authority_for_test(&self) -> Option<Arc<RuntimeLeaseAuthorityMarker>> {
        self.0.authority.clone()
    }

    #[cfg(test)]
    pub(crate) fn ptr_eq_for_test(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    #[cfg(test)]
    pub(crate) fn has_legacy_manager_bridge_for_test(&self) -> bool {
        self.has_legacy_manager_bridge()
    }
}
