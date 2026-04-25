use super::AccountOperationalState;
use super::AccountPoolAccount;
use super::AccountPoolAccountsPage;
use super::AccountPoolDiagnostics;
use super::AccountPoolDiagnosticsSeverity;
use super::AccountPoolDiagnosticsStatus;
use super::AccountPoolEvent;
use super::AccountPoolEventType;
use super::AccountPoolEventsPage;
use super::AccountPoolIssue;
use super::AccountPoolLease;
use super::AccountPoolQuota;
use super::AccountPoolQuotaFamily;
use super::AccountPoolQuotaWindow;
use super::AccountPoolReasonCode;
use super::AccountPoolSelection;
use super::AccountPoolSnapshot;
use super::AccountPoolSummary;
use anyhow::anyhow;

impl TryFrom<&str> for AccountOperationalState {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> anyhow::Result<Self> {
        match value {
            "available" => Ok(Self::Available),
            "leased" => Ok(Self::Leased),
            "paused" => Ok(Self::Paused),
            "draining" => Ok(Self::Draining),
            "coolingDown" => Ok(Self::CoolingDown),
            "nearExhausted" => Ok(Self::NearExhausted),
            "exhausted" => Ok(Self::Exhausted),
            "error" => Ok(Self::Error),
            _ => Err(anyhow!("unknown account operational state: {value}")),
        }
    }
}

impl TryFrom<&str> for AccountPoolEventType {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> anyhow::Result<Self> {
        match value {
            "leaseAcquired" => Ok(Self::LeaseAcquired),
            "leaseRenewed" => Ok(Self::LeaseRenewed),
            "leaseReleased" => Ok(Self::LeaseReleased),
            "leaseAcquireFailed" => Ok(Self::LeaseAcquireFailed),
            "proactiveSwitchSelected" => Ok(Self::ProactiveSwitchSelected),
            "proactiveSwitchSuppressed" => Ok(Self::ProactiveSwitchSuppressed),
            "quotaObserved" => Ok(Self::QuotaObserved),
            "quotaNearExhausted" => Ok(Self::QuotaNearExhausted),
            "quotaExhausted" => Ok(Self::QuotaExhausted),
            "accountPaused" => Ok(Self::AccountPaused),
            "accountResumed" => Ok(Self::AccountResumed),
            "accountDrainingStarted" => Ok(Self::AccountDrainingStarted),
            "accountDrainingCleared" => Ok(Self::AccountDrainingCleared),
            "authFailed" => Ok(Self::AuthFailed),
            "cooldownStarted" => Ok(Self::CooldownStarted),
            "cooldownCleared" => Ok(Self::CooldownCleared),
            _ => Err(anyhow!("unknown account pool event type: {value}")),
        }
    }
}

impl TryFrom<&str> for AccountPoolDiagnosticsStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> anyhow::Result<Self> {
        match value {
            "healthy" => Ok(Self::Healthy),
            "degraded" => Ok(Self::Degraded),
            "blocked" => Ok(Self::Blocked),
            _ => Err(anyhow!("unknown diagnostics status: {value}")),
        }
    }
}

impl TryFrom<&str> for AccountPoolDiagnosticsSeverity {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> anyhow::Result<Self> {
        match value {
            "info" => Ok(Self::Info),
            "warning" => Ok(Self::Warning),
            "error" => Ok(Self::Error),
            _ => Err(anyhow!("unknown diagnostics severity: {value}")),
        }
    }
}

impl From<codex_state::AccountPoolSummaryRecord> for AccountPoolSummary {
    fn from(value: codex_state::AccountPoolSummaryRecord) -> Self {
        Self {
            total_accounts: value.total_accounts,
            active_leases: value.active_leases,
            available_accounts: value.available_accounts,
            leased_accounts: value.leased_accounts,
            paused_accounts: value.paused_accounts,
            draining_accounts: value.draining_accounts,
            near_exhausted_accounts: value.near_exhausted_accounts,
            exhausted_accounts: value.exhausted_accounts,
            error_accounts: value.error_accounts,
        }
    }
}

