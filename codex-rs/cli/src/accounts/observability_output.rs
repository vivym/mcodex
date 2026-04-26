use crate::accounts::observability_types::DiagnosticsIssueView;
use crate::accounts::observability_types::DiagnosticsView;
use crate::accounts::observability_types::EventView;
use crate::accounts::observability_types::EventsView;
use crate::accounts::observability_types::PoolAccountView;
use crate::accounts::observability_types::PoolLeaseView;
use crate::accounts::observability_types::PoolQuotaFamilyView;
use crate::accounts::observability_types::PoolQuotaView;
use crate::accounts::observability_types::PoolQuotaWindowView;
use crate::accounts::observability_types::PoolSelectionView;
use crate::accounts::observability_types::PoolShowView;
use crate::accounts::observability_types::PoolSummaryView;
use crate::accounts::observability_types::StatusPoolObservabilityView;

pub(crate) fn print_pool_show_text(view: &PoolShowView) {
    print!("{}", render_pool_show_text(view));
}

pub(crate) fn print_pool_show_json(view: &PoolShowView) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&pool_show_json_value(view))?
    );
    Ok(())
}

pub(crate) fn print_diagnostics_text(view: &DiagnosticsView) {
    print!("{}", render_diagnostics_text(view));
}

pub(crate) fn print_diagnostics_json(view: &DiagnosticsView) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&diagnostics_json_value(view))?
    );
    Ok(())
}

pub(crate) fn print_events_text(view: &EventsView) {
    print!("{}", render_events_text(view));
}

pub(crate) fn print_events_json(view: &EventsView) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&events_json_value(view))?
    );
    Ok(())
}

pub(crate) fn render_status_pool_observability_text(view: &StatusPoolObservabilityView) -> String {
    let mut lines = vec![format!("pooled observability pool: {}", view.pool_id)];

    if let Some(summary) = view.summary.as_ref() {
        lines.push(format!("pooled accounts total: {}", summary.total_accounts));
        lines.push(format!("pooled active leases: {}", summary.active_leases));
        lines.push(format!(
            "pooled accounts available: {}",
            optional_count_text(summary.available_accounts)
        ));
        lines.push(format!(
            "pooled accounts leased: {}",
            optional_count_text(summary.leased_accounts)
        ));
    }

    if let Some(diagnostics) = view.diagnostics.as_ref() {
        lines.push(format!("pooled diagnostics status: {}", diagnostics.status));
        if diagnostics.issues.is_empty() {
            lines.push("pooled diagnostics issues: none".to_string());
        } else if let Some((issue, remaining_issues)) = diagnostics.issues.split_first() {
            lines.push(format!(
                "issue: {} {} {}",
                issue.severity, issue.reason_code, issue.message
            ));
            if !remaining_issues.is_empty() {
                lines.push(format!("issues: +{} more", remaining_issues.len()));
            }
        }
    }

    if let Some(warning) = view.warning.as_deref() {
        lines.push(format!("warning: {warning}"));
    }

    format!("{}\n", lines.join("\n"))
}

pub(crate) fn status_pool_observability_json_value(
    view: Option<&StatusPoolObservabilityView>,
) -> serde_json::Value {
    match view {
        Some(view) => serde_json::json!({
            "poolId": view.pool_id,
            "summary": view.summary.as_ref().map(pool_summary_json_value),
            "diagnostics": view.diagnostics.as_ref().map(diagnostics_json_value),
            "warning": view.warning,
        }),
        None => serde_json::Value::Null,
    }
}

