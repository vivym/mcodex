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
    pub(crate) remote_reset: Option<String>,
}
