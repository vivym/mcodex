use anyhow::Context;
use std::collections::HashSet;
use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::Mutex;

use super::authority::RuntimeLeaseAuthority;
use super::collaboration_tree::CollaborationTreeId;
use super::collaboration_tree::CollaborationTreeMembership;
use super::collaboration_tree::CollaborationTreeRegistry;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteContextResetRecord {
    pub(crate) session_id: String,
    pub(crate) turn_id: Option<String>,
    pub(crate) request_id: String,
    pub(crate) lease_generation: u64,
    pub(crate) transport_reset_generation: u64,
}

#[derive(Default)]
struct RuntimeLeaseHostLifecycle {
    attached_sessions: HashSet<String>,
    pending_startups: HashSet<String>,
}

#[cfg_attr(not(test), allow(dead_code))]
struct RuntimeLeaseHostInner {
    id: RuntimeLeaseHostId,
    mode: RuntimeLeaseHostMode,
    authority: StdMutex<Option<RuntimeLeaseAuthority>>,
    collaboration_registry: Arc<CollaborationTreeRegistry>,
    latest_remote_context_reset: StdMutex<Option<RemoteContextResetRecord>>,
    lifecycle: Mutex<RuntimeLeaseHostLifecycle>,
}

impl fmt::Debug for RuntimeLeaseHostInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeLeaseHostInner")
            .field("id", &self.id)
            .field("mode", &self.mode)
            .field(
                "has_authority",
                &self
                    .authority
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_some(),
            )
            .field(
                "latest_remote_context_reset",
                &self
                    .latest_remote_context_reset
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            )
            .finish()
    }
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
            authority: StdMutex::new(None),
            collaboration_registry: Arc::new(CollaborationTreeRegistry::default()),
            latest_remote_context_reset: StdMutex::new(None),
            lifecycle: Mutex::new(RuntimeLeaseHostLifecycle::default()),
        }))
    }

    pub(crate) fn non_pooled(id: RuntimeLeaseHostId) -> Self {
        Self(Arc::new(RuntimeLeaseHostInner {
            id,
            mode: RuntimeLeaseHostMode::NonPooled,
            authority: StdMutex::new(None),
            collaboration_registry: Arc::new(CollaborationTreeRegistry::default()),
            latest_remote_context_reset: StdMutex::new(None),
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

    pub(crate) fn install_authority(&self, authority: RuntimeLeaseAuthority) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.is_pooled(),
            "runtime lease host {} cannot install a pooled authority in non-pooled mode",
            self.id()
        );
        authority.set_collaboration_registry(Arc::clone(&self.0.collaboration_registry));
        let mut stored_authority = self
            .0
            .authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if stored_authority.is_some() {
            anyhow::bail!(
                "runtime lease host {} already has published pooled authority",
                self.id()
            );
        }
        *stored_authority = Some(authority);
        Ok(())
    }

    pub(crate) fn register_collaboration_member(
        &self,
        tree_id: CollaborationTreeId,
        member_id: String,
        cancellation_token: tokio_util::sync::CancellationToken,
    ) -> CollaborationTreeMembership {
        self.0
            .collaboration_registry
            .register_member(tree_id, member_id, cancellation_token)
    }

    pub(crate) fn pooled_authority(&self) -> Option<RuntimeLeaseAuthority> {
        if !self.is_pooled() {
            return None;
        }
        self.0
            .authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn record_remote_context_reset(&self, record: RemoteContextResetRecord) {
        *self
            .0
            .latest_remote_context_reset
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(record);
    }

    pub(crate) fn latest_remote_context_reset(&self) -> Option<RemoteContextResetRecord> {
        self.0
            .latest_remote_context_reset
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) async fn account_lease_snapshot(
        &self,
    ) -> Option<crate::state::AccountLeaseRuntimeSnapshot> {
        if !self.is_pooled() {
            return None;
        }
        let mut snapshot = if let Some(authority) = self.pooled_authority() {
            authority.runtime_snapshot().await
        } else {
            return None;
        };
        if let Some(remote_context_reset) = self.latest_remote_context_reset() {
            snapshot.transport_reset_generation =
                Some(remote_context_reset.transport_reset_generation);
            snapshot.last_remote_context_reset_turn_id = remote_context_reset.turn_id;
        }
        Some(snapshot)
    }

    pub(crate) async fn release_for_shutdown(&self) -> anyhow::Result<()> {
        if let Some(authority) = self.pooled_authority() {
            authority.release_for_shutdown().await?;
        }
        Ok(())
    }

    pub(crate) fn ensure_child_startup_ready(&self) -> anyhow::Result<()> {
        if self.is_pooled() && self.pooled_authority().is_none() {
            anyhow::bail!(
                "runtime lease host {} has no published pooled authority for child startup",
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
            self.ensure_child_startup_ready()?;
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
        if let Some(authority) = self.pooled_authority() {
            authority.release_for_shutdown().await?;
        }
        self.0
            .authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
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
        if let Some(authority) = self.pooled_authority() {
            authority.release_for_shutdown().await?;
        }
        self.0
            .authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
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
    pub(crate) fn pooled_with_authority_for_test(
        id: RuntimeLeaseHostId,
        authority: RuntimeLeaseAuthority,
    ) -> Self {
        let collaboration_registry = Arc::new(CollaborationTreeRegistry::default());
        authority.set_collaboration_registry(Arc::clone(&collaboration_registry));
        Self(Arc::new(RuntimeLeaseHostInner {
            id,
            mode: RuntimeLeaseHostMode::Pooled,
            authority: StdMutex::new(Some(authority)),
            collaboration_registry,
            latest_remote_context_reset: StdMutex::new(None),
            lifecycle: Mutex::new(RuntimeLeaseHostLifecycle::default()),
        }))
    }

    #[cfg(test)]
    pub(crate) fn authority_for_test(&self) -> Option<RuntimeLeaseAuthority> {
        self.pooled_authority()
    }

    #[cfg(test)]
    pub(crate) fn ptr_eq_for_test(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    #[cfg(test)]
    pub(crate) fn collaboration_member_count_for_test(
        &self,
        tree_id: &CollaborationTreeId,
    ) -> usize {
        self.0.collaboration_registry.member_count_for_test(tree_id)
    }

    #[cfg(test)]
    pub(crate) fn collaboration_member_ids_for_test(
        &self,
        tree_id: &CollaborationTreeId,
    ) -> Vec<String> {
        self.0.collaboration_registry.member_ids_for_test(tree_id)
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
