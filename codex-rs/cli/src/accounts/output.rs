use crate::accounts::diagnostics::AccountsCurrentDiagnostic;
use crate::accounts::diagnostics::AccountsStatusDiagnostic;
use crate::accounts::observability_output::render_status_pool_observability_text;
use crate::accounts::observability_output::status_pool_observability_json_value;
use crate::accounts::observability_types::PoolAccountView;
use crate::accounts::observability_types::PoolQuotaFamilyView;
use crate::accounts::observability_types::StatusPoolObservabilityView;
use codex_state::AccountHealthState;
use codex_state::AccountPoolAccountDiagnostic;
use codex_state::AccountPoolDiagnostic;
use codex_state::AccountSource;
use codex_state::AccountStartupEligibility;
use codex_state::AccountStartupSelectionPreview;
use codex_state::EffectivePoolResolutionSource;

struct EligibilityView {
    code: &'static str,
    reason: String,
}

pub(crate) fn print_current_text(diagnostic: &AccountsCurrentDiagnostic) {
    print_preview(&diagnostic.startup.startup.preview);
    println!(
        "automatic selection: {}",
        if diagnostic.startup.startup.preview.suppressed {
            "suppressed"
        } else {
            "enabled"
        }
    );
}

pub(crate) fn print_current_json(diagnostic: &AccountsCurrentDiagnostic) -> anyhow::Result<()> {
    let startup = &diagnostic.startup.startup;
    let preview = &startup.preview;
    let output = serde_json::to_string_pretty(&serde_json::json!({
        "accountPoolOverrideId": diagnostic.account_pool_override_id.as_deref(),
        "effectivePoolId": preview.effective_pool_id.as_deref(),
        "effectivePoolResolutionSource": effective_pool_resolution_source_to_wire_string(
            startup.effective_pool_resolution_source
        ),
        "preferredAccountId": preview.preferred_account_id.as_deref(),
        "predictedAccountId": preview.predicted_account_id.as_deref(),
        "suppressed": preview.suppressed,
        "eligibility": {
            "code": eligibility_code(&preview.eligibility),
            "reason": eligibility_reason(&preview.eligibility),
        },
    }))?;
    println!("{output}");
    Ok(())
}

pub(crate) fn print_status_text(diagnostic: &AccountsStatusDiagnostic) {
    let startup = &diagnostic.startup.startup;
    let preview = &startup.preview;
    println!(
        "suppression: {}",
        if preview.suppressed {
            "enabled"
        } else {
            "disabled"
        }
    );
    print_status_preview(preview, effective_pool_source(diagnostic));
    println!(
        "health state: {}",
        status_health_state(preview, diagnostic.pool.as_ref()).unwrap_or("unknown")
    );
    println!("configured pools: {}", diagnostic.configured_pool_count);
    println!("registered pools: {}", diagnostic.registered_pool_count);
    println!(
        "configured default pool: {}",
        startup
            .configured_default_pool_id
            .as_deref()
            .unwrap_or("none")
    );
    println!(
        "persisted default pool: {}",
        startup
            .persisted_default_pool_id
            .as_deref()
            .unwrap_or("none")
    );
    println!(
        "effective pool resolution: {}",
        effective_pool_resolution_source_to_wire_string(startup.effective_pool_resolution_source)
    );

    if let Some(account_pool_override_id) = diagnostic.account_pool_override_id.as_deref() {
        println!("account pool override: {account_pool_override_id}");
    }

    if let Some(pool) = diagnostic.pool.as_ref() {
        println!(
            "next eligible at: {}",
            format_optional_timestamp(pool.next_eligible_at.as_ref())
        );
        for account in &pool.accounts {
            let eligibility = normalized_account_eligibility(
                account,
                preview,
                diagnostic.pool_observability.as_ref(),
            );
            println!(
                "account {}: enabled={}, health={}, eligibility={}, next eligible at={}{}",
                account.account_id,
                account.enabled,
                health_state(account),
                eligibility.reason,
                format_optional_timestamp(account.next_eligible_at.as_ref()),
                source_suffix(account.source)
            );
        }
    }

    if let Some(pool_observability) = diagnostic.pool_observability.as_ref() {
        print!(
            "{}",
            render_status_pool_observability_text(pool_observability)
        );
    }
}