fn render_pool_show_text(view: &PoolShowView) -> String {
    let mut lines = vec![
        format!("pool id: {}", view.pool_id),
        format!(
            "refreshed at: {}",
            view.refreshed_at.as_deref().unwrap_or("unknown")
        ),
        format!("total accounts: {}", view.summary.total_accounts),
        format!("active leases: {}", view.summary.active_leases),
        format!(
            "available accounts: {}",
            optional_count_text(view.summary.available_accounts)
        ),
        format!(
            "leased accounts: {}",
            optional_count_text(view.summary.leased_accounts)
        ),
    ];

    if view.data.is_empty() {
        lines.push("accounts: none".to_string());
    } else {
        lines.push(
            "accountId | kind | enabled | health | state | lease | eligible | preferred"
                .to_string(),
        );
        for account in &view.data {
            lines.push(format!(
                "{} | {} | {} | {} | {} | {} | {} | {}",
                account.account_id,
                account.account_kind,
                account.enabled,
                account.health_state.as_deref().unwrap_or("unknown"),
                account.operational_state.as_deref().unwrap_or("unknown"),
                lease_text(account.current_lease.as_ref()),
                selection_bool_text(
                    account
                        .selection
                        .as_ref()
                        .map(|selection| selection.eligible)
                ),
                selection_bool_text(
                    account
                        .selection
                        .as_ref()
                        .map(|selection| selection.preferred)
                ),
            ));
        }

        let quota_rows = view
            .data
            .iter()
            .flat_map(|account| {
                account
                    .quotas
                    .iter()
                    .map(|quota| (account.account_id.as_str(), quota))
            })
            .collect::<Vec<_>>();
        if !quota_rows.is_empty() {
            lines.push("quotas:".to_string());
            lines.push(
                "accountId | family | primary | secondary | exhausted | blockedUntil | nextProbe"
                    .to_string(),
            );
            for (account_id, quota) in quota_rows {
                lines.push(format!(
                    "{} | {} | {} | {} | {} | {} | {}",
                    account_id,
                    quota.limit_id,
                    quota_window_text(&quota.primary),
                    quota_window_text(&quota.secondary),
                    exhausted_windows_text(&quota.exhausted_windows),
                    quota.predicted_blocked_until.as_deref().unwrap_or("none"),
                    quota.next_probe_after.as_deref().unwrap_or("none"),
                ));
            }
        }
    }

    if let Some(next_cursor) = view.next_cursor.as_deref() {
        lines.push(format!("next cursor: {next_cursor}"));
    }

    format!("{}\n", lines.join("\n"))
}

fn optional_count_text(value: Option<u32>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "unknown".to_string(),
    }
}

fn lease_text(lease: Option<&PoolLeaseView>) -> String {
    match lease {
        Some(lease) => format!("{}@{}", lease.lease_id, lease.holder_instance_id),
        None => "-".to_string(),
    }
}

fn selection_bool_text(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "unknown",
    }
}

fn pool_show_json_value(view: &PoolShowView) -> serde_json::Value {
    serde_json::json!({
        "poolId": view.pool_id,
        "refreshedAt": view.refreshed_at,
        "summary": pool_summary_json_value(&view.summary),
        "data": view.data.iter().map(pool_account_json_value).collect::<Vec<_>>(),
        "nextCursor": view.next_cursor,
    })
}

fn pool_summary_json_value(summary: &PoolSummaryView) -> serde_json::Value {
    serde_json::json!({
        "totalAccounts": summary.total_accounts,
        "activeLeases": summary.active_leases,
        "availableAccounts": summary.available_accounts,
        "leasedAccounts": summary.leased_accounts,
        "pausedAccounts": summary.paused_accounts,
        "drainingAccounts": summary.draining_accounts,
        "nearExhaustedAccounts": summary.near_exhausted_accounts,
        "exhaustedAccounts": summary.exhausted_accounts,
        "errorAccounts": summary.error_accounts,
    })
}

fn pool_account_json_value(account: &PoolAccountView) -> serde_json::Value {
    serde_json::json!({
        "accountId": account.account_id,
        "backendAccountRef": account.backend_account_ref,
        "accountKind": account.account_kind,
        "selectionFamily": account.selection_family,
        "enabled": account.enabled,
        "healthState": account.health_state,
        "operationalState": account.operational_state,
        "allocatable": account.allocatable,
        "statusReasonCode": account.status_reason_code,
        "statusMessage": account.status_message,
        "currentLease": account.current_lease.as_ref().map(pool_lease_json_value),
        "quota": account.quota.as_ref().map(pool_quota_json_value),
        "quotas": account.quotas.iter().map(pool_quota_family_json_value).collect::<Vec<_>>(),
        "selection": account.selection.as_ref().map(pool_selection_json_value),
        "updatedAt": account.updated_at,
    })
}

