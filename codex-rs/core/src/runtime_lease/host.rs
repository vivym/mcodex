use anyhow::Context;
use std::collections::HashSet;
use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::MutexGuard as StdMutexGuard;
use tokio::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RuntimeLeaseHostId(String);

const SHUTDOWN_RELEASE_MAX_ATTEMPTS: u64 = 3;

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

type LegacyManagerBridge = Option<Arc<Mutex<crate::state::AccountPoolManager>>>;

#[derive(Default)]
struct RuntimeLeaseHostLifecycle {
    attached_sessions: HashSet<String>,
    pending_startups: HashSet<String>,
}

#[cfg_attr(not(test), allow(dead_code))]
struct RuntimeLeaseHostInner {
    id: RuntimeLeaseHostId,
    mode: RuntimeLeaseHostMode,
    authority: Option<Arc<RuntimeLeaseAuthorityMarker>>,
    legacy_manager_bridge: StdMutex<LegacyManagerBridge>,
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
                &self
                    .legacy_manager_bridge
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_some(),
            )
            .finish()
    }
}

fn lock_legacy_manager_bridge(
    bridge: &StdMutex<LegacyManagerBridge>,
) -> StdMutexGuard<'_, LegacyManagerBridge> {
    // Teardown should still be able to clear an attached bridge after a panic
    // poisons the bookkeeping lock.
    bridge
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeLeaseHost(Arc<RuntimeLeaseHostInner>);

#[derive(Debug)]
pub(crate) struct RuntimeLeaseStartupReservation {
    host: RuntimeLeaseHost,
    reservation_id: Option<String>,
}

impl RuntimeLeaseStartupReservation {
    pub(crate) async fn promote_to_session(mut self, session_id: &str) -> anyhow::Result<()> {
        {
            let Some(reservation_id) = self.reservation_id.as_deref() else {
                anyhow::bail!("startup reservation was already consumed");
            };
            self.host
                .promote_startup_reservation_to_session(reservation_id, session_id)
                .await?;
        }
        self.reservation_id = None;
        Ok(())
    }

    pub(crate) async fn rollback(mut self) -> anyhow::Result<()> {
        let result = {
            let Some(reservation_id) = self.reservation_id.as_deref() else {
                anyhow::bail!("startup reservation was already consumed");
            };
            self.host
                .rollback_startup_reservation_with_retry(reservation_id)
                .await
        };
        if result.is_ok() {
            self.reservation_id = None;
        }
        result
    }
}

