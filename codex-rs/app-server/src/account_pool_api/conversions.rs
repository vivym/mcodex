use codex_account_pool as pool;
use codex_app_server_protocol::AccountOperationalState;
use codex_app_server_protocol::AccountPoolAccountResponse;
use codex_app_server_protocol::AccountPoolDiagnosticsIssueResponse;
use codex_app_server_protocol::AccountPoolDiagnosticsSeverity;
use codex_app_server_protocol::AccountPoolDiagnosticsStatus;
use codex_app_server_protocol::AccountPoolEventResponse;
use codex_app_server_protocol::AccountPoolEventType;
use codex_app_server_protocol::AccountPoolLeaseResponse;
use codex_app_server_protocol::AccountPoolQuotaFamilyResponse;
use codex_app_server_protocol::AccountPoolQuotaResponse;
use codex_app_server_protocol::AccountPoolQuotaWindowResponse;
use codex_app_server_protocol::AccountPoolReasonCode;
use codex_app_server_protocol::AccountPoolSelectionResponse;
use codex_app_server_protocol::AccountPoolSummaryResponse;

pub(super) fn account_pool_summary_response(
    summary: pool::AccountPoolSummary,
) -> AccountPoolSummaryResponse {
    AccountPoolSummaryResponse {
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

pub(super) fn account_pool_account_response(
    account: pool::AccountPoolAccount,
) -> AccountPoolAccountResponse {
    AccountPoolAccountResponse {
        account_id: account.account_id,
        backend_account_ref: account.backend_account_ref,
        account_kind: account.account_kind,
        selection_family: account.selection_family,
        enabled: account.enabled,
        health_state: account.health_state,
        operational_state: account
            .operational_state
            .map(account_operational_state_response),
        allocatable: account.allocatable,
        status_reason_code: account
            .status_reason_code
            .map(account_pool_reason_code_response),
        status_message: account.status_message,
        current_lease: account.current_lease.map(account_pool_lease_response),
        quota: account.quota.map(account_pool_quota_response),
        quotas: account
            .quotas
            .into_iter()
            .map(account_pool_quota_family_response)
            .collect(),
        selection: account.selection.map(account_pool_selection_response),
        updated_at: account.updated_at.timestamp(),
    }
}

pub(super) fn account_pool_event_response(
    event: pool::AccountPoolEvent,
) -> AccountPoolEventResponse {
    AccountPoolEventResponse {
        event_id: event.event_id,
        occurred_at: event.occurred_at.timestamp(),
        pool_id: event.pool_id,
        account_id: event.account_id,
        lease_id: event.lease_id,
        holder_instance_id: event.holder_instance_id,
        event_type: account_pool_event_type_response(event.event_type),
        reason_code: event.reason_code.map(account_pool_reason_code_response),
        message: event.message,
        details: event.details_json,
    }
}

pub(super) fn account_pool_diagnostics_issue_response(
    issue: pool::AccountPoolIssue,
) -> AccountPoolDiagnosticsIssueResponse {
    AccountPoolDiagnosticsIssueResponse {
        severity: account_pool_diagnostics_severity_response(issue.severity),
        reason_code: account_pool_reason_code_response(issue.reason_code),
        message: issue.message,
        account_id: issue.account_id,
        holder_instance_id: issue.holder_instance_id,
        next_relevant_at: issue
            .next_relevant_at
            .map(|next_relevant_at| next_relevant_at.timestamp()),
    }
}

pub(super) fn account_operational_state_to_pool_state(
    state: AccountOperationalState,
) -> pool::AccountOperationalState {
    match state {
        AccountOperationalState::Available => pool::AccountOperationalState::Available,
        AccountOperationalState::Leased => pool::AccountOperationalState::Leased,
        AccountOperationalState::Paused => pool::AccountOperationalState::Paused,
        AccountOperationalState::Draining => pool::AccountOperationalState::Draining,
        AccountOperationalState::CoolingDown => pool::AccountOperationalState::CoolingDown,
        AccountOperationalState::NearExhausted => pool::AccountOperationalState::NearExhausted,
        AccountOperationalState::Exhausted => pool::AccountOperationalState::Exhausted,
        AccountOperationalState::Error => pool::AccountOperationalState::Error,
    }
}

pub(super) fn account_pool_event_type_to_pool_event_type(
    event_type: AccountPoolEventType,
) -> pool::AccountPoolEventType {
    match event_type {
        AccountPoolEventType::LeaseAcquired => pool::AccountPoolEventType::LeaseAcquired,
        AccountPoolEventType::LeaseRenewed => pool::AccountPoolEventType::LeaseRenewed,
        AccountPoolEventType::LeaseReleased => pool::AccountPoolEventType::LeaseReleased,
        AccountPoolEventType::LeaseAcquireFailed => pool::AccountPoolEventType::LeaseAcquireFailed,
        AccountPoolEventType::ProactiveSwitchSelected => {
            pool::AccountPoolEventType::ProactiveSwitchSelected
        }
        AccountPoolEventType::ProactiveSwitchSuppressed => {
            pool::AccountPoolEventType::ProactiveSwitchSuppressed
        }
        AccountPoolEventType::QuotaObserved => pool::AccountPoolEventType::QuotaObserved,
        AccountPoolEventType::QuotaNearExhausted => pool::AccountPoolEventType::QuotaNearExhausted,
        AccountPoolEventType::QuotaExhausted => pool::AccountPoolEventType::QuotaExhausted,
        AccountPoolEventType::AccountPaused => pool::AccountPoolEventType::AccountPaused,
        AccountPoolEventType::AccountResumed => pool::AccountPoolEventType::AccountResumed,
        AccountPoolEventType::AccountDrainingStarted => {
            pool::AccountPoolEventType::AccountDrainingStarted
        }
        AccountPoolEventType::AccountDrainingCleared => {
            pool::AccountPoolEventType::AccountDrainingCleared
        }
        AccountPoolEventType::AuthFailed => pool::AccountPoolEventType::AuthFailed,
        AccountPoolEventType::CooldownStarted => pool::AccountPoolEventType::CooldownStarted,
        AccountPoolEventType::CooldownCleared => pool::AccountPoolEventType::CooldownCleared,
    }
}

pub(super) fn account_pool_diagnostics_status_response(
    status: pool::AccountPoolDiagnosticsStatus,
) -> AccountPoolDiagnosticsStatus {
    match status {
        pool::AccountPoolDiagnosticsStatus::Healthy => AccountPoolDiagnosticsStatus::Healthy,
        pool::AccountPoolDiagnosticsStatus::Degraded => AccountPoolDiagnosticsStatus::Degraded,
        pool::AccountPoolDiagnosticsStatus::Blocked => AccountPoolDiagnosticsStatus::Blocked,
    }
}

fn account_pool_lease_response(lease: pool::AccountPoolLease) -> AccountPoolLeaseResponse {
    AccountPoolLeaseResponse {
        lease_id: lease.lease_id,
        lease_epoch: lease.lease_epoch,
        holder_instance_id: lease.holder_instance_id,
        acquired_at: lease.acquired_at.timestamp(),
        renewed_at: lease.renewed_at.timestamp(),
        expires_at: lease.expires_at.timestamp(),
    }
}

fn account_pool_quota_response(quota: pool::AccountPoolQuota) -> AccountPoolQuotaResponse {
    AccountPoolQuotaResponse {
        remaining_percent: quota.remaining_percent,
        resets_at: quota.resets_at.map(|resets_at| resets_at.timestamp()),
        observed_at: quota.observed_at.timestamp(),
    }
}

fn account_pool_quota_family_response(
    quota: pool::AccountPoolQuotaFamily,
) -> AccountPoolQuotaFamilyResponse {
    AccountPoolQuotaFamilyResponse {
        limit_id: quota.limit_id,
        primary: account_pool_quota_window_response(quota.primary),
        secondary: account_pool_quota_window_response(quota.secondary),
        exhausted_windows: quota.exhausted_windows,
        predicted_blocked_until: quota
            .predicted_blocked_until
            .map(|predicted_blocked_until| predicted_blocked_until.timestamp()),
        next_probe_after: quota
            .next_probe_after
            .map(|next_probe_after| next_probe_after.timestamp()),
        observed_at: quota.observed_at.timestamp(),
    }
}

fn account_pool_quota_window_response(
    window: pool::AccountPoolQuotaWindow,
) -> AccountPoolQuotaWindowResponse {
    AccountPoolQuotaWindowResponse {
        used_percent: window.used_percent,
        resets_at: window.resets_at.map(|resets_at| resets_at.timestamp()),
    }
}

fn account_pool_selection_response(
    selection: pool::AccountPoolSelection,
) -> AccountPoolSelectionResponse {
    AccountPoolSelectionResponse {
        eligible: selection.eligible,
        next_eligible_at: selection
            .next_eligible_at
            .map(|next_eligible_at| next_eligible_at.timestamp()),
        preferred: selection.preferred,
        suppressed: selection.suppressed,
    }
}

fn account_operational_state_response(
    state: pool::AccountOperationalState,
) -> AccountOperationalState {
    match state {
        pool::AccountOperationalState::Available => AccountOperationalState::Available,
        pool::AccountOperationalState::Leased => AccountOperationalState::Leased,
        pool::AccountOperationalState::Paused => AccountOperationalState::Paused,
        pool::AccountOperationalState::Draining => AccountOperationalState::Draining,
        pool::AccountOperationalState::CoolingDown => AccountOperationalState::CoolingDown,
        pool::AccountOperationalState::NearExhausted => AccountOperationalState::NearExhausted,
        pool::AccountOperationalState::Exhausted => AccountOperationalState::Exhausted,
        pool::AccountOperationalState::Error => AccountOperationalState::Error,
    }
}

fn account_pool_event_type_response(
    event_type: pool::AccountPoolEventType,
) -> AccountPoolEventType {
    match event_type {
        pool::AccountPoolEventType::LeaseAcquired => AccountPoolEventType::LeaseAcquired,
        pool::AccountPoolEventType::LeaseRenewed => AccountPoolEventType::LeaseRenewed,
        pool::AccountPoolEventType::LeaseReleased => AccountPoolEventType::LeaseReleased,
        pool::AccountPoolEventType::LeaseAcquireFailed => AccountPoolEventType::LeaseAcquireFailed,
        pool::AccountPoolEventType::ProactiveSwitchSelected => {
            AccountPoolEventType::ProactiveSwitchSelected
        }
        pool::AccountPoolEventType::ProactiveSwitchSuppressed => {
            AccountPoolEventType::ProactiveSwitchSuppressed
        }
        pool::AccountPoolEventType::QuotaObserved => AccountPoolEventType::QuotaObserved,
        pool::AccountPoolEventType::QuotaNearExhausted => AccountPoolEventType::QuotaNearExhausted,
        pool::AccountPoolEventType::QuotaExhausted => AccountPoolEventType::QuotaExhausted,
        pool::AccountPoolEventType::AccountPaused => AccountPoolEventType::AccountPaused,
        pool::AccountPoolEventType::AccountResumed => AccountPoolEventType::AccountResumed,
        pool::AccountPoolEventType::AccountDrainingStarted => {
            AccountPoolEventType::AccountDrainingStarted
        }
        pool::AccountPoolEventType::AccountDrainingCleared => {
            AccountPoolEventType::AccountDrainingCleared
        }
        pool::AccountPoolEventType::AuthFailed => AccountPoolEventType::AuthFailed,
        pool::AccountPoolEventType::CooldownStarted => AccountPoolEventType::CooldownStarted,
        pool::AccountPoolEventType::CooldownCleared => AccountPoolEventType::CooldownCleared,
    }
}

fn account_pool_reason_code_response(
    reason_code: pool::AccountPoolReasonCode,
) -> AccountPoolReasonCode {
    match reason_code {
        pool::AccountPoolReasonCode::DurablySuppressed => AccountPoolReasonCode::DurablySuppressed,
        pool::AccountPoolReasonCode::MissingPool => AccountPoolReasonCode::MissingPool,
        pool::AccountPoolReasonCode::PreferredAccountSelected => {
            AccountPoolReasonCode::PreferredAccountSelected
        }
        pool::AccountPoolReasonCode::AutomaticAccountSelected => {
            AccountPoolReasonCode::AutomaticAccountSelected
        }
        pool::AccountPoolReasonCode::PreferredAccountMissing => {
            AccountPoolReasonCode::PreferredAccountMissing
        }
        pool::AccountPoolReasonCode::PreferredAccountInOtherPool => {
            AccountPoolReasonCode::PreferredAccountInOtherPool
        }
        pool::AccountPoolReasonCode::PreferredAccountDisabled => {
            AccountPoolReasonCode::PreferredAccountDisabled
        }
        pool::AccountPoolReasonCode::PreferredAccountUnhealthy => {
            AccountPoolReasonCode::PreferredAccountUnhealthy
        }
        pool::AccountPoolReasonCode::PreferredAccountBusy => {
            AccountPoolReasonCode::PreferredAccountBusy
        }
        pool::AccountPoolReasonCode::ManualPause => AccountPoolReasonCode::ManualPause,
        pool::AccountPoolReasonCode::ManualDrain => AccountPoolReasonCode::ManualDrain,
        pool::AccountPoolReasonCode::QuotaNearExhausted => {
            AccountPoolReasonCode::QuotaNearExhausted
        }
        pool::AccountPoolReasonCode::QuotaExhausted => AccountPoolReasonCode::QuotaExhausted,
        pool::AccountPoolReasonCode::AuthFailure => AccountPoolReasonCode::AuthFailure,
        pool::AccountPoolReasonCode::CooldownActive => AccountPoolReasonCode::CooldownActive,
        pool::AccountPoolReasonCode::MinimumSwitchInterval => {
            AccountPoolReasonCode::MinimumSwitchInterval
        }
        pool::AccountPoolReasonCode::NoEligibleAccount => AccountPoolReasonCode::NoEligibleAccount,
        pool::AccountPoolReasonCode::LeaseHeldByAnotherInstance => {
            AccountPoolReasonCode::LeaseHeldByAnotherInstance
        }
        pool::AccountPoolReasonCode::NonReplayableTurn => AccountPoolReasonCode::NonReplayableTurn,
        pool::AccountPoolReasonCode::Unknown => AccountPoolReasonCode::Unknown,
    }
}

fn account_pool_diagnostics_severity_response(
    severity: pool::AccountPoolDiagnosticsSeverity,
) -> AccountPoolDiagnosticsSeverity {
    match severity {
        pool::AccountPoolDiagnosticsSeverity::Info => AccountPoolDiagnosticsSeverity::Info,
        pool::AccountPoolDiagnosticsSeverity::Warning => AccountPoolDiagnosticsSeverity::Warning,
        pool::AccountPoolDiagnosticsSeverity::Error => AccountPoolDiagnosticsSeverity::Error,
    }
}