fn pool_lease_json_value(lease: &PoolLeaseView) -> serde_json::Value {
    serde_json::json!({
        "leaseId": lease.lease_id,
        "leaseEpoch": lease.lease_epoch,
        "holderInstanceId": lease.holder_instance_id,
        "acquiredAt": lease.acquired_at,
        "renewedAt": lease.renewed_at,
        "expiresAt": lease.expires_at,
    })
}

fn pool_quota_json_value(quota: &PoolQuotaView) -> serde_json::Value {
    serde_json::json!({
        "remainingPercent": quota.remaining_percent,
        "resetsAt": quota.resets_at,
        "observedAt": quota.observed_at,
    })
}

fn pool_quota_family_json_value(quota: &PoolQuotaFamilyView) -> serde_json::Value {
    serde_json::json!({
        "limitId": quota.limit_id,
        "primary": pool_quota_window_json_value(&quota.primary),
        "secondary": pool_quota_window_json_value(&quota.secondary),
        "exhaustedWindows": quota.exhausted_windows,
        "predictedBlockedUntil": quota.predicted_blocked_until,
        "nextProbeAfter": quota.next_probe_after,
        "observedAt": quota.observed_at,
    })
}

fn pool_quota_window_json_value(window: &PoolQuotaWindowView) -> serde_json::Value {
    serde_json::json!({
        "usedPercent": window.used_percent,
        "resetsAt": window.resets_at,
    })
}

fn quota_window_text(window: &PoolQuotaWindowView) -> String {
    let used_percent = window
        .used_percent
        .map(|used_percent| format!("{used_percent:.1}% used"))
        .unwrap_or_else(|| "unknown used".to_string());
    match window.resets_at.as_deref() {
        Some(resets_at) => format!("{used_percent}, resets {resets_at}"),
        None => used_percent,
    }
}

fn exhausted_windows_text(exhausted_windows: &str) -> String {
    if exhausted_windows == "none" {
        "none".to_string()
    } else {
        format!("{exhausted_windows} exhausted")
    }
}

fn pool_selection_json_value(selection: &PoolSelectionView) -> serde_json::Value {
    serde_json::json!({
        "eligible": selection.eligible,
        "nextEligibleAt": selection.next_eligible_at,
        "preferred": selection.preferred,
        "suppressed": selection.suppressed,
    })
}

fn render_diagnostics_text(view: &DiagnosticsView) -> String {
    let mut lines = vec![
        format!("pool id: {}", view.pool_id),
        format!(
            "generated at: {}",
            view.generated_at.as_deref().unwrap_or("unknown")
        ),
        format!("status: {}", view.status),
    ];

    if view.issues.is_empty() {
        lines.push("issues: none".to_string());
    } else {
        lines.push(
            "severity | reasonCode | message | accountId | holderInstanceId | nextRelevantAt"
                .to_string(),
        );
        for issue in &view.issues {
            lines.push(format!(
                "{} | {} | {} | {} | {} | {}",
                issue.severity,
                issue.reason_code,
                issue.message,
                issue.account_id.as_deref().unwrap_or("none"),
                issue.holder_instance_id.as_deref().unwrap_or("none"),
                issue.next_relevant_at.as_deref().unwrap_or("none"),
            ));
        }
    }

    format!("{}\n", lines.join("\n"))
}

fn diagnostics_json_value(view: &DiagnosticsView) -> serde_json::Value {
    serde_json::json!({
        "poolId": view.pool_id,
        "generatedAt": view.generated_at,
        "status": view.status,
        "issues": view.issues.iter().map(diagnostics_issue_json_value).collect::<Vec<_>>(),
    })
}

fn diagnostics_issue_json_value(issue: &DiagnosticsIssueView) -> serde_json::Value {
    serde_json::json!({
        "severity": issue.severity,
        "reasonCode": issue.reason_code,
        "message": issue.message,
        "accountId": issue.account_id,
        "holderInstanceId": issue.holder_instance_id,
        "nextRelevantAt": issue.next_relevant_at,
    })
}

