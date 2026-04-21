use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::host::RuntimeLeaseHostId;

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CollaborationTreeId(String);

#[allow(dead_code)]
impl CollaborationTreeId {
    pub(crate) fn root_for_session(session_id: &str) -> Self {
        Self(format!("session:{session_id}"))
    }

    pub(crate) fn for_turn(turn_id: &str) -> Self {
        Self(format!("turn:{turn_id}"))
    }

    pub(crate) fn synthetic_background_tree_id(
        runtime_host_id: &RuntimeLeaseHostId,
        invocation_id: Uuid,
    ) -> Self {
        Self(format!("background:{runtime_host_id}:{invocation_id}"))
    }

    pub(crate) fn synthetic_local_background_tree_id(invocation_id: Uuid) -> Self {
        Self(format!("background:local:{invocation_id}"))
    }

    #[cfg(test)]
    pub(crate) fn for_test(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl fmt::Display for CollaborationTreeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct CollaborationTreeBindingHandle {
    tx: watch::Sender<CollaborationTreeId>,
    rx: watch::Receiver<CollaborationTreeId>,
    state: Mutex<BindingState>,
}

#[derive(Debug)]
struct BindingState {
    base_tree_id: CollaborationTreeId,
    bindings: Vec<BindingEntry>,
}

#[derive(Debug)]
struct BindingEntry {
    id: Uuid,
    tree_id: CollaborationTreeId,
}

impl BindingState {
    fn current(&self) -> &CollaborationTreeId {
        self.bindings
            .last()
            .map(|binding| &binding.tree_id)
            .unwrap_or(&self.base_tree_id)
    }
}

#[allow(dead_code)]
impl CollaborationTreeBindingHandle {
    pub(crate) fn new(initial: CollaborationTreeId) -> Self {
        let (tx, rx) = watch::channel(initial.clone());
        Self {
            tx,
            rx,
            state: Mutex::new(BindingState {
                base_tree_id: initial,
                bindings: Vec::new(),
            }),
        }
    }

    pub(crate) fn sender(&self) -> watch::Sender<CollaborationTreeId> {
        self.tx.clone()
    }

    pub(crate) fn receiver(&self) -> watch::Receiver<CollaborationTreeId> {
        self.rx.clone()
    }

    pub(crate) fn current(&self) -> CollaborationTreeId {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .current()
            .clone()
    }

    pub(crate) fn set_current(&self, tree_id: CollaborationTreeId) -> CollaborationTreeId {
        let (previous_tree_id, current_tree_id) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous_tree_id = state.current().clone();
            state.base_tree_id = tree_id;
            let current_tree_id = state.current().clone();
            (previous_tree_id, current_tree_id)
        };
        self.tx.send_replace(current_tree_id);
        previous_tree_id
    }

    fn push_binding(&self, tree_id: CollaborationTreeId) -> Uuid {
        let binding_id = Uuid::now_v7();
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.bindings.push(BindingEntry {
                id: binding_id,
                tree_id: tree_id.clone(),
            });
        }
        self.tx.send_replace(tree_id);
        binding_id
    }

    fn drop_binding(&self, binding_id: Uuid) {
        let (previous_tree_id, current_tree_id) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous_tree_id = state.current().clone();
            state.bindings.retain(|binding| binding.id != binding_id);
            let current_tree_id = state.current().clone();
            (previous_tree_id, current_tree_id)
        };
        if current_tree_id != previous_tree_id {
            self.tx.send_replace(current_tree_id);
        }
    }
}

pub(crate) struct CollaborationTreeBinding {
    _membership: Option<CollaborationTreeMembership>,
    binding_id: Uuid,
    handle: Arc<CollaborationTreeBindingHandle>,
}

impl CollaborationTreeBinding {
    pub(crate) fn new(
        handle: Arc<CollaborationTreeBindingHandle>,
        tree_id: CollaborationTreeId,
        membership: Option<CollaborationTreeMembership>,
    ) -> Self {
        let binding_id = handle.push_binding(tree_id);
        Self {
            _membership: membership,
            binding_id,
            handle,
        }
    }
}

impl Drop for CollaborationTreeBinding {
    fn drop(&mut self) {
        self.handle.drop_binding(self.binding_id);
    }
}

pub(crate) struct CollaborationTreeMembership {
    registry: Arc<CollaborationTreeRegistry>,
    tree_id: CollaborationTreeId,
    member_id: String,
}

impl CollaborationTreeMembership {
    pub(crate) fn tree_id(&self) -> &CollaborationTreeId {
        &self.tree_id
    }
}

impl Drop for CollaborationTreeMembership {
    fn drop(&mut self) {
        self.registry
            .unregister_member(&self.tree_id, self.member_id.as_str());
    }
}

#[derive(Default)]
pub(crate) struct CollaborationTreeRegistry {
    inner: Mutex<RegistryState>,
}

#[derive(Default)]
struct RegistryState {
    members: HashMap<CollaborationTreeId, HashMap<String, CancellationToken>>,
}

impl CollaborationTreeRegistry {
    pub(crate) fn register_member(
        self: &Arc<Self>,
        tree_id: CollaborationTreeId,
        member_id: String,
        cancellation_token: CancellationToken,
    ) -> CollaborationTreeMembership {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .members
            .entry(tree_id.clone())
            .or_default()
            .insert(member_id.clone(), cancellation_token);
        CollaborationTreeMembership {
            registry: Arc::clone(self),
            tree_id,
            member_id,
        }
    }

    pub(crate) fn cancel_tree(&self, tree_id: &CollaborationTreeId) {
        let tokens = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .members
            .get(tree_id)
            .map(|members| members.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for token in tokens {
            token.cancel();
        }
    }

    fn unregister_member(&self, tree_id: &CollaborationTreeId, member_id: &str) {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(members) = state.members.get_mut(tree_id) else {
            return;
        };
        members.remove(member_id);
        if members.is_empty() {
            state.members.remove(tree_id);
        }
    }

    #[cfg(test)]
    pub(crate) fn member_count_for_test(&self, tree_id: &CollaborationTreeId) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .members
            .get(tree_id)
            .map(HashMap::len)
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub(crate) fn member_ids_for_test(&self, tree_id: &CollaborationTreeId) -> Vec<String> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .members
            .get(tree_id)
            .map(|members| members.keys().cloned().collect())
            .unwrap_or_default()
    }
}
