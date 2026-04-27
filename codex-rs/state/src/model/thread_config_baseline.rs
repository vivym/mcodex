use anyhow::Result;
use codex_protocol::ThreadId;
use codex_protocol::config_types::ApprovalsReviewer;
use codex_protocol::config_types::Personality;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::SandboxPolicy;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use std::path::PathBuf;

/// Persisted source-thread config baseline used to reconstruct unloaded threads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadConfigBaselineSnapshot {
    /// Thread whose durable baseline this snapshot represents.
    pub thread_id: ThreadId,
    /// Latest effective model slug for the thread.
    pub model: String,
    /// Latest effective model provider identifier.
    pub model_provider_id: String,
    /// Latest effective service tier.
    pub service_tier: Option<ServiceTier>,
    /// Latest effective approval policy.
    pub approval_policy: AskForApproval,
    /// Latest effective approval reviewer routing.
    pub approvals_reviewer: ApprovalsReviewer,
    /// Latest effective sandbox policy.
    pub sandbox_policy: SandboxPolicy,
    /// Latest effective cwd.
    pub cwd: PathBuf,
    /// Latest effective reasoning effort.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Latest effective personality.
    pub personality: Option<Personality>,
    /// Whether the persisted personality should override rollout turn context
    /// when reconstructing an unloaded thread.
    pub personality_overrides_rollout: bool,
    /// Latest effective base instructions.
    pub base_instructions: Option<String>,
    /// Latest effective developer instructions.
    pub developer_instructions: Option<String>,
    /// Whether the persisted developer instructions should override rollout
    /// turn context when reconstructing an unloaded thread.
    pub developer_instructions_overrides_rollout: bool,
}

#[derive(Debug)]
pub(crate) struct ThreadConfigBaselineRow {
    thread_id: String,
    model: String,
    model_provider_id: String,
    service_tier: Option<String>,
    approval_policy: String,
    approvals_reviewer: String,
    sandbox_policy: String,
    cwd: String,
    reasoning_effort: Option<String>,
    personality: Option<String>,
    personality_overrides_rollout: bool,
    base_instructions: Option<String>,
    developer_instructions: Option<String>,
    developer_instructions_overrides_rollout: bool,
}

impl ThreadConfigBaselineRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            thread_id: row.try_get("thread_id")?,
            model: row.try_get("model")?,
            model_provider_id: row.try_get("model_provider_id")?,
            service_tier: row.try_get("service_tier")?,
            approval_policy: row.try_get("approval_policy")?,
            approvals_reviewer: row.try_get("approvals_reviewer")?,
            sandbox_policy: row.try_get("sandbox_policy")?,
            cwd: row.try_get("cwd")?,
            reasoning_effort: row.try_get("reasoning_effort")?,
            personality: row.try_get("personality")?,
            personality_overrides_rollout: row.try_get("personality_overrides_rollout")?,
            base_instructions: row.try_get("base_instructions")?,
            developer_instructions: row.try_get("developer_instructions")?,
            developer_instructions_overrides_rollout: row
                .try_get("developer_instructions_overrides_rollout")?,
        })
    }
}

impl TryFrom<ThreadConfigBaselineRow> for ThreadConfigBaselineSnapshot {
    type Error = anyhow::Error;

    fn try_from(row: ThreadConfigBaselineRow) -> std::result::Result<Self, Self::Error> {
        let ThreadConfigBaselineRow {
            thread_id,
            model,
            model_provider_id,
            service_tier,
            approval_policy,
            approvals_reviewer,
            sandbox_policy,
            cwd,
            reasoning_effort,
            personality,
            personality_overrides_rollout,
            base_instructions,
            developer_instructions,
            developer_instructions_overrides_rollout,
        } = row;
        Ok(Self {
            thread_id: ThreadId::try_from(thread_id)?,
            model,
            model_provider_id,
            service_tier: service_tier
                .as_deref()
                .map(parse_stringified_value)
                .transpose()?,
            approval_policy: parse_stringified_value(approval_policy.as_str())?,
            approvals_reviewer: parse_stringified_value(approvals_reviewer.as_str())?,
            sandbox_policy: parse_stringified_value(sandbox_policy.as_str())?,
            cwd: PathBuf::from(cwd),
            reasoning_effort: reasoning_effort
                .as_deref()
                .map(parse_stringified_value)
                .transpose()?,
            personality: personality
                .as_deref()
                .map(parse_stringified_value)
                .transpose()?,
            personality_overrides_rollout,
            base_instructions,
            developer_instructions,
            developer_instructions_overrides_rollout,
        })
    }
}

fn parse_stringified_value<T>(value: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    Ok(serde_json::from_value(Value::String(value.to_string()))
        .or_else(|_| serde_json::from_str(value))?)
}