impl Drop for RuntimeLeaseStartupReservation {
    fn drop(&mut self) {
        let Some(reservation_id) = self.reservation_id.take() else {
            return;
        };
        if !self.host.is_pooled() {
            return;
        }

        let host = self.host.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                std::mem::drop(handle.spawn(async move {
                    host.cleanup_startup_reservation_until_done(reservation_id)
                        .await;
                }));
            }
            Err(err) => {
                let cleanup_reservation_id = reservation_id.clone();
                if let Err(spawn_err) = std::thread::Builder::new()
                    .name("runtime-lease-startup-cleanup".to_string())
                    .spawn(move || {
                        match tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                        {
                            Ok(runtime) => {
                                runtime.block_on(
                                    host.cleanup_startup_reservation_until_done(reservation_id),
                                );
                            }
                            Err(build_err) => {
                                tracing::warn!(
                                    reservation_id,
                                    error = ?build_err,
                                    "failed to build runtime for dropped startup reservation cleanup"
                                );
                            }
                        }
                    })
                {
                    tracing::warn!(
                        reservation_id = %cleanup_reservation_id,
                        runtime_error = ?err,
                        error = ?spawn_err,
                        "failed to spawn cleanup thread for dropped startup reservation"
                    );
                }
            }
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl RuntimeLeaseHost {
    pub(crate) fn pooled(id: RuntimeLeaseHostId) -> Self {
        Self(Arc::new(RuntimeLeaseHostInner {
            id,
            mode: RuntimeLeaseHostMode::Pooled,
            authority: Some(Arc::new(RuntimeLeaseAuthorityMarker)),
            legacy_manager_bridge: StdMutex::new(None),
            lifecycle: Mutex::new(RuntimeLeaseHostLifecycle::default()),
        }))
    }

    pub(crate) fn non_pooled(id: RuntimeLeaseHostId) -> Self {
        Self(Arc::new(RuntimeLeaseHostInner {
            id,
            mode: RuntimeLeaseHostMode::NonPooled,
            authority: None,
            legacy_manager_bridge: StdMutex::new(None),
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
    ) -> anyhow::Result<()> {
        let mut legacy_manager_bridge = lock_legacy_manager_bridge(&self.0.legacy_manager_bridge);
        if let Some(existing) = legacy_manager_bridge.as_ref() {
            if Arc::ptr_eq(existing, &manager) {
                return Ok(());
            }
            anyhow::bail!(
                "runtime lease host {} already has a different legacy manager bridge",
                self.id()
            );
        }
        *legacy_manager_bridge = Some(manager);
        Ok(())
    }

    pub(crate) fn has_legacy_manager_bridge(&self) -> bool {
        lock_legacy_manager_bridge(&self.0.legacy_manager_bridge).is_some()
    }

    pub(crate) fn legacy_manager_bridge(
        &self,
    ) -> Option<Arc<Mutex<crate::state::AccountPoolManager>>> {
        lock_legacy_manager_bridge(&self.0.legacy_manager_bridge).clone()
    }

    pub(crate) fn ensure_legacy_manager_bridge_attached_for_child(&self) -> anyhow::Result<()> {
        if self.is_pooled() && !self.has_legacy_manager_bridge() {
            anyhow::bail!(
                "runtime lease host {} legacy manager bridge is not attached for child startup",
                self.id()
            );
        }
        Ok(())
    }

    pub(crate) async fn attach_session(&self, session_id: &str) {
        if !self.is_pooled() {
            return;
        }
        let mut lifecycle = self.0.lifecycle.lock().await;
        lifecycle.attached_sessions.insert(session_id.to_string());
    }

    pub(crate) async fn try_reserve_startup_for_child(
        &self,
        reservation_id: impl Into<String>,
    ) -> anyhow::Result<RuntimeLeaseStartupReservation> {
        let reservation_id = reservation_id.into();
        if self.is_pooled() {
            let mut lifecycle = self.0.lifecycle.lock().await;
            self.ensure_legacy_manager_bridge_attached_for_child()?;
            lifecycle.pending_startups.insert(reservation_id.clone());
        }
        Ok(RuntimeLeaseStartupReservation {
            host: self.clone(),
            reservation_id: Some(reservation_id),
        })
    }

    async fn promote_startup_reservation_to_session(
        &self,
        reservation_id: &str,
        session_id: &str,
    ) -> anyhow::Result<()> {
        if !self.is_pooled() {
            return Ok(());
        }
        let mut lifecycle = self.0.lifecycle.lock().await;
        if !lifecycle.pending_startups.remove(reservation_id) {
            anyhow::bail!(
                "runtime lease host {} has no pending startup reservation {reservation_id}",
                self.id()
            );
        }
        lifecycle.attached_sessions.insert(session_id.to_string());
        Ok(())
    }

    async fn rollback_startup_reservation_with_retry(
        &self,
        reservation_id: &str,
    ) -> anyhow::Result<()> {
        let release_target = format!(
            "runtime lease host {} startup reservation {reservation_id}",
            self.id()
        );
        retry_shutdown_release(&release_target, || {
            self.rollback_startup_reservation(reservation_id)
        })
        .await
    }

    async fn rollback_startup_reservation(&self, reservation_id: &str) -> anyhow::Result<()> {
        if !self.is_pooled() {
            return Ok(());
        }
        let mut lifecycle = self.0.lifecycle.lock().await;
        if !lifecycle.pending_startups.contains(reservation_id) {
            return Ok(());
        }
        if !lifecycle.attached_sessions.is_empty() || lifecycle.pending_startups.len() > 1 {
            lifecycle.pending_startups.remove(reservation_id);
            return Ok(());
        }
        if let Some(manager) = self.legacy_manager_bridge() {
            let mut manager = manager.lock().await;
            manager.release_for_shutdown().await?;
        }
        lock_legacy_manager_bridge(&self.0.legacy_manager_bridge).take();
        lifecycle.pending_startups.remove(reservation_id);
        Ok(())
    }

    async fn cleanup_startup_reservation_until_done(self, reservation_id: String) {
        let mut cleanup_attempt = 1_u64;
        loop {
            match self
                .rollback_startup_reservation_with_retry(&reservation_id)
                .await
            {
                Ok(()) => return,
                Err(err) => {
                    let delay = crate::util::backoff(cleanup_attempt);
                    tracing::warn!(
                        cleanup_attempt,
                        %reservation_id,
                        ?delay,
                        error = ?err,
                        "startup reservation cleanup failed; retrying"
                    );
                    tokio::time::sleep(delay).await;
                    cleanup_attempt = cleanup_attempt.saturating_add(1);
                }
            }
        }
    }

    pub(crate) async fn detach_session(&self, session_id: &str) -> anyhow::Result<()> {
        if !self.is_pooled() {
            return Ok(());
        }
        let mut lifecycle = self.0.lifecycle.lock().await;
        if !lifecycle.attached_sessions.contains(session_id) {
            return Ok(());
        }
        if lifecycle.attached_sessions.len() > 1 || !lifecycle.pending_startups.is_empty() {
            lifecycle.attached_sessions.remove(session_id);
            return Ok(());
        }
        if let Some(manager) = self.legacy_manager_bridge() {
            let mut manager = manager.lock().await;
            manager.release_for_shutdown().await?;
        }
        lock_legacy_manager_bridge(&self.0.legacy_manager_bridge).take();
        lifecycle.attached_sessions.remove(session_id);
        Ok(())
    }

    pub(crate) async fn detach_session_with_retry(&self, session_id: &str) -> anyhow::Result<()> {
        let release_target = format!("runtime lease host {} session {session_id}", self.id());
        retry_shutdown_release(&release_target, || self.detach_session(session_id)).await
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

    #[cfg(test)]
    pub(crate) async fn pending_startup_count_for_test(&self) -> usize {
        self.0.lifecycle.lock().await.pending_startups.len()
    }
}

pub(crate) async fn retry_shutdown_release<F, Fut>(
    release_target: &str,
    mut operation: F,
) -> anyhow::Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    for attempt in 1..=SHUTDOWN_RELEASE_MAX_ATTEMPTS {
        match operation().await {
            Ok(()) => return Ok(()),
            Err(err) if attempt < SHUTDOWN_RELEASE_MAX_ATTEMPTS => {
                let delay = crate::util::backoff(attempt);
                tracing::warn!(
                    attempt,
                    max_attempts = SHUTDOWN_RELEASE_MAX_ATTEMPTS,
                    %release_target,
                    ?delay,
                    error = ?err,
                    "shutdown release failed; retrying"
                );
                tokio::time::sleep(delay).await;
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "failed to release {release_target} during shutdown after {attempt} attempts"
                    )
                });
            }
        }
    }
    unreachable!("shutdown release retry loop should always return");
}
