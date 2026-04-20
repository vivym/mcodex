use std::fmt;
use std::sync::Arc;

use codex_login::auth::LeaseScopedAuthSession;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::collaboration_tree::CollaborationTreeId;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequestBoundaryKind {
    ResponsesHttp,
    ResponsesWebSocket,
    ResponsesWebSocketPrewarm,
    ResponsesCompact,
    Realtime,
    MemorySummary,
    BackgroundModelCall,
}

#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LeaseAdmissionError {
    Cancelled,
    NoEligibleAccount,
    NonPooled,
    RuntimeShutdown,
    UnsupportedPooledPath,
}

impl fmt::Display for LeaseAdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LeaseAdmissionError::Cancelled => f.write_str("lease admission cancelled"),
            LeaseAdmissionError::NoEligibleAccount => {
                f.write_str("no eligible pooled account available")
            }
            LeaseAdmissionError::NonPooled => {
                f.write_str("request admission requires a pooled runtime")
            }
            LeaseAdmissionError::RuntimeShutdown => {
                f.write_str("runtime lease authority is shutting down")
            }
            LeaseAdmissionError::UnsupportedPooledPath => {
                f.write_str("pooled request path is not supported by this authority mode")
            }
        }
    }
}

impl std::error::Error for LeaseAdmissionError {}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct LeaseRequestContext {
    pub(crate) boundary: RequestBoundaryKind,
    pub(crate) session_id: String,
    pub(crate) collaboration_tree_id: CollaborationTreeId,
    pub(crate) cancel: CancellationToken,
}

#[allow(dead_code)]
impl LeaseRequestContext {
    pub(crate) fn new(
        boundary: RequestBoundaryKind,
        session_id: String,
        collaboration_tree_id: CollaborationTreeId,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            boundary,
            session_id,
            collaboration_tree_id,
            cancel,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        boundary: RequestBoundaryKind,
        session_id: &str,
        collaboration_tree_id: CollaborationTreeId,
    ) -> Self {
        Self::new(
            boundary,
            session_id.to_string(),
            collaboration_tree_id,
            CancellationToken::new(),
        )
    }

    #[cfg(test)]
    pub(crate) fn for_test_with_cancel(
        boundary: RequestBoundaryKind,
        session_id: &str,
        collaboration_tree_id: CollaborationTreeId,
        cancel: CancellationToken,
    ) -> Self {
        Self::new(
            boundary,
            session_id.to_string(),
            collaboration_tree_id,
            cancel,
        )
    }
}

#[allow(dead_code)]
#[derive(Clone)]
pub(crate) struct LeaseAuthHandle {
    auth_session: Arc<dyn LeaseScopedAuthSession>,
}

impl fmt::Debug for LeaseAuthHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeaseAuthHandle").finish_non_exhaustive()
    }
}

#[allow(dead_code)]
impl LeaseAuthHandle {
    pub(crate) fn new(auth_session: Arc<dyn LeaseScopedAuthSession>) -> Self {
        Self { auth_session }
    }

    pub(crate) fn auth_session(&self) -> Arc<dyn LeaseScopedAuthSession> {
        Arc::clone(&self.auth_session)
    }

    pub(crate) fn auth_recovery(&self) -> crate::lease_auth::LeaseSessionAuthRecovery {
        crate::lease_auth::LeaseSessionAuthRecovery::new(self.auth_session())
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct LeaseSnapshot {
    pub(crate) admission_id: Uuid,
    pub(crate) pool_id: String,
    pub(crate) account_id: String,
    pub(crate) selection_family: String,
    pub(crate) generation: u64,
    pub(crate) boundary: RequestBoundaryKind,
    pub(crate) session_id: String,
    pub(crate) collaboration_tree_id: CollaborationTreeId,
    pub(crate) allow_context_reuse: bool,
    pub(crate) auth_handle: LeaseAuthHandle,
}

#[allow(dead_code)]
impl LeaseSnapshot {
    pub(crate) fn account_id(&self) -> &str {
        &self.account_id
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

#[allow(dead_code)]
pub(crate) struct LeaseAdmission {
    pub(crate) snapshot: LeaseSnapshot,
    pub(crate) guard: LeaseAdmissionGuard,
}

impl fmt::Debug for LeaseAdmission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeaseAdmission")
            .field("snapshot", &self.snapshot)
            .field("guard", &self.guard)
            .finish()
    }
}

#[allow(dead_code)]
pub(crate) struct LeaseAdmissionGuard {
    admission_id: Uuid,
    release: Option<Arc<dyn Fn(Uuid) + Send + Sync>>,
}

impl fmt::Debug for LeaseAdmissionGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeaseAdmissionGuard")
            .field("admission_id", &self.admission_id)
            .field("released", &self.release.is_none())
            .finish()
    }
}

#[allow(dead_code)]
impl LeaseAdmissionGuard {
    pub(crate) fn new(admission_id: Uuid, release: Arc<dyn Fn(Uuid) + Send + Sync>) -> Self {
        Self {
            admission_id,
            release: Some(release),
        }
    }
}

impl Drop for LeaseAdmissionGuard {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release(self.admission_id);
        }
    }
}
