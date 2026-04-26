#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PoolShowView {
    pub pool_id: String,
    pub refreshed_at: Option<String>,
    pub summary: PoolSummaryView,
    pub data: Vec<PoolAccountView>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PoolSummaryView {
    pub total_accounts: u32,
    pub active_leases: u32,
    pub available_accounts: Option<u32>,
    pub leased_accounts: Option<u32>,
    pub paused_accounts: Option<u32>,
    pub draining_accounts: Option<u32>,
    pub near_exhausted_accounts: Option<u32>,
    pub exhausted_accounts: Option<u32>,
    pub error_accounts: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PoolAccountView {
    pub account_id: String,
    pub backend_account_ref: Option<String>,
    pub account_kind: String,
    pub enabled: bool,
    pub health_state: Option<String>,
    pub operational_state: Option<String>,
    pub allocatable: Option<bool>,
    pub status_reason_code: Option<String>,
    pub status_message: Option<String>,
    pub current_lease: Option<PoolLeaseView>,
    pub quota: Option<PoolQuotaView>,
    pub quotas: Vec<PoolQuotaFamilyView>,
    pub selection: Option<PoolSelectionView>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PoolLeaseView {
    pub lease_id: String,
    pub lease_epoch: u64,
    pub holder_instance_id: String,
    pub acquired_at: String,
    pub renewed_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PoolQuotaView {
    pub remaining_percent: Option<f64>,
    pub resets_at: Option<String>,
    pub observed_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PoolQuotaFamilyView {
    pub limit_id: String,
    pub primary: PoolQuotaWindowView,
    pub secondary: PoolQuotaWindowView,
    pub exhausted_windows: String,
    pub predicted_blocked_until: Option<String>,
    pub next_probe_after: Option<String>,
    pub next_probe_after_is_future: bool,
    pub observed_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PoolQuotaWindowView {
    pub used_percent: Option<f64>,
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PoolSelectionView {
    pub eligible: bool,
    pub next_eligible_at: Option<String>,
    pub preferred: bool,
    pub suppressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiagnosticsView {
    pub pool_id: String,
    pub generated_at: Option<String>,
    pub status: String,
    pub issues: Vec<DiagnosticsIssueView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiagnosticsIssueView {
    pub severity: String,
    pub reason_code: String,
    pub message: String,
    pub account_id: Option<String>,
    pub holder_instance_id: Option<String>,
    pub next_relevant_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EventsView {
    pub pool_id: String,
    pub data: Vec<EventView>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EventView {
    pub event_id: String,
    pub occurred_at: String,
    pub pool_id: String,
    pub account_id: Option<String>,
    pub lease_id: Option<String>,
    pub holder_instance_id: Option<String>,
    pub event_type: String,
    pub reason_code: Option<String>,
    pub message: String,
    pub details: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StatusPoolObservabilityView {
    pub pool_id: String,
    pub summary: Option<PoolSummaryView>,
    pub accounts: Option<Vec<PoolAccountView>>,
    pub diagnostics: Option<DiagnosticsView>,
    pub warning: Option<String>,
}

impl StatusPoolObservabilityView {
    pub(crate) fn from_results(
        pool_id: String,
        summary: anyhow::Result<PoolSummaryView>,
        accounts: anyhow::Result<Vec<PoolAccountView>>,
        diagnostics: anyhow::Result<DiagnosticsView>,
    ) -> Self {
        let mut warnings = Vec::new();
        let summary = match summary {
            Ok(summary) => Some(summary),
            Err(err) => {
                warnings.push(err.to_string());
                None
            }
        };
        let accounts = match accounts {
            Ok(accounts) => Some(accounts),
            Err(err) => {
                warnings.push(err.to_string());
                None
            }
        };
        let diagnostics = match diagnostics {
            Ok(diagnostics) => Some(diagnostics),
            Err(err) => {
                warnings.push(err.to_string());
                None
            }
        };

        Self {
            pool_id,
            summary,
            accounts,
            diagnostics,
            warning: (!warnings.is_empty()).then(|| warnings.join("; ")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DiagnosticsView;
    use super::StatusPoolObservabilityView;
    use pretty_assertions::assert_eq;

    #[test]
    fn status_pool_observability_from_results_keeps_diagnostics_when_summary_fails() {
        let view = StatusPoolObservabilityView::from_results(
            "team-main".to_string(),
            Err(anyhow::anyhow!("summary unavailable")),
            Ok(Vec::new()),
            Ok(sample_diagnostics()),
        );

        assert_eq!(
            view,
            StatusPoolObservabilityView {
                pool_id: "team-main".to_string(),
                summary: None,
                accounts: Some(Vec::new()),
                diagnostics: Some(sample_diagnostics()),
                warning: Some("summary unavailable".to_string()),
            }
        );
    }

    #[test]
    fn status_pool_observability_from_results_combines_warnings_when_both_fail() {
        let view = StatusPoolObservabilityView::from_results(
            "team-main".to_string(),
            Err(anyhow::anyhow!("summary unavailable")),
            Ok(Vec::new()),
            Err(anyhow::anyhow!("diagnostics unavailable")),
        );

        assert_eq!(
            view,
            StatusPoolObservabilityView {
                pool_id: "team-main".to_string(),
                summary: None,
                accounts: Some(Vec::new()),
                diagnostics: None,
                warning: Some("summary unavailable; diagnostics unavailable".to_string()),
            }
        );
    }

    fn sample_diagnostics() -> DiagnosticsView {
        DiagnosticsView {
            pool_id: "team-main".to_string(),
            generated_at: None,
            status: "degraded".to_string(),
            issues: Vec::new(),
        }
    }
}
