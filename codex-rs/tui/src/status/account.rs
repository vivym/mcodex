#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StatusAccountDisplay {
    ChatGpt {
        email: Option<String>,
        plan: Option<String>,
    },
    ApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusAccountLeaseDisplay {
    pub(crate) pool_id: Option<String>,
    pub(crate) account_id: Option<String>,
    pub(crate) status: String,
    pub(crate) note: Option<String>,
    pub(crate) proactive_switch_allowed_at: Option<String>,
    pub(crate) next_eligible_at: Option<String>,
    pub(crate) next_probe_after: Option<String>,
    pub(crate) remote_reset: Option<String>,
    pub(crate) quota_families: Vec<StatusAccountQuotaFamilyDisplay>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusAccountQuotaFamilyDisplay {
    pub(crate) limit_id: String,
    pub(crate) primary: StatusAccountQuotaWindowDisplay,
    pub(crate) secondary: StatusAccountQuotaWindowDisplay,
    pub(crate) exhausted_windows: String,
    pub(crate) predicted_blocked_until: Option<String>,
    pub(crate) next_probe_after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusAccountQuotaWindowDisplay {
    pub(crate) used_percent: Option<String>,
    pub(crate) resets_at: Option<String>,
}
