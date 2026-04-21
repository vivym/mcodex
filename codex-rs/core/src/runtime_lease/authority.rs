use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use codex_login::auth::LeaseScopedAuthSession;
use codex_protocol::protocol::RateLimitSnapshot;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use uuid::Uuid;

use crate::state::AccountLeaseRuntimeSnapshot;
use crate::state::BridgedTurnPreview;

use super::admission::LeaseAdmission;
use super::admission::LeaseAdmissionError;
use super::admission::LeaseAdmissionGuard;
use super::admission::LeaseAuthHandle;
use super::admission::LeaseRequestContext;
use super::admission::LeaseSnapshot;
use super::admission::RequestBoundaryKind;
use super::collaboration_tree::CollaborationTreeMembership;
use super::collaboration_tree::CollaborationTreeRegistry;

#[allow(dead_code)]
#[derive(Clone)]
pub(crate) struct RuntimeLeaseAuthority {
    inner: Arc<AuthorityInner>,
}

struct AuthorityInner {
    state: Mutex<AuthorityState>,
    collaboration_registry: StdMutex<Arc<CollaborationTreeRegistry>>,
    admissions: StdMutex<AdmissionTrackerState>,
    recorded_boundaries: StdMutex<Vec<RequestBoundaryKind>>,
    changed: Notify,
}

struct AuthorityState {
    mode: RuntimeLeaseAuthorityMode,
}

#[allow(dead_code)]
enum RuntimeLeaseAuthorityMode {
    OwnedManager(Arc<Mutex<crate::state::AccountPoolManager>>),
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
    Admitted(Box<LeaseAdmission>),
    Draining,
}

enum AdmissionPreview {
    Compatible,
    Draining,
}

struct ManagerLeaseHeartbeatGuard {
    cancellation_token: CancellationToken,
    task: JoinHandle<()>,
}

impl Drop for ManagerLeaseHeartbeatGuard {
    fn drop(&mut self) {
        self.cancellation_token.cancel();
        self.task.abort();
    }
}

fn start_manager_heartbeat(
    manager: Arc<Mutex<crate::state::AccountPoolManager>>,
    heartbeat_interval: std::time::Duration,
    cancellation_token: &CancellationToken,
) -> ManagerLeaseHeartbeatGuard {
    let heartbeat_cancellation_token = cancellation_token.child_token();
    let heartbeat_task_cancellation = heartbeat_cancellation_token.clone();
    let task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(heartbeat_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            tokio::select! {
                () = heartbeat_task_cancellation.cancelled() => break,
                _ = ticker.tick() => {
                    let mut manager = manager.lock().await;
                    if let Err(err) = manager.renew_active_lease().await {
                        warn!("failed to renew runtime-lease manager heartbeat: {err:#}");
                    }
                }
            }
        }
    });
    ManagerLeaseHeartbeatGuard {
        cancellation_token: heartbeat_cancellation_token,
        task,
    }
}

#[allow(dead_code)]
impl RuntimeLeaseAuthority {
    pub(crate) fn owned_manager(manager: Arc<Mutex<crate::state::AccountPoolManager>>) -> Self {
        Self::new(RuntimeLeaseAuthorityMode::OwnedManager(manager))
    }

    fn new(mode: RuntimeLeaseAuthorityMode) -> Self {
        Self {
            inner: Arc::new(AuthorityInner {
                state: Mutex::new(AuthorityState { mode }),
                collaboration_registry: StdMutex::new(Arc::new(
                    CollaborationTreeRegistry::default(),
                )),
                admissions: StdMutex::new(AdmissionTrackerState {
                    active_generation: None,
                    admissions: HashSet::new(),
                }),
                recorded_boundaries: StdMutex::new(Vec::new()),
                changed: Notify::new(),
            }),
        }
    }

    pub(crate) fn set_collaboration_registry(&self, registry: Arc<CollaborationTreeRegistry>) {
        *self
            .inner
            .collaboration_registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = registry;
    }