pub(crate) fn print_status_json(diagnostic: &AccountsStatusDiagnostic) -> anyhow::Result<()> {
    let startup = &diagnostic.startup.startup;
    let preview = &startup.preview;
    let output = serde_json::to_string_pretty(&serde_json::json!({
        "accountPoolOverrideId": diagnostic.account_pool_override_id.as_deref(),
        "configuredPoolCount": diagnostic.configured_pool_count,
        "registeredPoolCount": diagnostic.registered_pool_count,
        "effectivePoolId": preview.effective_pool_id.as_deref(),
        "effectivePoolSource": effective_pool_source(diagnostic).map(AccountSource::as_str),
        "effectivePoolResolutionSource": effective_pool_resolution_source_to_wire_string(
            startup.effective_pool_resolution_source
        ),
        "configuredDefaultPoolId": startup.configured_default_pool_id.as_deref(),
        "persistedDefaultPoolId": startup.persisted_default_pool_id.as_deref(),
        "preferredAccountId": preview.preferred_account_id.as_deref(),
        "predictedAccountId": preview.predicted_account_id.as_deref(),
        "suppressed": preview.suppressed,
        "healthState": status_health_state(preview, diagnostic.pool.as_ref()),
        "switchReason": {
            "code": eligibility_code(&preview.eligibility),
            "reason": eligibility_reason(&preview.eligibility),
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
                        let eligibility =
                            normalized_account_eligibility(account, preview, diagnostic.pool_observability.as_ref());
                        serde_json::json!({
                            "accountId": &account.account_id,
                            "poolId": &account.pool_id,
                            "source": account.source.map(AccountSource::as_str),
                            "enabled": account.enabled,
                            "healthy": account.healthy,
                            "healthState": health_state(account),
                            "eligibility": {
                                "code": eligibility.code,
                                "reason": eligibility.reason,
                            },
                            "nextEligibleAt": format_timestamp(account.next_eligible_at.as_ref()),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        "poolObservability": status_pool_observability_json_value(
            diagnostic.pool_observability.as_ref()
        ),
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

fn print_status_preview(
    preview: &AccountStartupSelectionPreview,
    effective_pool_source: Option<AccountSource>,
) {
    let effective_pool = preview.effective_pool_id.as_deref().unwrap_or("none");
    println!(
        "effective pool: {effective_pool}{}",
        source_suffix(effective_pool_source)
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
        AccountStartupEligibility::PreferredAccountDisabled => "preferredAccountDisabled",
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
        AccountStartupEligibility::PreferredAccountDisabled => {
            "preferred account is disabled".to_string()
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

fn status_health_state(
    preview: &AccountStartupSelectionPreview,
    pool: Option<&AccountPoolDiagnostic>,
) -> Option<&'static str> {
    if preview.predicted_account_id.is_some() {
        return Some("healthy");
    }

    let pool = pool?;
    if pool.next_eligible_at.is_some() {
        return Some("coolingDown");
    }

    if pool
        .accounts
        .iter()
        .any(|account| account.enabled && health_state(account) == "healthy")
    {
        return Some("healthy");
    }

    if pool.accounts.is_empty() {
        None
    } else {
        Some("unavailable")
    }
}

fn effective_pool_source(diagnostic: &AccountsStatusDiagnostic) -> Option<AccountSource> {
    let preview = &diagnostic.startup.startup.preview;
    let selected_account_id = preview
        .preferred_account_id
        .as_deref()
        .or(preview.predicted_account_id.as_deref())?;
    let pool = diagnostic.pool.as_ref()?;
    pool.accounts
        .iter()
        .find(|account| account.account_id == selected_account_id)
        .and_then(|account| account.source)
}

fn effective_pool_resolution_source_to_wire_string(
    source: EffectivePoolResolutionSource,
) -> &'static str {
    match source {
        EffectivePoolResolutionSource::Override => "override",
        EffectivePoolResolutionSource::ConfigDefault => "configDefault",
        EffectivePoolResolutionSource::PersistedSelection => "persistedSelection",
        EffectivePoolResolutionSource::None => "none",
    }
}

fn source_suffix(source: Option<AccountSource>) -> String {
    source.map_or_else(String::new, |source| format!(" source={}", source.as_str()))
}

fn normalized_account_eligibility(
    account: &AccountPoolAccountDiagnostic,
    preview: &AccountStartupSelectionPreview,
    pool_observability: Option<&StatusPoolObservabilityView>,
) -> EligibilityView {
    let is_preferred = preview.preferred_account_id.as_deref() == Some(account.account_id.as_str());
    if is_preferred {
        if !account.enabled {
            return EligibilityView {
                code: "preferredAccountDisabled",
                reason: "preferred account is disabled".to_string(),
            };
        }

        if account.active_lease.is_some() {
            return EligibilityView {
                code: "preferredAccountBusy",
                reason: "preferred account is currently leased by another runtime".to_string(),
            };
        }

        match account.health_state {
            Some(AccountHealthState::RateLimited) => {
                if let Some(eligibility) = quota_account_eligibility(
                    account,
                    pool_observability,
                    /*is_preferred*/ true,
                ) {
                    return eligibility;
                }
                return EligibilityView {
                    code: "preferredAccountRateLimited",
                    reason: "preferred account is rate limited".to_string(),
                };
            }
            Some(AccountHealthState::Unauthorized) => {
                return EligibilityView {
                    code: "preferredAccountUnauthorized",
                    reason: "preferred account is unauthorized".to_string(),
                };
            }
            Some(AccountHealthState::Healthy) | None => {}
        }

        if !account.healthy {
            return EligibilityView {
                code: "preferredAccountUnhealthy",
                reason: "preferred account is unhealthy".to_string(),
            };
        }

        if preview.suppressed {
            return EligibilityView {
                code: "suppressed",
                reason: eligibility_reason(&AccountStartupEligibility::Suppressed),
            };
        }

        return EligibilityView {
            code: "preferredAccountSelected",
            reason: "preferred account is selected for startup".to_string(),
        };
    }

    if !account.enabled {
        return EligibilityView {
            code: "disabled",
            reason: "account is disabled".to_string(),
        };
    }

    if account.active_lease.is_some() {
        return EligibilityView {
            code: "busy",
            reason: "account is currently leased by another runtime".to_string(),
        };
    }

    match account.health_state {
        Some(AccountHealthState::RateLimited) => {
            if let Some(eligibility) =
                quota_account_eligibility(account, pool_observability, /*is_preferred*/ false)
            {
                return eligibility;
            }
            return EligibilityView {
                code: "rateLimited",
                reason: "account is rate limited".to_string(),
            };
        }
        Some(AccountHealthState::Unauthorized) => {
            return EligibilityView {
                code: "unauthorized",
                reason: "account is unauthorized".to_string(),
            };
        }
        Some(AccountHealthState::Healthy) | None => {}
    }

    if !account.healthy {
        return EligibilityView {
            code: "unhealthy",
            reason: "account is unhealthy".to_string(),
        };
    }

    if preview.suppressed {
        return EligibilityView {
            code: "suppressed",
            reason: eligibility_reason(&AccountStartupEligibility::Suppressed),
        };
    }

    if preview.predicted_account_id.as_deref() == Some(account.account_id.as_str()) {
        return EligibilityView {
            code: "automaticAccountSelected",
            reason: "account is selected for automatic startup selection".to_string(),
        };
    }

    EligibilityView {
        code: "eligible",
        reason: "account is eligible for automatic startup selection".to_string(),
    }
}

fn quota_account_eligibility(
    account: &AccountPoolAccountDiagnostic,
    pool_observability: Option<&StatusPoolObservabilityView>,
    is_preferred: bool,
) -> Option<EligibilityView> {
    let account = observed_account(pool_observability, &account.account_id)?;
    quota_eligibility_from_families(&account.quotas, is_preferred)
}

fn observed_account<'a>(
    pool_observability: Option<&'a StatusPoolObservabilityView>,
    account_id: &str,
) -> Option<&'a PoolAccountView> {
    pool_observability?
        .accounts
        .as_ref()?
        .iter()
        .find(|account| account.account_id == account_id)
}

fn quota_eligibility_from_families(
    quotas: &[PoolQuotaFamilyView],
    is_preferred: bool,
) -> Option<EligibilityView> {
    if quotas.is_empty() {
        return None;
    }
    if quotas.iter().any(|quota| quota.next_probe_after.is_some()) {
        return Some(quota_eligibility_view(
            is_preferred,
            "probeThrottle",
            "preferredAccountProbeThrottle",
            "account is waiting for the next quota probe",
            "preferred account is waiting for the next quota probe",
        ));
    }
    if quotas
        .iter()
        .any(|quota| matches!(quota.exhausted_windows.as_str(), "secondary" | "both"))
    {
        return Some(quota_eligibility_view(
            is_preferred,
            "secondaryWindowBlocked",
            "preferredAccountSecondaryWindowBlocked",
            "account is blocked by the secondary quota window",
            "preferred account is blocked by the secondary quota window",
        ));
    }
    if quotas
        .iter()
        .any(|quota| quota.exhausted_windows.as_str() == "primary")
    {
        return Some(quota_eligibility_view(
            is_preferred,
            "primaryWindowBlocked",
            "preferredAccountPrimaryWindowBlocked",
            "account is blocked by the primary quota window",
            "preferred account is blocked by the primary quota window",
        ));
    }
    if quotas
        .iter()
        .any(|quota| quota.exhausted_windows.as_str() == "unknown")
    {
        return Some(quota_eligibility_view(
            is_preferred,
            "quotaWindowBlocked",
            "preferredAccountQuotaWindowBlocked",
            "account is blocked by quota state",
            "preferred account is blocked by quota state",
        ));
    }
    None
}

fn quota_eligibility_view(
    is_preferred: bool,
    code: &'static str,
    preferred_code: &'static str,
    reason: &'static str,
    preferred_reason: &'static str,
) -> EligibilityView {
    EligibilityView {
        code: if is_preferred { preferred_code } else { code },
        reason: if is_preferred {
            preferred_reason.to_string()
        } else {
            reason.to_string()
        },
    }
}

fn format_optional_timestamp<T: std::fmt::Display>(value: Option<&T>) -> String {
    format_timestamp(value).unwrap_or_else(|| "none".to_string())
}

fn format_timestamp<T: std::fmt::Display>(value: Option<&T>) -> Option<String> {
    value.map(ToString::to_string)
}
