use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use codex_login::auth::LeaseScopedAuthSession;
use codex_protocol::protocol::RateLimitSnapshot;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use uuid::Uuid;

use super::admission::LeaseAdmission;
use super::admission::LeaseAdmissionError;
use super::admission::LeaseAdmissionGuard;
use super::admission::LeaseAuthHandle;
use super::admission::LeaseRequestContext;
use super::admission::LeaseSnapshot;

#[allow(dead_code)]
#[derive(Clone)]
pub(crate) struct RuntimeLeaseAuthority {
    inner: Arc<AuthorityInner>,
}

struct AuthorityInner {
    state: Mutex<AuthorityState>,
    admissions: StdMutex<AdmissionTrackerState>,
    changed: Notify,
}

struct AuthorityState {
    mode: RuntimeLeaseAuthorityMode,
}

#[allow(dead_code)]
enum RuntimeLeaseAuthorityMode {
    LegacyManagerBridge(Arc<Mutex<crate::state::AccountPoolManager>>),
    HostOwned(HostOwnedLeaseState),
}

#[derive(Clone)]
struct GenerationState {
    pool_id: String,
    account_id: String,
    selection_family: String,
    generation: u64,
    auth_session: Arc<dyn LeaseScopedAuthSession>,
    allow_context_reuse: bool,
    accepting: bool,
}

struct AdmissionTrackerState {
    active_generation: Option<u64>,
    admissions: HashSet<Uuid>,
}

struct HostOwnedLeaseState {
    generation: Option<GenerationState>,
}

enum AdmissionAttempt {
    Admitted(LeaseAdmission),
    Draining,
}

#[allow(dead_code)]
impl RuntimeLeaseAuthority {
    pub(crate) fn legacy_manager_bridge(
        manager: Arc<Mutex<crate::state::AccountPoolManager>>,
    ) -> Self {
        Self::new(RuntimeLeaseAuthorityMode::LegacyManagerBridge(manager))
    }

    fn new(mode: RuntimeLeaseAuthorityMode) -> Self {
        Self {
            inner: Arc::new(AuthorityInner {
                state: Mutex::new(AuthorityState { mode }),
                admissions: StdMutex::new(AdmissionTrackerState {
                    active_generation: None,
                    admissions: HashSet::new(),
                }),
                changed: Notify::new(),
            }),
        }
    }

    pub(crate) async fn acquire_request_lease(
        &self,
        context: LeaseRequestContext,
    ) -> Result<LeaseAdmission, LeaseAdmissionError> {
        loop {
            let changed = self.inner.changed.notified();
            if context.cancel.is_cancelled() {
                return Err(LeaseAdmissionError::Cancelled);
            }

            let legacy_manager = {
                let state = self.inner.state.lock().await;
                match &state.mode {
                    RuntimeLeaseAuthorityMode::LegacyManagerBridge(manager) => {
                        Some(Arc::clone(manager))
                    }
                    RuntimeLeaseAuthorityMode::HostOwned(host_owned) => {
                        if let Some(generation) = host_owned.generation.as_ref()
                            && generation.accepting
                        {
                            match self.inner.try_admit(&context, generation) {
                                AdmissionAttempt::Admitted(admission) => {
                                    return Ok(admission);
                                }
                                AdmissionAttempt::Draining => {}
                            }
                        }
                        None
                    }
                }
            };

            if let Some(manager) = legacy_manager {
                return self.acquire_from_legacy_manager(context, manager).await;
            }

            tokio::select! {
                () = context.cancel.cancelled() => return Err(LeaseAdmissionError::Cancelled),
                () = changed => {}
            }
        }
    }

    pub(crate) async fn close_current_generation(&self) {
        {
            let mut state = self.inner.state.lock().await;
            if let RuntimeLeaseAuthorityMode::HostOwned(host_owned) = &mut state.mode
                && let Some(generation) = host_owned.generation.as_mut()
            {
                generation.accepting = false;
            }
        }
        self.inner.changed.notify_waiters();
    }

    pub(crate) async fn invalidate_current_generation(&self) {
        {
            let mut state = self.inner.state.lock().await;
            if let RuntimeLeaseAuthorityMode::HostOwned(host_owned) = &mut state.mode {
                host_owned.generation = None;
            }
        }
        self.inner.changed.notify_waiters();
    }

