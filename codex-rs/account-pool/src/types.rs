/// Request for choosing the startup account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionRequest;

/// Result of choosing the startup account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionResult {
    pub account_id: String,
}

/// Kind of account available to the startup selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountKind {
    ChatGpt,
    ManualOnly,
}

/// Minimal account record used by the policy test scaffold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRecord {
    pub account_id: String,
    pub healthy: bool,
    pub kind: AccountKind,
}
