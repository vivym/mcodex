use std::fmt;

use tokio::sync::watch;

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CollaborationTreeId(String);

#[allow(dead_code)]
impl CollaborationTreeId {
    pub(crate) fn root_for_session(session_id: &str) -> Self {
        Self(format!("session:{session_id}"))
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
}

#[allow(dead_code)]
impl CollaborationTreeBindingHandle {
    pub(crate) fn new(initial: CollaborationTreeId) -> Self {
        let (tx, rx) = watch::channel(initial);
        Self { tx, rx }
    }

    pub(crate) fn sender(&self) -> watch::Sender<CollaborationTreeId> {
        self.tx.clone()
    }

    pub(crate) fn receiver(&self) -> watch::Receiver<CollaborationTreeId> {
        self.rx.clone()
    }

    pub(crate) fn current(&self) -> CollaborationTreeId {
        self.rx.borrow().clone()
    }
}
