use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;

/// Inputs for a single soft-pressure observation against the active lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProactiveSwitchObservation {
    pub lease_acquired_at: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
    pub min_switch_interval: Duration,
}

/// Live-only diagnostics for whether proactive switching is currently pending.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProactiveSwitchSnapshot {
    pub pending: bool,
    pub suppressed: bool,
    pub allowed_at: Option<DateTime<Utc>>,
}

/// Immediate result of observing a new soft-pressure signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProactiveSwitchOutcome {
    NoAction,
    Suppressed { allowed_at: DateTime<Utc> },
    RotateOnNextTurn,
}

/// Turn-time decision derived from any remembered proactive-switch state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProactiveSwitchTurnDecision {
    KeepCurrentLease,
    /// Reserved for later manager wiring that may promote fresh pressure into
    /// a concrete turn-time rotation decision.
    RotateAwayFromActive,
}

/// Runtime-local soft-pressure state bound to the current active lease only.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProactiveSwitchState {
    pending_allowed_at: Option<DateTime<Utc>>,
}

impl ProactiveSwitchState {
    /// Clears any remembered soft-pressure state when the active lease changes.
    pub fn reset(&mut self) {
        self.pending_allowed_at = None;
    }

    pub fn observe_soft_pressure(
        &mut self,
        observation: ProactiveSwitchObservation,
    ) -> ProactiveSwitchOutcome {
        if observation.min_switch_interval < Duration::zero() {
            return ProactiveSwitchOutcome::NoAction;
        }
        if observation.observed_at < observation.lease_acquired_at {
            return ProactiveSwitchOutcome::NoAction;
        }

        let allowed_at = observation.lease_acquired_at + observation.min_switch_interval;
        if observation.observed_at >= allowed_at {
            self.pending_allowed_at = None;
            return ProactiveSwitchOutcome::RotateOnNextTurn;
        }

        self.pending_allowed_at = Some(allowed_at);
        ProactiveSwitchOutcome::Suppressed { allowed_at }
    }

    pub fn revalidate_before_turn(&mut self, now: DateTime<Utc>) -> ProactiveSwitchTurnDecision {
        if self
            .pending_allowed_at
            .is_some_and(|allowed_at| allowed_at <= now)
        {
            self.pending_allowed_at = None;
        }

        ProactiveSwitchTurnDecision::KeepCurrentLease
    }

