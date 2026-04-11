use crate::types::LeasedAccount;
use chrono::DateTime;
use chrono::Utc;
use codex_state::AccountHealthEvent;
use codex_state::AccountHealthState;

pub(crate) enum LeaseHealthEvent {
    RateLimited { observed_at: DateTime<Utc> },
    Unauthorized { observed_at: DateTime<Utc> },
}

impl LeaseHealthEvent {
    pub(crate) fn into_account_health_event(
        self,
        lease: &LeasedAccount,
        sequence_number: i64,
    ) -> AccountHealthEvent {
        let (health_state, observed_at) = match self {
            Self::RateLimited { observed_at } => (AccountHealthState::RateLimited, observed_at),
            Self::Unauthorized { observed_at } => (AccountHealthState::Unauthorized, observed_at),
        };
        AccountHealthEvent {
            account_id: lease.account_id().to_string(),
            pool_id: lease.pool_id().to_string(),
            health_state,
            sequence_number,
            observed_at,
        }
    }
}