    pub(crate) async fn report_rate_limits(
        &self,
        snapshot: &LeaseSnapshot,
        rate_limits: &RateLimitSnapshot,
    ) -> anyhow::Result<()> {
        let manager = {
            let state = self.inner.state.lock().await;
            match &state.mode {
                RuntimeLeaseAuthorityMode::LegacyManagerBridge(manager) => {
                    Some(Arc::clone(manager))
                }
                RuntimeLeaseAuthorityMode::HostOwned(_) => None,
            }
        };
        if let Some(manager) = manager {
            manager
                .lock()
                .await
                .report_rate_limits_for_generation(
                    snapshot.generation,
                    snapshot.account_id.as_str(),
                    snapshot.pool_id.as_str(),
                    rate_limits,
                )
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn report_usage_limit_reached(
        &self,
        snapshot: &LeaseSnapshot,
    ) -> anyhow::Result<()> {
        let manager = {
            let state = self.inner.state.lock().await;
            match &state.mode {
                RuntimeLeaseAuthorityMode::LegacyManagerBridge(manager) => {
                    Some(Arc::clone(manager))
                }
                RuntimeLeaseAuthorityMode::HostOwned(_) => None,
            }
        };
        if let Some(manager) = manager {
            manager
                .lock()
                .await
                .report_usage_limit_reached_for_generation(
                    snapshot.generation,
                    snapshot.account_id.as_str(),
                    snapshot.pool_id.as_str(),
                )
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn report_terminal_unauthorized(
        &self,
        snapshot: &LeaseSnapshot,
    ) -> anyhow::Result<()> {
        let manager = {
            let state = self.inner.state.lock().await;
            match &state.mode {
                RuntimeLeaseAuthorityMode::LegacyManagerBridge(manager) => {
                    Some(Arc::clone(manager))
                }
                RuntimeLeaseAuthorityMode::HostOwned(_) => None,
            }
        };
        if let Some(manager) = manager {
            manager
                .lock()
                .await
                .report_unauthorized_for_generation(
                    snapshot.generation,
                    snapshot.account_id.as_str(),
                    snapshot.pool_id.as_str(),
                )
                .await?;
        }
        Ok(())
    }

    async fn acquire_from_legacy_manager(
        &self,
        context: LeaseRequestContext,
        manager: Arc<Mutex<crate::state::AccountPoolManager>>,
    ) -> Result<LeaseAdmission, LeaseAdmissionError> {
        let mut manager = tokio::select! {
            () = context.cancel.cancelled() => return Err(LeaseAdmissionError::Cancelled),
            manager = manager.lock_owned() => manager,
        };
        if context.cancel.is_cancelled() {
            return Err(LeaseAdmissionError::Cancelled);
        }
        let selection = manager
            .prepare_turn()
            .await
            .map_err(|_| LeaseAdmissionError::RuntimeShutdown)?
            .ok_or(LeaseAdmissionError::NoEligibleAccount)?;
        if context.cancel.is_cancelled() {
            return Err(LeaseAdmissionError::Cancelled);
        }
        let generation = GenerationState {
            pool_id: selection.pool_id,
            account_id: selection.account_id,
            selection_family: "codex".to_string(),
            generation: selection.generation,
            auth_session: selection.auth_session,
            allow_context_reuse: selection.allow_context_reuse,
            accepting: true,
        };
        match self.inner.try_admit(&context, &generation) {
            AdmissionAttempt::Admitted(admission) => Ok(admission),
            AdmissionAttempt::Draining => Err(LeaseAdmissionError::UnsupportedPooledPath),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test_accepting(account_id: &str, generation: u64) -> Self {
        Self::new(RuntimeLeaseAuthorityMode::HostOwned(HostOwnedLeaseState {
            generation: Some(GenerationState::for_test(account_id, generation, true)),
        }))
    }

    #[cfg(test)]
    pub(crate) fn for_test_draining(account_id: &str, generation: u64) -> Self {
        Self::new(RuntimeLeaseAuthorityMode::HostOwned(HostOwnedLeaseState {
            generation: Some(GenerationState::for_test(account_id, generation, false)),
        }))
    }

    #[cfg(test)]
    pub(crate) async fn acquire_request_lease_for_test(
        &self,
        context: LeaseRequestContext,
    ) -> Result<LeaseAdmission, LeaseAdmissionError> {
        self.acquire_request_lease(context).await
    }

    #[cfg(test)]
    pub(crate) async fn close_current_generation_for_test(&self) {
        self.close_current_generation().await;
    }

    #[cfg(test)]
    pub(crate) async fn install_replacement_for_test(&self, account_id: &str, generation: u64) {
        {
            let mut state = self.inner.state.lock().await;
            let RuntimeLeaseAuthorityMode::HostOwned(host_owned) = &mut state.mode else {
                return;
            };
            host_owned.generation = Some(GenerationState::for_test(account_id, generation, true));
        }
        self.inner.changed.notify_waiters();
    }

    #[cfg(test)]
    pub(crate) fn admitted_count_for_test(&self) -> usize {
        self.inner
            .admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .admissions
            .len()
    }
}

impl AuthorityInner {
    fn try_admit(
        self: &Arc<Self>,
        context: &LeaseRequestContext,
        generation: &GenerationState,
    ) -> AdmissionAttempt {
        let admission_id = Uuid::new_v4();
        {
            let mut admissions = self
                .admissions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match admissions.active_generation {
                Some(active_generation) if active_generation != generation.generation => {
                    return AdmissionAttempt::Draining;
                }
                Some(_) => {}
                None => {
                    admissions.active_generation = Some(generation.generation);
                }
            }
            admissions.admissions.insert(admission_id);
        }

        let inner = Arc::clone(self);
        let snapshot = LeaseSnapshot {
            admission_id,
            pool_id: generation.pool_id.clone(),
            account_id: generation.account_id.clone(),
            selection_family: generation.selection_family.clone(),
            generation: generation.generation,
            boundary: context.boundary,
            session_id: context.session_id.clone(),
            collaboration_tree_id: context.collaboration_tree_id.clone(),
            allow_context_reuse: generation.allow_context_reuse,
            auth_handle: LeaseAuthHandle::new(Arc::clone(&generation.auth_session)),
        };
        let release = Arc::new(move |released_admission_id| {
            inner.release_admission(released_admission_id);
        });

        AdmissionAttempt::Admitted(LeaseAdmission {
            snapshot,
            guard: LeaseAdmissionGuard::new(admission_id, release),
        })
    }

    fn release_admission(&self, admission_id: Uuid) {
        let drained = {
            let mut admissions = self
                .admissions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !admissions.admissions.remove(&admission_id) {
                return;
            }
            if admissions.admissions.is_empty() {
                admissions.active_generation = None;
                true
            } else {
                false
            }
        };
        if drained {
            self.changed.notify_waiters();
        }
    }
}

#[cfg(test)]
impl GenerationState {
    fn for_test(account_id: &str, generation: u64, accepting: bool) -> Self {
        Self {
            pool_id: "pool-main".to_string(),
            account_id: account_id.to_string(),
            selection_family: "codex".to_string(),
            generation,
            auth_session: Arc::new(TestLeaseScopedAuthSession::new(account_id, generation)),
            allow_context_reuse: true,
            accepting,
        }
    }
}

#[cfg(test)]
struct TestLeaseScopedAuthSession {
    binding: codex_login::auth::LeaseAuthBinding,
}

#[cfg(test)]
impl TestLeaseScopedAuthSession {
    fn new(account_id: &str, lease_epoch: u64) -> Self {
        Self {
            binding: codex_login::auth::LeaseAuthBinding {
                account_id: account_id.to_string(),
                backend_account_handle: account_id.to_string(),
                lease_epoch,
            },
        }
    }
}

#[cfg(test)]
impl LeaseScopedAuthSession for TestLeaseScopedAuthSession {
    fn leased_turn_auth(&self) -> anyhow::Result<codex_login::auth::LeasedTurnAuth> {
        Err(anyhow::anyhow!("test auth session does not mint auth"))
    }

    fn refresh_leased_turn_auth(&self) -> anyhow::Result<codex_login::auth::LeasedTurnAuth> {
        Err(anyhow::anyhow!("test auth session does not refresh auth"))
    }

    fn binding(&self) -> &codex_login::auth::LeaseAuthBinding {
        &self.binding
    }

    fn ensure_current(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