    pub fn snapshot(&mut self, now: DateTime<Utc>) -> ProactiveSwitchSnapshot {
        let Some(allowed_at) = self.pending_allowed_at else {
            return ProactiveSwitchSnapshot::default();
        };
        if now >= allowed_at {
            self.pending_allowed_at = None;
            return ProactiveSwitchSnapshot::default();
        }

        ProactiveSwitchSnapshot {
            pending: true,
            suppressed: true,
            allowed_at: Some(allowed_at),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProactiveSwitchObservation;
    use super::ProactiveSwitchOutcome;
    use super::ProactiveSwitchSnapshot;
    use super::ProactiveSwitchState;
    use super::ProactiveSwitchTurnDecision;
    use chrono::Duration;
    use chrono::TimeZone;
    use chrono::Utc;
    use pretty_assertions::assert_eq;

    #[test]
    fn soft_pressure_is_suppressed_before_min_switch_interval() {
        let acquired_at = Utc.with_ymd_and_hms(2026, 4, 16, 9, 0, 0).unwrap();
        let observed_at = acquired_at + Duration::minutes(2);
        let mut state = ProactiveSwitchState::default();

        let outcome = state.observe_soft_pressure(ProactiveSwitchObservation {
            lease_acquired_at: acquired_at,
            observed_at,
            min_switch_interval: Duration::minutes(10),
        });

        assert_eq!(
            outcome,
            ProactiveSwitchOutcome::Suppressed {
                allowed_at: acquired_at + Duration::minutes(10),
            }
        );
        assert_eq!(
            state.snapshot(acquired_at + Duration::minutes(3)),
            ProactiveSwitchSnapshot {
                pending: true,
                suppressed: true,
                allowed_at: Some(acquired_at + Duration::minutes(10)),
            }
        );
    }

    #[test]
    fn stale_soft_pressure_expires_when_window_opens_without_forcing_rotation() {
        let acquired_at = Utc.with_ymd_and_hms(2026, 4, 16, 9, 0, 0).unwrap();
        let observed_at = acquired_at + Duration::minutes(2);
        let mut state = ProactiveSwitchState::default();
        state.observe_soft_pressure(ProactiveSwitchObservation {
            lease_acquired_at: acquired_at,
            observed_at,
            min_switch_interval: Duration::minutes(10),
        });

        let expired = state.revalidate_before_turn(acquired_at + Duration::minutes(11));

        assert_eq!(expired, ProactiveSwitchTurnDecision::KeepCurrentLease);
        assert_eq!(
            state.snapshot(acquired_at + Duration::minutes(11)),
            ProactiveSwitchSnapshot::default()
        );
        assert_eq!(state, ProactiveSwitchState::default());
    }

    #[test]
    fn stale_soft_pressure_snapshot_clears_after_window_opens_without_revalidation() {
        let acquired_at = Utc.with_ymd_and_hms(2026, 4, 16, 9, 0, 0).unwrap();
        let observed_at = acquired_at + Duration::minutes(2);
        let mut state = ProactiveSwitchState::default();
        state.observe_soft_pressure(ProactiveSwitchObservation {
            lease_acquired_at: acquired_at,
            observed_at,
            min_switch_interval: Duration::minutes(10),
        });

        assert_eq!(
            state.snapshot(acquired_at + Duration::minutes(11)),
            ProactiveSwitchSnapshot::default()
        );
        assert_eq!(state, ProactiveSwitchState::default());
    }

    #[test]
    fn reset_clears_pending_pressure_for_a_new_lease() {
        let acquired_at = Utc.with_ymd_and_hms(2026, 4, 16, 9, 0, 0).unwrap();
        let observed_at = acquired_at + Duration::minutes(2);
        let mut state = ProactiveSwitchState::default();
        state.observe_soft_pressure(ProactiveSwitchObservation {
            lease_acquired_at: acquired_at,
            observed_at,
            min_switch_interval: Duration::minutes(10),
        });

        state.reset();

        assert_eq!(
            state.snapshot(acquired_at + Duration::minutes(3)),
            ProactiveSwitchSnapshot::default()
        );
        assert_eq!(state, ProactiveSwitchState::default());
    }

    #[test]
    fn negative_min_switch_interval_is_ignored() {
        let acquired_at = Utc.with_ymd_and_hms(2026, 4, 16, 9, 0, 0).unwrap();
        let observed_at = acquired_at + Duration::minutes(2);
        let mut state = ProactiveSwitchState::default();

        let outcome = state.observe_soft_pressure(ProactiveSwitchObservation {
            lease_acquired_at: acquired_at,
            observed_at,
            min_switch_interval: Duration::minutes(-1),
        });

        assert_eq!(outcome, ProactiveSwitchOutcome::NoAction);
        assert_eq!(state, ProactiveSwitchState::default());
    }

    #[test]
    fn fresh_soft_pressure_after_window_requests_rotation() {
        let acquired_at = Utc.with_ymd_and_hms(2026, 4, 16, 9, 0, 0).unwrap();
        let observed_at = acquired_at + Duration::minutes(12);
        let mut state = ProactiveSwitchState::default();

        let outcome = state.observe_soft_pressure(ProactiveSwitchObservation {
            lease_acquired_at: acquired_at,
            observed_at,
            min_switch_interval: Duration::minutes(10),
        });

        assert_eq!(outcome, ProactiveSwitchOutcome::RotateOnNextTurn);
    }
}