fn render_events_text(view: &EventsView) -> String {
    let mut lines = vec![format!("pool id: {}", view.pool_id)];

    if view.data.is_empty() {
        lines.push("events: none".to_string());
    } else {
        lines.push(
            "eventId | occurredAt | type | accountId | reasonCode | message | details".to_string(),
        );
        for event in &view.data {
            lines.push(format!(
                "{} | {} | {} | {} | {} | {} | {}",
                event.event_id,
                event.occurred_at,
                event.event_type,
                event.account_id.as_deref().unwrap_or("none"),
                event.reason_code.as_deref().unwrap_or("none"),
                event.message,
                event.details,
            ));
        }
    }

    if let Some(next_cursor) = view.next_cursor.as_deref() {
        lines.push(format!("next cursor: {next_cursor}"));
    }

    format!("{}\n", lines.join("\n"))
}

fn events_json_value(view: &EventsView) -> serde_json::Value {
    serde_json::json!({
        "poolId": view.pool_id,
        "data": view.data.iter().map(event_json_value).collect::<Vec<_>>(),
        "nextCursor": view.next_cursor,
    })
}

fn event_json_value(event: &EventView) -> serde_json::Value {
    serde_json::json!({
        "eventId": event.event_id,
        "occurredAt": event.occurred_at,
        "poolId": event.pool_id,
        "accountId": event.account_id,
        "leaseId": event.lease_id,
        "holderInstanceId": event.holder_instance_id,
        "eventType": event.event_type,
        "reasonCode": event.reason_code,
        "message": event.message,
        "details": event.details,
    })
}

#[cfg(test)]
mod tests {
    use super::EventView;
    use super::EventsView;
    use super::PoolAccountView;
    use super::PoolLeaseView;
    use super::PoolShowView;
    use super::lease_text;
    use super::render_diagnostics_text;
    use super::render_events_text;
    use super::render_pool_show_text;
    use super::render_status_pool_observability_text;
    use super::status_pool_observability_json_value;
    use crate::accounts::observability_types::DiagnosticsIssueView;
    use crate::accounts::observability_types::DiagnosticsView;
    use crate::accounts::observability_types::PoolSelectionView;
    use crate::accounts::observability_types::PoolSummaryView;
    use crate::accounts::observability_types::StatusPoolObservabilityView;
    use pretty_assertions::assert_eq;

    #[test]
    fn pool_show_text_formats_active_lease() {
        let text = lease_text(Some(&PoolLeaseView {
            lease_id: "lease-1".to_string(),
            lease_epoch: 7,
            holder_instance_id: "holder-1".to_string(),
            acquired_at: "2026-04-18T00:00:00Z".to_string(),
            renewed_at: "2026-04-18T00:01:00Z".to_string(),
            expires_at: "2026-04-18T00:05:00Z".to_string(),
        }));

        assert_eq!(text, "lease-1@holder-1");
    }

    #[test]
    fn pool_show_text_reports_accounts_none() {
        let text = render_pool_show_text(&sample_view(Vec::new(), None));

        assert!(text.contains("accounts: none"));
    }

    #[test]
    fn pool_show_text_reports_next_cursor() {
        let text = render_pool_show_text(&sample_view(vec![sample_account()], Some("cursor-1")));

        assert!(text.contains("next cursor: cursor-1"));
    }

    #[test]
    fn pool_show_json_preserves_nulls() {
        let json = super::pool_show_json_value(&sample_view(vec![sample_account()], None));

        assert!(json["data"][0]["quota"].is_null());
        assert!(json["data"][0]["statusReasonCode"].is_null());
        assert!(json["nextCursor"].is_null());
    }

    #[test]
    fn diagnostics_text_reports_issues_none() {
        let text = render_diagnostics_text(&sample_diagnostics(Vec::new()));

        assert!(text.contains("issues: none"));
    }

