use super::LeaseSnapshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionLeaseViewDecision {
    Continue,
    ResetRemoteContext,
}

#[derive(Debug, Default)]
pub(crate) struct SessionLeaseView {
    last_account_id: Option<String>,
}

impl SessionLeaseView {
    pub(crate) fn new() -> Self {
        Self {
            last_account_id: None,
        }
    }

    pub(crate) fn before_request(&mut self, snapshot: &LeaseSnapshot) -> SessionLeaseViewDecision {
        let reset_remote_context = self
            .last_account_id
            .as_deref()
            .is_some_and(|previous| previous != snapshot.account_id())
            && !snapshot.allow_context_reuse;
        self.last_account_id = Some(snapshot.account_id().to_string());
        if reset_remote_context {
            SessionLeaseViewDecision::ResetRemoteContext
        } else {
            SessionLeaseViewDecision::Continue
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self::new()
    }

    #[cfg(test)]
    pub(crate) fn before_request_for_test(
        &mut self,
        snapshot: &LeaseSnapshot,
    ) -> SessionLeaseViewDecision {
        self.before_request(snapshot)
    }
}
