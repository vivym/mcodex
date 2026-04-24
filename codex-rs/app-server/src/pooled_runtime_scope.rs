use crate::error_code::INVALID_REQUEST_ERROR_CODE;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_protocol::ThreadId;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) const POOLED_RUNTIME_ALREADY_LOADED: &str = "pooledRuntimeAlreadyLoaded";
pub(crate) const POOLED_RUNTIME_UNSUPPORTED_TRANSPORT: &str = "pooledRuntimeUnsupportedTransport";

#[derive(Default)]
pub(crate) struct PooledRuntimeScope {
    state: Mutex<PooledRuntimeScopeState>,
}

#[derive(Default)]
struct PooledRuntimeScopeState {
    current: Option<PooledRuntimeContext>,
    next_reservation_id: u64,
}

enum PooledRuntimeContext {
    Starting {
        reservation_id: u64,
    },
    Loaded {
        owner_thread_id: ThreadId,
        root_thread_id: ThreadId,
    },
}

impl PooledRuntimeScope {
    pub(crate) async fn reserve(
        self: &Arc<Self>,
    ) -> Result<PooledRuntimeReservation, JSONRPCErrorError> {
        let mut state = self.state.lock().await;
        if let Some(current) = &state.current {
            return Err(already_loaded_error(current));
        }

        let reservation_id = state.next_reservation_id;
        state.next_reservation_id = state.next_reservation_id.wrapping_add(1);
        state.current = Some(PooledRuntimeContext::Starting { reservation_id });

        Ok(PooledRuntimeReservation {
            scope: Arc::clone(self),
            reservation_id,
        })
    }

    pub(crate) async fn loaded_thread_id(&self) -> Option<ThreadId> {
        let state = self.state.lock().await;
        match state.current {
            Some(PooledRuntimeContext::Loaded {
                owner_thread_id, ..
            }) => Some(owner_thread_id),
            Some(PooledRuntimeContext::Starting { .. }) | None => None,
        }
    }

    pub(crate) async fn loaded_root_thread_id(&self) -> Option<ThreadId> {
        let state = self.state.lock().await;
        match state.current {
            Some(PooledRuntimeContext::Loaded { root_thread_id, .. }) => Some(root_thread_id),
            Some(PooledRuntimeContext::Starting { .. }) | None => None,
        }
    }

    pub(crate) async fn reject_if_occupied(&self) -> Result<(), JSONRPCErrorError> {
        let state = self.state.lock().await;
        if let Some(current) = &state.current {
            return Err(already_loaded_error(current));
        }
        Ok(())
    }

    pub(crate) async fn mark_thread_unloaded_or_transfer(
        &self,
        unloaded_thread_id: ThreadId,
        replacement_thread_id: Option<ThreadId>,
    ) {
        let mut state = self.state.lock().await;
        if let Some(PooledRuntimeContext::Loaded {
            owner_thread_id,
            root_thread_id,
        }) = state.current
            && owner_thread_id == unloaded_thread_id
        {
            state.current =
                replacement_thread_id.map(|owner_thread_id| PooledRuntimeContext::Loaded {
                    owner_thread_id,
                    root_thread_id,
                });
        }
    }

    async fn promote_reservation(&self, reservation_id: u64, thread_id: ThreadId) {
        let mut state = self.state.lock().await;
        if matches!(
            state.current,
            Some(PooledRuntimeContext::Starting {
                reservation_id: current_reservation_id
            }) if current_reservation_id == reservation_id
        ) {
            state.current = Some(PooledRuntimeContext::Loaded {
                owner_thread_id: thread_id,
                root_thread_id: thread_id,
            });
        }
    }

    async fn rollback_reservation(&self, reservation_id: u64) {
        let mut state = self.state.lock().await;
        if matches!(
            state.current,
            Some(PooledRuntimeContext::Starting {
                reservation_id: current_reservation_id
            }) if current_reservation_id == reservation_id
        ) {
            state.current = None;
        }
    }
}

#[must_use]
pub(crate) struct PooledRuntimeReservation {
    scope: Arc<PooledRuntimeScope>,
    reservation_id: u64,
}

impl PooledRuntimeReservation {
    pub(crate) async fn promote(self, thread_id: ThreadId) {
        self.scope
            .promote_reservation(self.reservation_id, thread_id)
            .await;
    }

    pub(crate) async fn rollback(self) {
        self.scope.rollback_reservation(self.reservation_id).await;
    }
}

pub(crate) fn unsupported_transport_error() -> JSONRPCErrorError {
    pooled_runtime_error(
        POOLED_RUNTIME_UNSUPPORTED_TRANSPORT,
        "pooled lease mode is only supported for stdio app-server",
    )
}

fn already_loaded_error(current: &PooledRuntimeContext) -> JSONRPCErrorError {
    let message = match current {
        PooledRuntimeContext::Starting { .. } => {
            "pooled runtime already has a top-level context starting".to_string()
        }
        PooledRuntimeContext::Loaded {
            owner_thread_id, ..
        } => {
            format!("pooled runtime already has loaded top-level thread {owner_thread_id}")
        }
    };
    pooled_runtime_error(POOLED_RUNTIME_ALREADY_LOADED, message)
}

fn pooled_runtime_error(error_code: &str, message: impl Into<String>) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: INVALID_REQUEST_ERROR_CODE,
        message: message.into(),
        data: Some(json!({ "errorCode": error_code })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unload_can_transfer_loaded_pooled_owner_to_live_descendant() {
        let scope = Arc::new(PooledRuntimeScope::default());
        let parent_thread_id = ThreadId::new();
        let child_thread_id = ThreadId::new();

        scope
            .reserve()
            .await
            .expect("reserve")
            .promote(parent_thread_id)
            .await;
        scope
            .mark_thread_unloaded_or_transfer(parent_thread_id, Some(child_thread_id))
            .await;

        pretty_assertions::assert_eq!(scope.loaded_thread_id().await, Some(child_thread_id));
        pretty_assertions::assert_eq!(scope.loaded_root_thread_id().await, Some(parent_thread_id));

        scope
            .mark_thread_unloaded_or_transfer(child_thread_id, None)
            .await;
        pretty_assertions::assert_eq!(scope.loaded_thread_id().await, None);
    }

    #[tokio::test]
    async fn unload_transfer_preserves_original_root_across_multiple_handoffs() {
        let scope = Arc::new(PooledRuntimeScope::default());
        let parent_thread_id = ThreadId::new();
        let first_child_thread_id = ThreadId::new();
        let second_child_thread_id = ThreadId::new();

        scope
            .reserve()
            .await
            .expect("reserve")
            .promote(parent_thread_id)
            .await;
        scope
            .mark_thread_unloaded_or_transfer(parent_thread_id, Some(first_child_thread_id))
            .await;
        scope
            .mark_thread_unloaded_or_transfer(first_child_thread_id, Some(second_child_thread_id))
            .await;

        pretty_assertions::assert_eq!(scope.loaded_thread_id().await, Some(second_child_thread_id));
        pretty_assertions::assert_eq!(scope.loaded_root_thread_id().await, Some(parent_thread_id));
    }
}