    #[test]
    fn diagnostics_text_formats_issue_rows() {
        let text = render_diagnostics_text(&sample_diagnostics(vec![DiagnosticsIssueView {
            severity: "warning".to_string(),
            reason_code: "leaseHeldByAnotherInstance".to_string(),
            message: "account is leased".to_string(),
            account_id: Some("acct-1".to_string()),
            holder_instance_id: Some("holder-1".to_string()),
            next_relevant_at: Some("2026-04-18T00:05:00Z".to_string()),
        }]));

        assert!(text.contains("warning | leaseHeldByAnotherInstance | account is leased | acct-1 | holder-1 | 2026-04-18T00:05:00Z"));
    }

    #[test]
    fn events_text_reports_events_none() {
        let text = render_events_text(&sample_events(Vec::new(), None));

        assert!(text.contains("pool id: team-main"));
        assert!(text.contains("events: none"));
    }

    #[test]
    fn events_text_reports_next_cursor() {
        let text = render_events_text(&sample_events(vec![sample_event()], Some("cursor-1")));

        assert!(text.contains("next cursor: cursor-1"));
    }

    #[test]
    fn status_pool_observability_text_reports_healthy_summary_lines() {
        let text = render_status_pool_observability_text(&StatusPoolObservabilityView {
            pool_id: "team-main".to_string(),
            summary: Some(sample_summary()),
            accounts: Some(Vec::new()),
            diagnostics: Some(sample_diagnostics(Vec::new())),
            warning: None,
        });

        assert!(text.contains("pooled accounts total: 1"));
        assert!(text.contains("pooled active leases: 0"));
        assert!(text.contains("pooled diagnostics status: healthy"));
    }

    #[test]
    fn accounts_status_text_shows_counts_issue_summary_and_warning() {
        let text = render_status_pool_observability_text(&StatusPoolObservabilityView {
            pool_id: "team-main".to_string(),
            summary: Some(sample_summary()),
            accounts: Some(Vec::new()),
            diagnostics: Some(sample_diagnostics(vec![DiagnosticsIssueView {
                severity: "warning".to_string(),
                reason_code: "cooldownActive".to_string(),
                message: "account acct-1 is in cooldown".to_string(),
                account_id: Some("acct-1".to_string()),
                holder_instance_id: None,
                next_relevant_at: None,
            }])),
            warning: Some("summary unavailable".to_string()),
        });

        assert!(text.contains("pooled accounts total: 1"));
        assert!(text.contains("pooled diagnostics status: degraded"));
        assert!(text.contains("issue: warning cooldownActive account acct-1 is in cooldown"));
        assert!(text.contains("warning: summary unavailable"));
    }

    #[test]
    fn status_pool_observability_text_reports_diagnostics_issue_and_warning_without_summary() {
        let text = render_status_pool_observability_text(&StatusPoolObservabilityView {
            pool_id: "team-main".to_string(),
            summary: None,
            accounts: Some(Vec::new()),
            diagnostics: Some(sample_diagnostics(vec![DiagnosticsIssueView {
                severity: "warning".to_string(),
                reason_code: "cooldownActive".to_string(),
                message: "account acct-1 is in cooldown".to_string(),
                account_id: Some("acct-1".to_string()),
                holder_instance_id: None,
                next_relevant_at: None,
            }])),
            warning: Some("summary unavailable".to_string()),
        });

        assert!(text.contains("pooled diagnostics status: degraded"));
        assert!(text.contains("issue: warning cooldownActive account acct-1 is in cooldown"));
        assert!(text.contains("warning: summary unavailable"));
    }

    #[test]
    fn status_pool_observability_text_summarizes_multiple_issues() {
        let text = render_status_pool_observability_text(&StatusPoolObservabilityView {
            pool_id: "team-main".to_string(),
            summary: None,
            accounts: Some(Vec::new()),
            diagnostics: Some(sample_diagnostics(vec![
                DiagnosticsIssueView {
                    severity: "warning".to_string(),
                    reason_code: "cooldownActive".to_string(),
                    message: "account acct-1 is in cooldown".to_string(),
                    account_id: Some("acct-1".to_string()),
                    holder_instance_id: None,
                    next_relevant_at: None,
                },
                DiagnosticsIssueView {
                    severity: "error".to_string(),
                    reason_code: "authFailure".to_string(),
                    message: "account acct-2 is unauthorized".to_string(),
                    account_id: Some("acct-2".to_string()),
                    holder_instance_id: None,
                    next_relevant_at: None,
                },
            ])),
            warning: None,
        });

        assert!(text.contains("issue: warning cooldownActive account acct-1 is in cooldown"));
        assert!(text.contains("issues: +1 more"));
        assert!(!text.contains("account acct-2 is unauthorized"));
    }

