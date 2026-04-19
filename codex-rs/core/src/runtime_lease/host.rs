use std::fmt;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RuntimeLeaseHostId(String);

impl RuntimeLeaseHostId {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    #[cfg(test)]
    pub(crate) fn for_test(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl fmt::Display for RuntimeLeaseHostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeLeaseHostMode {
    Pooled,
    NonPooled,
}

#[derive(Debug)]
pub(crate) struct RuntimeLeaseAuthorityMarker;

#[derive(Clone, Debug)]
pub(crate) struct RuntimeLeaseHost {
    id: RuntimeLeaseHostId,
    authority: Option<Arc<RuntimeLeaseAuthorityMarker>>,
}

impl RuntimeLeaseHost {
    pub(crate) fn pooled(id: RuntimeLeaseHostId) -> Self {
        Self {
            id,
            authority: Some(Arc::new(RuntimeLeaseAuthorityMarker)),
        }
    }

    pub(crate) fn non_pooled(id: RuntimeLeaseHostId) -> Self {
        Self {
            id,
            authority: None,
        }
    }

    pub(crate) fn id(&self) -> RuntimeLeaseHostId {
        self.id.clone()
    }

    pub(crate) fn mode(&self) -> RuntimeLeaseHostMode {
        if self.authority.is_some() {
            RuntimeLeaseHostMode::Pooled
        } else {
            RuntimeLeaseHostMode::NonPooled
        }
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
        self.authority.clone()
    }
}