impl From<codex_state::AccountPoolLeaseRecord> for AccountPoolLease {
    fn from(value: codex_state::AccountPoolLeaseRecord) -> Self {
        Self {
            lease_id: value.lease_id,
            lease_epoch: value.lease_epoch,
            holder_instance_id: value.holder_instance_id,
            acquired_at: value.acquired_at,
            renewed_at: value.renewed_at,
            expires_at: value.expires_at,
        }
    }
}

impl From<codex_state::AccountPoolQuotaRecord> for AccountPoolQuota {
    fn from(value: codex_state::AccountPoolQuotaRecord) -> Self {
        Self {
            remaining_percent: value.remaining_percent,
            resets_at: value.resets_at,
            observed_at: value.observed_at,
        }
    }
}

impl From<codex_state::AccountPoolQuotaWindowRecord> for AccountPoolQuotaWindow {
    fn from(value: codex_state::AccountPoolQuotaWindowRecord) -> Self {
        Self {
            used_percent: value.used_percent,
            resets_at: value.resets_at,
        }
    }
}

impl From<codex_state::AccountPoolQuotaFamilyRecord> for AccountPoolQuotaFamily {
    fn from(value: codex_state::AccountPoolQuotaFamilyRecord) -> Self {
        Self {
            limit_id: value.limit_id,
            primary: value.primary.into(),
            secondary: value.secondary.into(),
            exhausted_windows: value.exhausted_windows,
            predicted_blocked_until: value.predicted_blocked_until,
            next_probe_after: value.next_probe_after,
            observed_at: value.observed_at,
        }
    }
}

impl From<codex_state::AccountPoolSelectionRecord> for AccountPoolSelection {
    fn from(value: codex_state::AccountPoolSelectionRecord) -> Self {
        Self {
            eligible: value.eligible,
            next_eligible_at: value.next_eligible_at,
            preferred: value.preferred,
            suppressed: value.suppressed,
        }
    }
}

impl TryFrom<codex_state::AccountPoolSnapshotRecord> for AccountPoolSnapshot {
    type Error = anyhow::Error;

    fn try_from(value: codex_state::AccountPoolSnapshotRecord) -> anyhow::Result<Self> {
        Ok(Self {
            pool_id: value.pool_id,
            summary: value.summary.into(),
            refreshed_at: value.refreshed_at,
        })
    }
}

impl TryFrom<codex_state::AccountPoolAccountRecord> for AccountPoolAccount {
    type Error = anyhow::Error;

    fn try_from(value: codex_state::AccountPoolAccountRecord) -> anyhow::Result<Self> {
        Ok(Self {
            account_id: value.account_id,
            backend_account_ref: value.backend_account_ref,
            account_kind: value.account_kind,
            enabled: value.enabled,
            health_state: value.health_state,
            operational_state: value
                .operational_state
                .as_deref()
                .map(AccountOperationalState::try_from)
                .transpose()?,
            allocatable: value.allocatable,
            status_reason_code: value
                .status_reason_code
                .as_deref()
                .map(AccountPoolReasonCode::from_wire_value),
            status_message: value.status_message,
            current_lease: value.current_lease.map(Into::into),
            quota: value.quota.map(Into::into),
            quotas: value.quotas.into_iter().map(Into::into).collect(),
            selection: value.selection.map(Into::into),
            updated_at: value.updated_at,
        })
    }
}

impl TryFrom<codex_state::AccountPoolAccountsPage> for AccountPoolAccountsPage {
    type Error = anyhow::Error;

    fn try_from(value: codex_state::AccountPoolAccountsPage) -> anyhow::Result<Self> {
        let data = value
            .data
            .into_iter()
            .map(AccountPoolAccount::try_from)
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Self {
            data,
            next_cursor: value.next_cursor,
        })
    }
}

