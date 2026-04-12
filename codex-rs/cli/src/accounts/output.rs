use crate::accounts::diagnostics::AccountsCurrentDiagnostic;
use crate::accounts::diagnostics::AccountsStatusDiagnostic;
use codex_state::AccountHealthState;
use codex_state::AccountPoolAccountDiagnostic;
use codex_state::AccountStartupEligibility;
use codex_state::AccountStartupSelectionPreview;

pub(crate) fn print_current_text(diagnostic: &AccountsCurrentDiagnostic) {
    print_preview(&diagnostic.preview);
    println!(
        "automatic selection: {}",
        if diagnostic.preview.suppressed {
            "suppressed"
        } else {
            "enabled"
        }
    );
}

pub(crate) fn print_current_json(diagnostic: &AccountsCurrentDiagnostic) -> anyhow::Result<()> {
    let output = serde_json::to_string_pretty(&serde_json::json!({
        "accountPoolOverrideId": diagnostic.account_pool_override_id.as_deref(),
        "effectivePoolId": diagnostic.preview.effective_pool_id.as_deref(),
        "preferredAccountId": diagnostic.preview.preferred_account_id.as_deref(),
        "predictedAccountId": diagnostic.preview.predicted_account_id.as_deref(),
        "suppressed": diagnostic.preview.suppressed,
        "eligibility": {
            "code": eligibility_code(&diagnostic.preview.eligibility),
            "reason": eligibility_reason(&diagnostic.preview.eligibility),
        },
    }))?;
    println!("{output}");
    Ok(())
}

pub(crate) fn print_status_text(diagnostic: &AccountsStatusDiagnostic) {
    println!(
        "suppression: {}",
        if diagnostic.preview.suppressed {
            "enabled"
        } else {
            "disabled"
        }
    );
    print_preview(&diagnostic.preview);
    println!("configured pools: {}", diagnostic.configured_pool_count);

    if let Some(account_pool_override_id) = diagnostic.account_pool_override_id.as_deref() {
        println!("account pool override: {account_pool_override_id}");
    }

    if let Some(pool) = diagnostic.pool.as_ref() {
        println!(
            "next eligible at: {}",
            format_optional_timestamp(pool.next_eligible_at.as_ref())
        );
        for account in &pool.accounts {
            println!(
                "account {}: health={}, eligibility={}, next eligible at={}",
                account.account_id,
                health_state(account),
                eligibility_reason(&account.eligibility),
                format_optional_timestamp(account.next_eligible_at.as_ref())
            );
        }
    }
}

pub(crate) fn print_status_json(diagnostic: &AccountsStatusDiagnostic) -> anyhow::Result<()> {
    let output = serde_json::to_string_pretty(&serde_json::json!({
        "accountPoolOverrideId": diagnostic.account_pool_override_id.as_deref(),
        "configuredPoolCount": diagnostic.configured_pool_count,
        "effectivePoolId": diagnostic.preview.effective_pool_id.as_deref(),
        "preferredAccountId": diagnostic.preview.preferred_account_id.as_deref(),
        "predictedAccountId": diagnostic.preview.predicted_account_id.as_deref(),
        "suppressed": diagnostic.preview.suppressed,
        "switchReason": {
            "code": eligibility_code(&diagnostic.preview.eligibility),
            "reason": eligibility_reason(&diagnostic.preview.eligibility),
        },
        "nextEligibleAt": diagnostic
            .pool
            .as_ref()
            .and_then(|pool| format_timestamp(pool.next_eligible_at.as_ref())),
        "accounts": diagnostic
            .pool
            .as_ref()
            .map(|pool| {
                pool.accounts
                    .iter()
                    .map(|account| {
                        serde_json::json!({
                            "accountId": &account.account_id,
                            "poolId": &account.pool_id,
                            "healthy": account.healthy,
                            "healthState": health_state(account),
                            "eligibility": {
                                "code": eligibility_code(&account.eligibility),
                                "reason": eligibility_reason(&account.eligibility),
                            },
                            "nextEligibleAt": format_timestamp(account.next_eligible_at.as_ref()),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    }))?;
    println!("{output}");
    Ok(())
}

fn print_preview(preview: &AccountStartupSelectionPreview) {
    println!(
        "effective pool: {}",
        preview.effective_pool_id.as_deref().unwrap_or("none")
    );
    println!(
        "preferred account: {}",
        preview
            .preferred_account_id
            .as_deref()
            .unwrap_or("automatic")
    );
    println!(
        "predicted account: {}",
        preview.predicted_account_id.as_deref().unwrap_or("none")
    );
    println!("eligibility: {}", eligibility_reason(&preview.eligibility));
}

fn eligibility_code(eligibility: &AccountStartupEligibility) -> &'static str {
    match eligibility {
        AccountStartupEligibility::Suppressed => "suppressed",
        AccountStartupEligibility::MissingPool => "missingPool",
        AccountStartupEligibility::PreferredAccountSelected => "preferredAccountSelected",
        AccountStartupEligibility::AutomaticAccountSelected => "automaticAccountSelected",
        AccountStartupEligibility::PreferredAccountMissing => "preferredAccountMissing",
        AccountStartupEligibility::PreferredAccountInOtherPool { .. } => {
            "preferredAccountInOtherPool"
        }
        AccountStartupEligibility::PreferredAccountUnhealthy => "preferredAccountUnhealthy",
        AccountStartupEligibility::PreferredAccountBusy => "preferredAccountBusy",
        AccountStartupEligibility::NoEligibleAccount => "noEligibleAccount",
    }
}

fn eligibility_reason(eligibility: &AccountStartupEligibility) -> String {
    match eligibility {
        AccountStartupEligibility::Suppressed => {
            "automatic pooled selection is suppressed".to_string()
        }
        AccountStartupEligibility::MissingPool => "no effective pool is configured".to_string(),
        AccountStartupEligibility::PreferredAccountSelected => {
            "preferred account is eligible for fresh-runtime startup".to_string()
        }
        AccountStartupEligibility::AutomaticAccountSelected => {
            "automatic startup selection is eligible".to_string()
        }
        AccountStartupEligibility::PreferredAccountMissing => {
            "preferred account is not registered".to_string()
        }
        AccountStartupEligibility::PreferredAccountInOtherPool { actual_pool_id } => {
            format!("preferred account belongs to pool `{actual_pool_id}`")
        }
        AccountStartupEligibility::PreferredAccountUnhealthy => {
            "preferred account is unhealthy".to_string()
        }
        AccountStartupEligibility::PreferredAccountBusy => {
            "preferred account is currently leased by another runtime".to_string()
        }
        AccountStartupEligibility::NoEligibleAccount => {
            "no eligible account is available in the effective pool".to_string()
        }
    }
}

fn health_state(account: &AccountPoolAccountDiagnostic) -> &'static str {
    match account.health_state {
        Some(AccountHealthState::Healthy) => "healthy",
        Some(AccountHealthState::RateLimited) => "rateLimited",
        Some(AccountHealthState::Unauthorized) => "unauthorized",
        None if account.healthy => "healthy",
        None => "unhealthy",
    }
}

fn format_optional_timestamp<T: std::fmt::Display>(value: Option<&T>) -> String {
    format_timestamp(value).unwrap_or_else(|| "none".to_string())
}

fn format_timestamp<T: std::fmt::Display>(value: Option<&T>) -> Option<String> {
    value.map(ToString::to_string)
}
