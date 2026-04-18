use anyhow::Result;
use anyhow::bail;
use codex_account_pool::AccountOperationalState;
use codex_account_pool::AccountPoolAccountsListRequest;
use codex_account_pool::AccountPoolAccountsPage;
use codex_account_pool::AccountPoolConfig;
use codex_account_pool::AccountPoolDiagnostics;
use codex_account_pool::AccountPoolDiagnosticsReadRequest;
use codex_account_pool::AccountPoolDiagnosticsSeverity;
use codex_account_pool::AccountPoolDiagnosticsStatus;
use codex_account_pool::AccountPoolIssue;
use codex_account_pool::AccountPoolLease;
use codex_account_pool::AccountPoolObservabilityReader;
use codex_account_pool::AccountPoolQuota;
use codex_account_pool::AccountPoolReadRequest;
use codex_account_pool::AccountPoolReasonCode;
use codex_account_pool::AccountPoolSelection;
use codex_account_pool::AccountPoolSnapshot;
use codex_account_pool::AccountPoolSummary;
use codex_account_pool::LocalAccountPoolBackend;
use codex_core::config::Config;
use codex_state::StateRuntime;
use std::sync::Arc;

use crate::accounts::PoolShowCommand;
use crate::accounts::diagnostics::read_accounts_startup_status;
use crate::accounts::observability_types::DiagnosticsIssueView;
use crate::accounts::observability_types::DiagnosticsView;
use crate::accounts::observability_types::PoolAccountView;
use crate::accounts::observability_types::PoolLeaseView;
use crate::accounts::observability_types::PoolQuotaView;
use crate::accounts::observability_types::PoolSelectionView;
use crate::accounts::observability_types::PoolShowView;
use crate::accounts::observability_types::PoolSummaryView;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TargetPoolSource {
    CommandArg,
    TopLevelOverride,
    EffectivePool,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ResolvedTargetPool {
    pub pool_id: String,
    pub source: TargetPoolSource,
}

pub(crate) fn resolve_target_pool(
    command_pool: Option<&str>,
    top_level_override: Option<&str>,
    effective_pool_id: Option<&str>,
) -> Result<ResolvedTargetPool> {
    if let (Some(command_pool), Some(top_level_override)) = (command_pool, top_level_override)
        && command_pool != top_level_override
    {
        bail!("--pool `{command_pool}` conflicts with --account-pool `{top_level_override}`");
    }

    if let Some(command_pool) = command_pool {
        return Ok(ResolvedTargetPool {
            pool_id: command_pool.to_owned(),
            source: TargetPoolSource::CommandArg,
        });
    }

    if let Some(top_level_override) = top_level_override {
        return Ok(ResolvedTargetPool {
            pool_id: top_level_override.to_owned(),
            source: TargetPoolSource::TopLevelOverride,
        });
    }

    if let Some(effective_pool_id) = effective_pool_id {
        return Ok(ResolvedTargetPool {
            pool_id: effective_pool_id.to_owned(),
            source: TargetPoolSource::EffectivePool,
        });
    }

    bail!("no account pool is configured; pass --pool <POOL_ID> or configure a pool")
}

pub(crate) async fn read_pool_show(
    runtime: &Arc<StateRuntime>,
    config: &Config,
    top_level_override: Option<&str>,
    command: &PoolShowCommand,
) -> Result<PoolShowView> {
    let target =
        resolve_strict_target_pool(runtime, config, command.pool.as_deref(), top_level_override)
            .await?;

    ensure_pool_exists(runtime, config, &target.pool_id).await?;

    let reader = local_observability_reader(runtime, config);
    let snapshot = reader
        .read_pool(AccountPoolReadRequest {
            pool_id: target.pool_id.clone(),
        })
        .await?;
    let page = reader
        .list_accounts(AccountPoolAccountsListRequest {
            pool_id: target.pool_id,
            cursor: command.cursor.clone(),
            limit: command.limit,
            ..Default::default()
        })
        .await?;

    Ok(map_pool_show(snapshot, page))
}

pub(crate) async fn read_pool_diagnostics(
    runtime: &Arc<StateRuntime>,
    config: &Config,
    top_level_override: Option<&str>,
    command_pool: Option<&str>,
) -> Result<DiagnosticsView> {
    let target =
        resolve_strict_target_pool(runtime, config, command_pool, top_level_override).await?;
    ensure_pool_exists(runtime, config, &target.pool_id).await?;

    let diagnostics = local_observability_reader(runtime, config)
        .read_diagnostics(AccountPoolDiagnosticsReadRequest {
            pool_id: target.pool_id,
        })
        .await?;

    Ok(map_diagnostics(diagnostics))
}

async fn resolve_strict_target_pool(
    runtime: &Arc<StateRuntime>,
    config: &Config,
    command_pool: Option<&str>,
    top_level_override: Option<&str>,
) -> Result<ResolvedTargetPool> {
    if command_pool.is_some() || top_level_override.is_some() {
        return resolve_target_pool(command_pool, top_level_override, None);
    }

    let startup_status = read_accounts_startup_status(runtime, config, None).await?;
    let effective_pool_id = startup_status.startup.preview.effective_pool_id.as_deref();
    resolve_target_pool(None, None, effective_pool_id)
        .map_err(|_| anyhow::anyhow!("no effective pool is configured; pass --pool <POOL_ID>"))
}

fn local_observability_reader(
    runtime: &Arc<StateRuntime>,
    config: &Config,
) -> LocalAccountPoolBackend {
    let lease_ttl_secs = config
        .accounts
        .as_ref()
        .and_then(|accounts| accounts.lease_ttl_secs)
        .unwrap_or(AccountPoolConfig::default().lease_ttl_secs);
    let lease_ttl = AccountPoolConfig {
        lease_ttl_secs,
        ..AccountPoolConfig::default()
    }
    .lease_ttl_duration();
    LocalAccountPoolBackend::new(Arc::clone(runtime), lease_ttl)
}

async fn ensure_pool_exists(runtime: &StateRuntime, config: &Config, pool_id: &str) -> Result<()> {
    let configured_pool_exists = config.accounts.as_ref().is_some_and(|accounts| {
        accounts.default_pool.as_deref() == Some(pool_id)
            || accounts
                .pools
                .as_ref()
                .is_some_and(|pools| pools.contains_key(pool_id))
    });
    let registered_pool_exists = runtime
        .list_account_pool_memberships(None)
        .await?
        .into_iter()
        .any(|membership| membership.pool_id == pool_id);
    if !configured_pool_exists && !registered_pool_exists {
        bail!("account pool `{pool_id}` was not found");
    }

    Ok(())
}

fn map_pool_show(snapshot: AccountPoolSnapshot, page: AccountPoolAccountsPage) -> PoolShowView {
    PoolShowView {
        pool_id: snapshot.pool_id,
        refreshed_at: Some(snapshot.refreshed_at.to_rfc3339()),
        summary: map_summary(snapshot.summary),
        data: page.data.into_iter().map(map_account).collect(),
        next_cursor: page.next_cursor,
    }
}

fn map_summary(summary: AccountPoolSummary) -> PoolSummaryView {
    PoolSummaryView {
        total_accounts: summary.total_accounts,
        active_leases: summary.active_leases,
        available_accounts: summary.available_accounts,
        leased_accounts: summary.leased_accounts,
        paused_accounts: summary.paused_accounts,
        draining_accounts: summary.draining_accounts,
        near_exhausted_accounts: summary.near_exhausted_accounts,
        exhausted_accounts: summary.exhausted_accounts,
        error_accounts: summary.error_accounts,
    }
}

fn map_account(account: codex_account_pool::AccountPoolAccount) -> PoolAccountView {
    PoolAccountView {
        account_id: account.account_id,
        backend_account_ref: account.backend_account_ref,
        account_kind: account.account_kind,
        enabled: account.enabled,
        health_state: account.health_state,
        operational_state: account
            .operational_state
            .map(account_operational_state_to_string),
        allocatable: account.allocatable,
        status_reason_code: account
            .status_reason_code
            .map(account_pool_reason_code_to_string),
        status_message: account.status_message,
        current_lease: account.current_lease.map(map_lease),
        quota: account.quota.map(map_quota),
        selection: account.selection.map(map_selection),
        updated_at: Some(account.updated_at.to_rfc3339()),
    }
}

fn map_lease(lease: AccountPoolLease) -> PoolLeaseView {
    PoolLeaseView {
        lease_id: lease.lease_id,
        lease_epoch: lease.lease_epoch,
        holder_instance_id: lease.holder_instance_id,
        acquired_at: lease.acquired_at.to_rfc3339(),
        renewed_at: lease.renewed_at.to_rfc3339(),
        expires_at: lease.expires_at.to_rfc3339(),
    }
}

fn map_quota(quota: AccountPoolQuota) -> PoolQuotaView {
    PoolQuotaView {
        remaining_percent: quota.remaining_percent,
        resets_at: quota.resets_at.map(|resets_at| resets_at.to_rfc3339()),
        observed_at: quota.observed_at.to_rfc3339(),
    }
}

fn map_selection(selection: AccountPoolSelection) -> PoolSelectionView {
    PoolSelectionView {
        eligible: selection.eligible,
        next_eligible_at: selection
            .next_eligible_at
            .map(|next_eligible_at| next_eligible_at.to_rfc3339()),
        preferred: selection.preferred,
        suppressed: selection.suppressed,
    }
}

fn map_diagnostics(diagnostics: AccountPoolDiagnostics) -> DiagnosticsView {
    DiagnosticsView {
        pool_id: diagnostics.pool_id,
        generated_at: Some(diagnostics.generated_at.to_rfc3339()),
        status: account_pool_diagnostics_status_to_string(diagnostics.status),
        issues: diagnostics.issues.into_iter().map(map_issue).collect(),
    }
}

fn map_issue(issue: AccountPoolIssue) -> DiagnosticsIssueView {
    DiagnosticsIssueView {
        severity: account_pool_diagnostics_severity_to_string(issue.severity),
        reason_code: account_pool_reason_code_to_string(issue.reason_code),
        message: issue.message,
        account_id: issue.account_id,
        holder_instance_id: issue.holder_instance_id,
        next_relevant_at: issue
            .next_relevant_at
            .map(|next_relevant_at| next_relevant_at.to_rfc3339()),
    }
}

fn account_operational_state_to_string(state: AccountOperationalState) -> String {
    state.as_str().to_string()
}

fn account_pool_reason_code_to_string(reason_code: AccountPoolReasonCode) -> String {
    match reason_code {
        AccountPoolReasonCode::DurablySuppressed => "durablySuppressed",
        AccountPoolReasonCode::MissingPool => "missingPool",
        AccountPoolReasonCode::PreferredAccountSelected => "preferredAccountSelected",
        AccountPoolReasonCode::AutomaticAccountSelected => "automaticAccountSelected",
        AccountPoolReasonCode::PreferredAccountMissing => "preferredAccountMissing",
        AccountPoolReasonCode::PreferredAccountInOtherPool => "preferredAccountInOtherPool",
        AccountPoolReasonCode::PreferredAccountDisabled => "preferredAccountDisabled",
        AccountPoolReasonCode::PreferredAccountUnhealthy => "preferredAccountUnhealthy",
        AccountPoolReasonCode::PreferredAccountBusy => "preferredAccountBusy",
        AccountPoolReasonCode::ManualPause => "manualPause",
        AccountPoolReasonCode::ManualDrain => "manualDrain",
        AccountPoolReasonCode::QuotaNearExhausted => "quotaNearExhausted",
        AccountPoolReasonCode::QuotaExhausted => "quotaExhausted",
        AccountPoolReasonCode::AuthFailure => "authFailure",
        AccountPoolReasonCode::CooldownActive => "cooldownActive",
        AccountPoolReasonCode::MinimumSwitchInterval => "minimumSwitchInterval",
        AccountPoolReasonCode::NoEligibleAccount => "noEligibleAccount",
        AccountPoolReasonCode::LeaseHeldByAnotherInstance => "leaseHeldByAnotherInstance",
        AccountPoolReasonCode::NonReplayableTurn => "nonReplayableTurn",
        AccountPoolReasonCode::Unknown => "unknown",
    }
    .to_string()
}

fn account_pool_diagnostics_status_to_string(status: AccountPoolDiagnosticsStatus) -> String {
    match status {
        AccountPoolDiagnosticsStatus::Healthy => "healthy",
        AccountPoolDiagnosticsStatus::Degraded => "degraded",
        AccountPoolDiagnosticsStatus::Blocked => "blocked",
    }
    .to_string()
}

fn account_pool_diagnostics_severity_to_string(severity: AccountPoolDiagnosticsSeverity) -> String {
    match severity {
        AccountPoolDiagnosticsSeverity::Info => "info",
        AccountPoolDiagnosticsSeverity::Warning => "warning",
        AccountPoolDiagnosticsSeverity::Error => "error",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::ResolvedTargetPool;
    use super::TargetPoolSource;
    use super::resolve_target_pool;
    use pretty_assertions::assert_eq;

    #[test]
    fn resolve_target_pool_rejects_conflicting_command_and_override_pool_ids() {
        let err = resolve_target_pool(
            Some("team-command"),
            Some("team-override"),
            Some("team-effective"),
        )
        .expect_err("expected conflict");

        assert!(err.to_string().contains("conflicts with --account-pool"));
    }

    #[test]
    fn resolve_target_pool_prefers_command_arg_when_present() {
        let target = resolve_target_pool(
            Some("team-command"),
            Some("team-command"),
            Some("team-effective"),
        )
        .expect("command pool should resolve");

        assert_eq!(
            target,
            ResolvedTargetPool {
                pool_id: "team-command".to_owned(),
                source: TargetPoolSource::CommandArg,
            }
        );
    }

    #[test]
    fn resolve_target_pool_prefers_top_level_override_when_command_arg_is_absent() {
        let target = resolve_target_pool(None, Some("team-override"), Some("team-effective"))
            .expect("top-level override should resolve");

        assert_eq!(
            target,
            ResolvedTargetPool {
                pool_id: "team-override".to_owned(),
                source: TargetPoolSource::TopLevelOverride,
            }
        );
    }

    #[test]
    fn resolve_target_pool_prefers_effective_pool_when_no_explicit_sources_exist() {
        let target = resolve_target_pool(None, None, Some("team-effective"))
            .expect("effective pool should resolve");

        assert_eq!(
            target,
            ResolvedTargetPool {
                pool_id: "team-effective".to_owned(),
                source: TargetPoolSource::EffectivePool,
            }
        );
    }

    #[test]
    fn resolve_target_pool_errors_when_no_pool_can_be_resolved() {
        let err = resolve_target_pool(None, None, None).expect_err("expected missing pool error");

        assert!(
            err.to_string().contains(
                "no account pool is configured; pass --pool <POOL_ID> or configure a pool"
            )
        );
    }
}