    fn collaboration_registry(&self) -> Arc<CollaborationTreeRegistry> {
        Arc::clone(
            &self
                .inner
                .collaboration_registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
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

            let manager = {
                let state = self.inner.state.lock().await;
                match &state.mode {
                    RuntimeLeaseAuthorityMode::OwnedManager(manager) => Some(Arc::clone(manager)),
                    RuntimeLeaseAuthorityMode::HostOwned(host_owned) => {
                        if let Some(generation) = host_owned.generation.as_ref()
                            && generation.accepting
                        {
                            let membership = self.register_collaboration_membership(&context);
                            match self.inner.try_admit(
                                &context,
                                generation,
                                vec![Box::new(membership)],
                            ) {
                                AdmissionAttempt::Admitted(admission) => {
                                    return Ok(*admission);
                                }
                                AdmissionAttempt::Draining => {}
                            }
                        }
                        None
                    }
                }
            };

            if let Some(manager) = manager {
                return self.acquire_from_manager(context, manager).await;
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
                RuntimeLeaseAuthorityMode::OwnedManager(manager) => Some(Arc::clone(manager)),
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
                    snapshot.selection_family.as_str(),
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
                RuntimeLeaseAuthorityMode::OwnedManager(manager) => Some(Arc::clone(manager)),
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
                    snapshot.selection_family.as_str(),
                )
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn report_terminal_unauthorized(
        &self,
        snapshot: &LeaseSnapshot,
    ) -> anyhow::Result<()> {
        let (manager, invalidated_host_owned) = {
            let mut state = self.inner.state.lock().await;
            match &mut state.mode {
                RuntimeLeaseAuthorityMode::OwnedManager(manager) => {
                    (Some(Arc::clone(manager)), false)
                }
                RuntimeLeaseAuthorityMode::HostOwned(host_owned) => {
                    let should_invalidate =
                        host_owned.generation.as_ref().is_some_and(|generation| {
                            generation.generation == snapshot.generation
                                && generation.account_id == snapshot.account_id
                                && generation.pool_id == snapshot.pool_id
                        });
                    if should_invalidate {
                        host_owned.generation = None;
                    }
                    (None, should_invalidate)
                }
            }
        };

        if invalidated_host_owned {
            self.inner.changed.notify_waiters();
        }

        let report_result = if let Some(manager) = manager {
            manager
                .lock()
                .await
                .report_unauthorized_for_generation(
                    snapshot.generation,
                    snapshot.account_id.as_str(),
                    snapshot.pool_id.as_str(),
                )
                .await
        } else {
            Ok(invalidated_host_owned)
        };
        if report_result.as_ref().copied().unwrap_or(true) {
            self.collaboration_registry()
                .cancel_tree(&snapshot.collaboration_tree_id);
        }
        report_result.map(|_| ())
    }

    pub(crate) async fn runtime_snapshot(&self) -> AccountLeaseRuntimeSnapshot {
        let manager = {
            let state = self.inner.state.lock().await;
            match &state.mode {
                RuntimeLeaseAuthorityMode::OwnedManager(manager) => Some(Arc::clone(manager)),
                RuntimeLeaseAuthorityMode::HostOwned(host_owned) => {
                    return runtime_snapshot_for_generation(host_owned.generation.clone());
                }
            }
        };
        let Some(manager) = manager else {
            return runtime_snapshot_for_generation(None);
        };
        let snapshot_seed = {
            let manager = manager.lock().await;
            manager.snapshot_seed()
        };
        snapshot_seed.snapshot().await
    }

    pub(crate) async fn release_for_shutdown(&self) -> anyhow::Result<()> {
        let manager = {
            let mut state = self.inner.state.lock().await;
            match &mut state.mode {
                RuntimeLeaseAuthorityMode::OwnedManager(manager) => Some(Arc::clone(manager)),
                RuntimeLeaseAuthorityMode::HostOwned(host_owned) => {
                    host_owned.generation = None;
                    None
                }
            }
        };
        if let Some(manager) = manager {
            manager.lock().await.release_for_shutdown().await?;
        } else {
            self.inner.changed.notify_waiters();
        }
        Ok(())
    }

    async fn acquire_from_manager(
        &self,
        context: LeaseRequestContext,
        manager: Arc<Mutex<crate::state::AccountPoolManager>>,
    ) -> Result<LeaseAdmission, LeaseAdmissionError> {
        loop {
            let changed = self.inner.changed.notified();
            let manager_for_lock = Arc::clone(&manager);
            let mut manager_guard = tokio::select! {
                () = context.cancel.cancelled() => return Err(LeaseAdmissionError::Cancelled),
                manager_guard = manager_for_lock.lock_owned() => manager_guard,
            };
            if context.cancel.is_cancelled() {
                return Err(LeaseAdmissionError::Cancelled);
            }
            let heartbeat_interval = manager_guard.heartbeat_interval();
            let preview = manager_guard
                .preview_next_bridged_turn()
                .await
                .map_err(|_| LeaseAdmissionError::RuntimeShutdown)?
                .ok_or(LeaseAdmissionError::NoEligibleAccount)?;
            if context.cancel.is_cancelled() {
                return Err(LeaseAdmissionError::Cancelled);
            }
            let previewed_generation = match &preview {
                BridgedTurnPreview::ReuseCurrent(preview) | BridgedTurnPreview::Rotate(preview) => {
                    preview.generation
                }
            };
            if matches!(
                self.inner.preview_admission(previewed_generation),
                AdmissionPreview::Draining
            ) {
                drop(manager_guard);
                tokio::select! {
                    () = context.cancel.cancelled() => return Err(LeaseAdmissionError::Cancelled),
                    () = changed => continue,
                }
            }
            let selection = manager_guard
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
                selection_family: selection.selection_family,
                generation: selection.generation,
                auth_session: selection.auth_session,
                allow_context_reuse: selection.allow_context_reuse,
                accepting: true,
            };
            drop(manager_guard);
            if context.cancel.is_cancelled() {
                return Err(LeaseAdmissionError::Cancelled);
            }
            let heartbeat_guard =
                start_manager_heartbeat(Arc::clone(&manager), heartbeat_interval, &context.cancel);
            let membership = self.register_collaboration_membership(&context);
            match self.inner.try_admit(
                &context,
                &generation,
                vec![Box::new(heartbeat_guard), Box::new(membership)],
            ) {
                AdmissionAttempt::Admitted(admission) => return Ok(*admission),
                AdmissionAttempt::Draining => {
                    tokio::select! {
                        () = context.cancel.cancelled() => return Err(LeaseAdmissionError::Cancelled),
                        () = changed => {}
                    }
                }
            }
        }
    }

    fn register_collaboration_membership(
        &self,
        context: &LeaseRequestContext,
    ) -> CollaborationTreeMembership {
        self.collaboration_registry().register_member(
            context.collaboration_tree_id.clone(),
            format!("{}:{}", context.session_id, Uuid::now_v7()),
            context.cancel.clone(),
        )
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

    #[cfg(test)]
    pub(crate) fn recorded_boundaries_for_test(&self) -> Vec<RequestBoundaryKind> {
        self.inner
            .recorded_boundaries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[cfg(test)]
    pub(crate) fn ptr_eq_for_test(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl AuthorityInner {
    fn preview_admission(&self, generation: u64) -> AdmissionPreview {
        let admissions = self
            .admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match admissions.active_generation {
            Some(active_generation) if active_generation != generation => {
                AdmissionPreview::Draining
            }
            Some(_) | None => AdmissionPreview::Compatible,
        }
    }

    fn try_admit(
        self: &Arc<Self>,
        context: &LeaseRequestContext,
        generation: &GenerationState,
        drop_guards: Vec<Box<dyn Send + Sync>>,
    ) -> AdmissionAttempt {
        if matches!(
            self.preview_admission(generation.generation),
            AdmissionPreview::Draining
        ) {
            return AdmissionAttempt::Draining;
        }

        let admission_id = Uuid::new_v4();
        {
            let mut admissions = self
                .admissions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if admissions.active_generation.is_none() {
                admissions.active_generation = Some(generation.generation);
            }
            admissions.admissions.insert(admission_id);
        }
        self.recorded_boundaries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(context.boundary);

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

        AdmissionAttempt::Admitted(Box::new(LeaseAdmission {
            snapshot,
            guard: LeaseAdmissionGuard::new(admission_id, release, drop_guards),
        }))
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
        Ok(codex_login::auth::LeasedTurnAuth::chatgpt(
            self.binding.account_id.clone(),
            fake_access_token(&self.binding.account_id),
        ))
    }

    fn refresh_leased_turn_auth(&self) -> anyhow::Result<codex_login::auth::LeasedTurnAuth> {
        self.leased_turn_auth()
    }

    fn binding(&self) -> &codex_login::auth::LeaseAuthBinding {
        &self.binding
    }

    fn ensure_current(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
fn fake_access_token(chatgpt_account_id: &str) -> String {
    use base64::Engine as _;

    let header = serde_json::json!({
        "alg": "none",
        "typ": "JWT",
    });
    let payload = serde_json::json!({
        "email": "user@example.com",
        "email_verified": true,
        "https://api.openai.com/auth": {
            "chatgpt_plan_type": "pro",
            "chatgpt_user_id": "user-12345",
            "chatgpt_account_id": chatgpt_account_id,
        },
    });
    let b64 = |value: serde_json::Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&value).expect("serialize fake JWT part"))
    };
    format!("{}.{}.sig", b64(header), b64(payload))
}

fn runtime_snapshot_for_generation(
    generation: Option<GenerationState>,
) -> AccountLeaseRuntimeSnapshot {
    let Some(generation) = generation else {
        return AccountLeaseRuntimeSnapshot {
            active: false,
            suppressed: false,
            account_id: None,
            pool_id: None,
            lease_id: None,
            lease_epoch: None,
            lease_acquired_at: None,
            health_state: None,
            switch_reason: None,
            suppression_reason: None,
            transport_reset_generation: None,
            last_remote_context_reset_turn_id: None,
            min_switch_interval_secs: None,
            proactive_switch_pending: None,
            proactive_switch_suppressed: None,
            proactive_switch_allowed_at: None,
            next_eligible_at: None,
        };
    };
    AccountLeaseRuntimeSnapshot {
        active: true,
        suppressed: false,
        account_id: Some(generation.account_id),
        pool_id: Some(generation.pool_id),
        lease_id: None,
        lease_epoch: Some(generation.auth_session.binding().lease_epoch as i64),
        lease_acquired_at: None,
        health_state: None,
        switch_reason: None,
        suppression_reason: None,
        transport_reset_generation: None,
        last_remote_context_reset_turn_id: None,
        min_switch_interval_secs: None,
        proactive_switch_pending: None,
        proactive_switch_suppressed: None,
        proactive_switch_allowed_at: None,
        next_eligible_at: None,
    }
}
