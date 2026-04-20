use codex_protocol::protocol::RateLimitSnapshot;

use super::LeaseSnapshot;
use super::RuntimeLeaseAuthority;

#[allow(dead_code)]
#[derive(Clone)]
pub(crate) struct LeaseRequestReporter {
    authority: RuntimeLeaseAuthority,
    snapshot: LeaseSnapshot,
}

#[allow(dead_code)]
impl LeaseRequestReporter {
    pub(crate) fn new(authority: RuntimeLeaseAuthority, snapshot: LeaseSnapshot) -> Self {
        Self {
            authority,
            snapshot,
        }
    }

    pub(crate) fn snapshot(&self) -> &LeaseSnapshot {
        &self.snapshot
    }

    pub(crate) async fn report_rate_limits(&self, rate_limits: &RateLimitSnapshot) {
        let _ = self
            .authority
            .report_rate_limits(&self.snapshot, rate_limits)
            .await;
    }

    pub(crate) async fn report_usage_limit_reached(&self) {
        let _ = self
            .authority
            .report_usage_limit_reached(&self.snapshot)
            .await;
    }

    pub(crate) async fn report_terminal_unauthorized(&self) {
        let _ = self
            .authority
            .report_terminal_unauthorized(&self.snapshot)
            .await;
    }
}