impl TryFrom<codex_state::AccountPoolEventRecord> for AccountPoolEvent {
    type Error = anyhow::Error;

    fn try_from(value: codex_state::AccountPoolEventRecord) -> anyhow::Result<Self> {
        Ok(Self {
            event_id: value.event_id,
            occurred_at: value.occurred_at,
            pool_id: value.pool_id,
            account_id: value.account_id,
            lease_id: value.lease_id,
            holder_instance_id: value.holder_instance_id,
            event_type: AccountPoolEventType::try_from(value.event_type.as_str())?,
            reason_code: value
                .reason_code
                .as_deref()
                .map(AccountPoolReasonCode::from_wire_value),
            message: value.message,
            details_json: value.details_json,
        })
    }
}

impl TryFrom<codex_state::AccountPoolEventsPage> for AccountPoolEventsPage {
    type Error = anyhow::Error;

    fn try_from(value: codex_state::AccountPoolEventsPage) -> anyhow::Result<Self> {
        let data = value
            .data
            .into_iter()
            .map(AccountPoolEvent::try_from)
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Self {
            data,
            next_cursor: value.next_cursor,
        })
    }
}

impl TryFrom<codex_state::AccountPoolIssueRecord> for AccountPoolIssue {
    type Error = anyhow::Error;

    fn try_from(value: codex_state::AccountPoolIssueRecord) -> anyhow::Result<Self> {
        Ok(Self {
            severity: AccountPoolDiagnosticsSeverity::try_from(value.severity.as_str())?,
            reason_code: AccountPoolReasonCode::from_wire_value(value.reason_code.as_str()),
            message: value.message,
            account_id: value.account_id,
            holder_instance_id: value.holder_instance_id,
            next_relevant_at: value.next_relevant_at,
        })
    }
}

impl TryFrom<codex_state::AccountPoolDiagnosticsRecord> for AccountPoolDiagnostics {
    type Error = anyhow::Error;

