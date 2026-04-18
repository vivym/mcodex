use crate::accounts::observability_types::PoolAccountView;
use crate::accounts::observability_types::PoolLeaseView;
use crate::accounts::observability_types::PoolQuotaView;
use crate::accounts::observability_types::PoolSelectionView;
use crate::accounts::observability_types::PoolShowView;

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
        "summary": {
            "totalAccounts": view.summary.total_accounts,
            "activeLeases": view.summary.active_leases,
            "availableAccounts": view.summary.available_accounts,
            "leasedAccounts": view.summary.leased_accounts,
            "pausedAccounts": view.summary.paused_accounts,
            "drainingAccounts": view.summary.draining_accounts,
            "nearExhaustedAccounts": view.summary.near_exhausted_accounts,
            "exhaustedAccounts": view.summary.exhausted_accounts,
            "errorAccounts": view.summary.error_accounts,
        },
        "data": view.data.iter().map(pool_account_json_value).collect::<Vec<_>>(),
        "nextCursor": view.next_cursor,
    })
}

fn pool_account_json_value(account: &PoolAccountView) -> serde_json::Value {
    serde_json::json!({
        "accountId": account.account_id,
        "backendAccountRef": account.backend_account_ref,
        "accountKind": account.account_kind,
        "enabled": account.enabled,
        "healthState": account.health_state,
        "operationalState": account.operational_state,
        "allocatable": account.allocatable,
        "statusReasonCode": account.status_reason_code,
        "statusMessage": account.status_message,
        "currentLease": account.current_lease.as_ref().map(pool_lease_json_value),
        "quota": account.quota.as_ref().map(pool_quota_json_value),
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

fn pool_selection_json_value(selection: &PoolSelectionView) -> serde_json::Value {
    serde_json::json!({
        "eligible": selection.eligible,
        "nextEligibleAt": selection.next_eligible_at,
        "preferred": selection.preferred,
        "suppressed": selection.suppressed,
    })
}

#[cfg(test)]
mod tests {
    use super::PoolAccountView;
    use super::PoolLeaseView;
    use super::PoolShowView;
    use super::lease_text;
    use super::render_pool_show_text;
    use crate::accounts::observability_types::PoolSelectionView;
    use crate::accounts::observability_types::PoolSummaryView;
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

    fn sample_view(data: Vec<PoolAccountView>, next_cursor: Option<&str>) -> PoolShowView {
        PoolShowView {
            pool_id: "team-main".to_string(),
            refreshed_at: Some("2026-04-18T00:00:00Z".to_string()),
            summary: PoolSummaryView {
                total_accounts: 1,
                active_leases: 0,
                available_accounts: Some(1),
                leased_accounts: Some(0),
                paused_accounts: None,
                draining_accounts: None,
                near_exhausted_accounts: None,
                exhausted_accounts: None,
                error_accounts: None,
            },
            data,
            next_cursor: next_cursor.map(ToOwned::to_owned),
        }
    }

    fn sample_account() -> PoolAccountView {
        PoolAccountView {
            account_id: "acct-1".to_string(),
            backend_account_ref: None,
            account_kind: "chatgpt".to_string(),
            enabled: true,
            health_state: None,
            operational_state: None,
            allocatable: None,
            status_reason_code: None,
            status_message: None,
            current_lease: None,
            quota: None,
            selection: Some(PoolSelectionView {
                eligible: true,
                next_eligible_at: None,
                preferred: false,
                suppressed: false,
            }),
            updated_at: Some("2026-04-18T00:00:00Z".to_string()),
        }
    }
}