    #[test]
    fn status_pool_observability_text_reports_summary_and_warning_without_diagnostics() {
        let text = render_status_pool_observability_text(&StatusPoolObservabilityView {
            pool_id: "team-main".to_string(),
            summary: Some(sample_summary()),
            accounts: Some(Vec::new()),
            diagnostics: None,
            warning: Some("diagnostics unavailable".to_string()),
        });

        assert!(text.contains("pooled accounts total: 1"));
        assert!(text.contains("pooled active leases: 0"));
        assert!(text.contains("warning: diagnostics unavailable"));
    }

    #[test]
    fn status_pool_observability_json_preserves_null_fields() {
        let json = status_pool_observability_json_value(Some(&StatusPoolObservabilityView {
            pool_id: "team-main".to_string(),
            summary: None,
            accounts: Some(Vec::new()),
            diagnostics: None,
            warning: None,
        }));

        assert_eq!(json["poolId"], "team-main");
        assert!(json["summary"].is_null());
        assert!(json["diagnostics"].is_null());
        assert!(json["warning"].is_null());
    }

    fn sample_view(data: Vec<PoolAccountView>, next_cursor: Option<&str>) -> PoolShowView {
        PoolShowView {
            pool_id: "team-main".to_string(),
            refreshed_at: Some("2026-04-18T00:00:00Z".to_string()),
            summary: sample_summary(),
            data,
            next_cursor: next_cursor.map(ToOwned::to_owned),
        }
    }

    fn sample_summary() -> PoolSummaryView {
        PoolSummaryView {
            total_accounts: 1,
            active_leases: 0,
            available_accounts: Some(1),
            leased_accounts: Some(0),
            paused_accounts: None,
            draining_accounts: None,
            near_exhausted_accounts: None,
            exhausted_accounts: None,
            error_accounts: None,
        }
    }

    fn sample_account() -> PoolAccountView {
        PoolAccountView {
            account_id: "acct-1".to_string(),
            backend_account_ref: None,
            account_kind: "chatgpt".to_string(),
            selection_family: "chatgpt".to_string(),
            enabled: true,
            health_state: None,
            operational_state: None,
            allocatable: None,
            status_reason_code: None,
            status_message: None,
            current_lease: None,
            quota: None,
            quotas: Vec::new(),
            selection: Some(PoolSelectionView {
                eligible: true,
                next_eligible_at: None,
                preferred: false,
                suppressed: false,
            }),
            updated_at: Some("2026-04-18T00:00:00Z".to_string()),
        }
    }

    fn sample_diagnostics(issues: Vec<DiagnosticsIssueView>) -> DiagnosticsView {
        DiagnosticsView {
            pool_id: "team-main".to_string(),
            generated_at: Some("2026-04-18T00:00:00Z".to_string()),
            status: if issues.is_empty() {
                "healthy".to_string()
            } else {
                "degraded".to_string()
            },
            issues,
        }
    }

    fn sample_events(data: Vec<EventView>, next_cursor: Option<&str>) -> EventsView {
        EventsView {
            pool_id: "team-main".to_string(),
            data,
            next_cursor: next_cursor.map(ToOwned::to_owned),
        }
    }

    fn sample_event() -> EventView {
        EventView {
            event_id: "event-1".to_string(),
            occurred_at: "2026-04-18T00:00:00Z".to_string(),
            pool_id: "team-main".to_string(),
            account_id: Some("acct-1".to_string()),
            lease_id: Some("lease-1".to_string()),
            holder_instance_id: Some("holder-1".to_string()),
            event_type: "leaseAcquired".to_string(),
            reason_code: Some("automaticAccountSelected".to_string()),
            message: "lease acquired".to_string(),
            details: serde_json::json!(["soft-limit", 42]),
        }
    }
}