    fn try_from(value: codex_state::AccountPoolDiagnosticsRecord) -> anyhow::Result<Self> {
        let issues = value
            .issues
            .into_iter()
            .map(AccountPoolIssue::try_from)
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Self {
            pool_id: value.pool_id,
            generated_at: value.generated_at,
            status: AccountPoolDiagnosticsStatus::try_from(value.status.as_str())?,
            issues,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::AccountOperationalState;
    use super::AccountPoolAccount;
    use super::AccountPoolAccountsPage;
    use super::AccountPoolDiagnostics;
    use super::AccountPoolDiagnosticsSeverity;
    use super::AccountPoolDiagnosticsStatus;
    use super::AccountPoolEvent;
    use super::AccountPoolEventType;
    use super::AccountPoolEventsPage;
    use super::AccountPoolIssue;
    use super::AccountPoolLease;
    use super::AccountPoolQuota;
    use super::AccountPoolReasonCode;
    use super::AccountPoolSelection;
    use super::AccountPoolSnapshot;
    use super::AccountPoolSummary;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn snapshot_conversion_preserves_summary_fields() {
        let snapshot = codex_state::AccountPoolSnapshotRecord {
            pool_id: "team-main".to_string(),
            summary: codex_state::AccountPoolSummaryRecord {
                total_accounts: 4,
                active_leases: 1,
                available_accounts: Some(2),
                leased_accounts: Some(1),
                paused_accounts: Some(1),
                draining_accounts: None,
                near_exhausted_accounts: Some(1),
                exhausted_accounts: None,
                error_accounts: Some(1),
            },
            refreshed_at: timestamp(10),
        };

        let observed = expect_ok(AccountPoolSnapshot::try_from(snapshot));
        let expected = AccountPoolSnapshot {
            pool_id: "team-main".to_string(),
            summary: AccountPoolSummary {
                total_accounts: 4,
                active_leases: 1,
                available_accounts: Some(2),
                leased_accounts: Some(1),
                paused_accounts: Some(1),
                draining_accounts: None,
                near_exhausted_accounts: Some(1),
                exhausted_accounts: None,
                error_accounts: Some(1),
            },
            refreshed_at: timestamp(10),
        };

        assert_eq!(observed, expected);
    }

    #[test]
    fn account_conversion_preserves_fields_and_nested_records() {
        let account = codex_state::AccountPoolAccountRecord {
            account_id: "acct-1".to_string(),
            backend_account_ref: Some("backend-acct-1".to_string()),
            account_kind: "chatgpt".to_string(),
            enabled: true,
            health_state: Some("unauthorized".to_string()),
            operational_state: Some("error".to_string()),
            allocatable: Some(false),
            status_reason_code: Some("authFailure".to_string()),
            status_message: Some("token expired".to_string()),
            current_lease: Some(codex_state::AccountPoolLeaseRecord {
                lease_id: "lease-1".to_string(),
                lease_epoch: 7,
                holder_instance_id: "inst-a".to_string(),
                acquired_at: timestamp(11),
                renewed_at: timestamp(12),
                expires_at: timestamp(13),
            }),
            quota: Some(codex_state::AccountPoolQuotaRecord {
                remaining_percent: Some(12.5),
                resets_at: Some(timestamp(14)),
                observed_at: timestamp(15),
            }),
            quotas: vec![codex_state::AccountPoolQuotaFamilyRecord {
                limit_id: "codex".to_string(),
                primary: codex_state::AccountPoolQuotaWindowRecord {
                    used_percent: Some(87.5),
                    resets_at: Some(timestamp(14)),
                },
                secondary: codex_state::AccountPoolQuotaWindowRecord {
                    used_percent: None,
                    resets_at: None,
                },
                exhausted_windows: "primary".to_string(),
                predicted_blocked_until: Some(timestamp(14)),
                next_probe_after: Some(timestamp(13)),
                observed_at: timestamp(15),
            }],
            selection: Some(codex_state::AccountPoolSelectionRecord {
                eligible: false,
                next_eligible_at: Some(timestamp(16)),
                preferred: false,
                suppressed: true,
            }),
            updated_at: timestamp(17),
        };

        let observed = expect_ok(AccountPoolAccount::try_from(account));
        let expected = AccountPoolAccount {
            account_id: "acct-1".to_string(),
            backend_account_ref: Some("backend-acct-1".to_string()),
            account_kind: "chatgpt".to_string(),
            enabled: true,
            health_state: Some("unauthorized".to_string()),
            operational_state: Some(AccountOperationalState::Error),
            allocatable: Some(false),
            status_reason_code: Some(AccountPoolReasonCode::AuthFailure),
            status_message: Some("token expired".to_string()),
            current_lease: Some(AccountPoolLease {
                lease_id: "lease-1".to_string(),
                lease_epoch: 7,
                holder_instance_id: "inst-a".to_string(),
                acquired_at: timestamp(11),
                renewed_at: timestamp(12),
                expires_at: timestamp(13),
            }),
            quota: Some(AccountPoolQuota {
                remaining_percent: Some(12.5),
                resets_at: Some(timestamp(14)),
                observed_at: timestamp(15),
            }),
            quotas: vec![super::AccountPoolQuotaFamily {
                limit_id: "codex".to_string(),
                primary: super::AccountPoolQuotaWindow {
                    used_percent: Some(87.5),
                    resets_at: Some(timestamp(14)),
                },
                secondary: super::AccountPoolQuotaWindow {
                    used_percent: None,
                    resets_at: None,
                },
                exhausted_windows: "primary".to_string(),
                predicted_blocked_until: Some(timestamp(14)),
                next_probe_after: Some(timestamp(13)),
                observed_at: timestamp(15),
            }],
            selection: Some(AccountPoolSelection {
                eligible: false,
                next_eligible_at: Some(timestamp(16)),
                preferred: false,
                suppressed: true,
            }),
            updated_at: timestamp(17),
        };

        assert_eq!(observed, expected);
    }

    #[test]
    fn event_and_diagnostics_conversion_preserve_enums_and_payloads() {
        let event = codex_state::AccountPoolEventRecord {
            event_id: "evt-1".to_string(),
            occurred_at: timestamp(20),
            pool_id: "team-main".to_string(),
            account_id: Some("acct-1".to_string()),
            lease_id: Some("lease-1".to_string()),
            holder_instance_id: Some("inst-a".to_string()),
            event_type: "quotaObserved".to_string(),
            reason_code: Some("quotaNearExhausted".to_string()),
            message: "quota observed".to_string(),
            details_json: Some(json!({"remainingPercent": 12.5})),
        };
        let diagnostics = codex_state::AccountPoolDiagnosticsRecord {
            pool_id: "team-main".to_string(),
            generated_at: timestamp(30),
            status: "blocked".to_string(),
            issues: vec![codex_state::AccountPoolIssueRecord {
                severity: "error".to_string(),
                reason_code: "authFailure".to_string(),
                message: "auth failed".to_string(),
                account_id: Some("acct-1".to_string()),
                holder_instance_id: Some("inst-a".to_string()),
                next_relevant_at: Some(timestamp(31)),
            }],
        };

        let observed_event = expect_ok(AccountPoolEvent::try_from(event));
        let expected_event = AccountPoolEvent {
            event_id: "evt-1".to_string(),
            occurred_at: timestamp(20),
            pool_id: "team-main".to_string(),
            account_id: Some("acct-1".to_string()),
            lease_id: Some("lease-1".to_string()),
            holder_instance_id: Some("inst-a".to_string()),
            event_type: AccountPoolEventType::QuotaObserved,
            reason_code: Some(AccountPoolReasonCode::QuotaNearExhausted),
            message: "quota observed".to_string(),
            details_json: Some(json!({"remainingPercent": 12.5})),
        };
        assert_eq!(observed_event, expected_event);

        let observed_diagnostics = expect_ok(AccountPoolDiagnostics::try_from(diagnostics));
        let expected_diagnostics = AccountPoolDiagnostics {
            pool_id: "team-main".to_string(),
            generated_at: timestamp(30),
            status: AccountPoolDiagnosticsStatus::Blocked,
            issues: vec![AccountPoolIssue {
                severity: AccountPoolDiagnosticsSeverity::Error,
                reason_code: AccountPoolReasonCode::AuthFailure,
                message: "auth failed".to_string(),
                account_id: Some("acct-1".to_string()),
                holder_instance_id: Some("inst-a".to_string()),
                next_relevant_at: Some(timestamp(31)),
            }],
        };
        assert_eq!(observed_diagnostics, expected_diagnostics);
    }

    #[test]
    fn page_conversion_preserves_next_cursor() {
        let observed_accounts = expect_ok(AccountPoolAccountsPage::try_from(
            codex_state::AccountPoolAccountsPage {
                data: Vec::new(),
                next_cursor: Some("acct-cursor".to_string()),
            },
        ));
        let observed_events = expect_ok(AccountPoolEventsPage::try_from(
            codex_state::AccountPoolEventsPage {
                data: Vec::new(),
                next_cursor: Some("event-cursor".to_string()),
            },
        ));

        assert_eq!(
            observed_accounts.next_cursor,
            Some("acct-cursor".to_string())
        );
        assert_eq!(
            observed_events.next_cursor,
            Some("event-cursor".to_string())
        );
    }

    fn timestamp(seconds: i64) -> chrono::DateTime<chrono::Utc> {
        match chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0) {
            Some(value) => value,
            None => panic!("invalid unix timestamp"),
        }
    }

    fn expect_ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => panic!("unexpected error: {err:?}"),
        }
    }
}
