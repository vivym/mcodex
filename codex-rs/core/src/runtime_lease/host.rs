use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::sync::Mutex;

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

#[derive(Default)]
struct RuntimeLeaseHostLifecycle {
    attached_sessions: HashSet<String>,
}

#[cfg_attr(not(test), allow(dead_code))]
struct RuntimeLeaseHostInner {
    id: RuntimeLeaseHostId,
    mode: RuntimeLeaseHostMode,
    authority: Option<Arc<RuntimeLeaseAuthorityMarker>>,
    legacy_manager_bridge: OnceLock<Arc<Mutex<crate::state::AccountPoolManager>>>,
    lifecycle: Mutex<RuntimeLeaseHostLifecycle>,
}

impl fmt::Debug for RuntimeLeaseHostInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeLeaseHostInner")
            .field("id", &self.id)
            .field("mode", &self.mode)
            .field("has_authority", &self.authority.is_some())
            .field(
                "has_legacy_manager_bridge",
                &self.legacy_manager_bridge.get().is_some(),
            )
            .finish()
    }
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
            legacy_manager_bridge: OnceLock::new(),
            lifecycle: Mutex::new(RuntimeLeaseHostLifecycle::default()),
        }))
    }

    pub(crate) fn non_pooled(id: RuntimeLeaseHostId) -> Self {
        Self(Arc::new(RuntimeLeaseHostInner {
            id,
            mode: RuntimeLeaseHostMode::NonPooled,
            authority: None,
            legacy_manager_bridge: OnceLock::new(),
            lifecycle: Mutex::new(RuntimeLeaseHostLifecycle::default()),
        }))
    }

    pub(crate) fn id(&self) -> RuntimeLeaseHostId {
        self.0.id.clone()
    }

    pub(crate) fn mode(&self) -> RuntimeLeaseHostMode {
        self.0.mode
    }

    pub(crate) fn is_pooled(&self) -> bool {
        self.mode() == RuntimeLeaseHostMode::Pooled
    }

    pub(crate) fn attach_legacy_manager_bridge(
        &self,
        manager: Arc<Mutex<crate::state::AccountPoolManager>>,
    ) {
        if let Err(existing) = self.0.legacy_manager_bridge.set(manager) {
            debug_assert!(
                self.0
                    .legacy_manager_bridge
                    .get()
                    .is_some_and(|attached| Arc::ptr_eq(attached, &existing)),
                "legacy manager bridge cannot be replaced"
            );
        }
    }

    pub(crate) fn has_legacy_manager_bridge(&self) -> bool {
        self.0.legacy_manager_bridge.get().is_some()
    }

    pub(crate) fn legacy_manager_bridge(
        &self,
    ) -> Option<Arc<Mutex<crate::state::AccountPoolManager>>> {
        self.0.legacy_manager_bridge.get().cloned()
    }

    pub(crate) async fn attach_session(&self, session_id: &str) {
        if !self.is_pooled() {
            return;
        }
        let mut lifecycle = self.0.lifecycle.lock().await;
        lifecycle.attached_sessions.insert(session_id.to_string());
    }

    pub(crate) async fn detach_session(&self, session_id: &str) -> anyhow::Result<()> {
        if !self.is_pooled() {
            return Ok(());
        }
        let mut lifecycle = self.0.lifecycle.lock().await;
        if !lifecycle.attached_sessions.remove(session_id)
            || !lifecycle.attached_sessions.is_empty()
        {
            return Ok(());
        }
        if let Some(manager) = self.legacy_manager_bridge() {
            let mut manager = manager.lock().await;
            manager.release_for_shutdown().await?;
        }
        Ok(())
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

    #[cfg(test)]
    pub(crate) fn legacy_manager_bridge_for_test(
        &self,
    ) -> Option<Arc<Mutex<crate::state::AccountPoolManager>>> {
        self.legacy_manager_bridge()
    }

    #[cfg(test)]
    pub(crate) async fn attached_session_count_for_test(&self) -> usize {
        self.0.lifecycle.lock().await.attached_sessions.len()
    }
}
