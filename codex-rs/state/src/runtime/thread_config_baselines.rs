use super::*;
use codex_protocol::config_types::ApprovalsReviewer;
use codex_protocol::config_types::Personality;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::SandboxPolicy;

enum ThreadConfigBaselineUpdateKind {
    LiveIncremental,
    HistoricalReplay,
}

enum ThreadConfigBaselineConflictPolicy {
    ReplaceAll,
    PreserveOverrideOnlyFields,
}

impl StateRuntime {
    pub(crate) async fn upsert_thread_config_baseline_from_rollout_items(
        &self,
        builder: &crate::ThreadMetadataBuilder,
        items: &[RolloutItem],
    ) -> anyhow::Result<()> {
        self.upsert_thread_config_baseline_from_rollout_items_with_kind(
            builder,
            items,
            ThreadConfigBaselineUpdateKind::LiveIncremental,
        )
        .await
    }

    /// Persist a durable config baseline derived from rollout items during
    /// rollout metadata backfill.
    pub async fn backfill_thread_config_baseline_from_rollout_items(
        &self,
        builder: &crate::ThreadMetadataBuilder,
        items: &[RolloutItem],
    ) -> anyhow::Result<()> {
        self.upsert_thread_config_baseline_from_rollout_items_with_kind(
            builder,
            items,
            ThreadConfigBaselineUpdateKind::HistoricalReplay,
        )
        .await
    }

    async fn upsert_thread_config_baseline_from_rollout_items_with_kind(
        &self,
        builder: &crate::ThreadMetadataBuilder,
        items: &[RolloutItem],
        kind: ThreadConfigBaselineUpdateKind,
    ) -> anyhow::Result<()> {
        let existing = self.get_thread_config_baseline(builder.id).await?;
        let Some(mut snapshot) = derive_thread_config_baseline_from_rollout_items(
            builder,
            items,
            existing.as_ref(),
            self.default_provider.as_str(),
        ) else {
            return Ok(());
        };
        if matches!(kind, ThreadConfigBaselineUpdateKind::HistoricalReplay)
            && let Some(existing) = existing.as_ref()
        {
            restore_override_only_fields_from_existing(&mut snapshot, existing);
        }
        match kind {
            ThreadConfigBaselineUpdateKind::LiveIncremental => {
                self.upsert_thread_config_baseline(&snapshot).await
            }
            ThreadConfigBaselineUpdateKind::HistoricalReplay => {
                self.upsert_thread_config_baseline_preserving_override_only_fields(&snapshot)
                    .await
            }
        }
    }
}

#[derive(Debug)]
struct ThreadConfigBaselineAccumulator {
    thread_id: ThreadId,
    model: Option<String>,
    model_provider_id: String,
    service_tier: Option<ServiceTier>,
    approval_policy: AskForApproval,
    approvals_reviewer: ApprovalsReviewer,
    sandbox_policy: SandboxPolicy,
    cwd: PathBuf,
    cwd_known: bool,
    reasoning_effort: Option<ReasoningEffort>,
    personality: Option<Personality>,
    personality_overrides_rollout: bool,
    base_instructions: Option<String>,
    developer_instructions: Option<String>,
    developer_instructions_overrides_rollout: bool,
}

impl ThreadConfigBaselineAccumulator {
    fn from_builder(builder: &crate::ThreadMetadataBuilder, default_provider: &str) -> Self {
        let model_provider_id = builder
            .model_provider
            .clone()
            .filter(|provider| !provider.is_empty())
            .unwrap_or_else(|| default_provider.to_string());
        let cwd_known = !builder.cwd.as_os_str().is_empty();
        Self {
            thread_id: builder.id,
            model: None,
            model_provider_id,
            service_tier: None,
            approval_policy: builder.approval_mode,
            approvals_reviewer: ApprovalsReviewer::User,
            sandbox_policy: builder.sandbox_policy.clone(),
            cwd: builder.cwd.clone(),
            cwd_known,
            reasoning_effort: None,
            personality: None,
            personality_overrides_rollout: false,
            base_instructions: builder.base_instructions.clone(),
            developer_instructions: None,
            developer_instructions_overrides_rollout: false,
        }
    }

    fn from_snapshot(snapshot: &ThreadConfigBaselineSnapshot) -> Self {
        Self {
            thread_id: snapshot.thread_id,
            model: Some(snapshot.model.clone()),
            model_provider_id: snapshot.model_provider_id.clone(),
            service_tier: snapshot.service_tier,
            approval_policy: snapshot.approval_policy,
            approvals_reviewer: snapshot.approvals_reviewer,
            sandbox_policy: snapshot.sandbox_policy.clone(),
            cwd: snapshot.cwd.clone(),
            cwd_known: true,
            reasoning_effort: snapshot.reasoning_effort,
            personality: snapshot.personality,
            personality_overrides_rollout: snapshot.personality_overrides_rollout,
            base_instructions: snapshot.base_instructions.clone(),
            developer_instructions: snapshot.developer_instructions.clone(),
            developer_instructions_overrides_rollout: snapshot
                .developer_instructions_overrides_rollout,
        }
    }

    fn apply_rollout_item(&mut self, item: &RolloutItem) {
        match item {
            RolloutItem::SessionMeta(meta_line) => {
                if meta_line.meta.id != self.thread_id {
                    return;
                }
                if !meta_line.meta.cwd.as_os_str().is_empty() {
                    self.cwd = meta_line.meta.cwd.clone();
                    self.cwd_known = true;
                }
                if let Some(model_provider_id) = meta_line
                    .meta
                    .model_provider
                    .as_ref()
                    .filter(|provider| !provider.is_empty())
                {
                    self.model_provider_id = model_provider_id.clone();
                }
                if let Some(base_instructions) = meta_line.meta.base_instructions.as_ref() {
                    self.base_instructions = Some(base_instructions.text.clone());
                }
            }
            RolloutItem::EventMsg(EventMsg::SessionConfigured(session_configured)) => {
                self.model = Some(session_configured.model.clone());
                self.model_provider_id = session_configured.model_provider_id.clone();
                self.service_tier = session_configured.service_tier;
                self.approval_policy = session_configured.approval_policy;
                self.approvals_reviewer = session_configured.approvals_reviewer;
                self.sandbox_policy = session_configured.sandbox_policy.clone();
                self.cwd = session_configured.cwd.clone().to_path_buf();
                self.cwd_known = true;
                self.reasoning_effort = session_configured.reasoning_effort;
            }
            RolloutItem::TurnContext(turn_context) => {
                self.model = Some(turn_context.model.clone());
                self.service_tier = turn_context.service_tier;
                self.approval_policy = turn_context.approval_policy;
                if let Some(approvals_reviewer) = turn_context.approvals_reviewer {
                    self.approvals_reviewer = approvals_reviewer;
                }
                self.sandbox_policy = turn_context.sandbox_policy.clone();
                self.cwd = turn_context.cwd.clone();
                self.cwd_known = true;
                self.reasoning_effort = turn_context.effort;
                self.personality = turn_context.personality;
                self.personality_overrides_rollout = false;
                self.developer_instructions = turn_context.developer_instructions.clone();
                self.developer_instructions_overrides_rollout = false;
            }
            RolloutItem::EventMsg(_) | RolloutItem::ResponseItem(_) | RolloutItem::Compacted(_) => {
            }
        }
    }

    fn into_snapshot(self) -> Option<ThreadConfigBaselineSnapshot> {
        let model = self.model?;
        if !self.cwd_known {
            return None;
        }
        Some(ThreadConfigBaselineSnapshot {
            thread_id: self.thread_id,
            model,
            model_provider_id: self.model_provider_id,
            service_tier: self.service_tier,
            approval_policy: self.approval_policy,
            approvals_reviewer: self.approvals_reviewer,
            sandbox_policy: self.sandbox_policy,
            cwd: self.cwd,
            reasoning_effort: self.reasoning_effort,
            personality: self.personality,
            personality_overrides_rollout: self.personality_overrides_rollout,
            base_instructions: self.base_instructions,
            developer_instructions: self.developer_instructions,
            developer_instructions_overrides_rollout: self.developer_instructions_overrides_rollout,
        })
    }
}

fn derive_thread_config_baseline_from_rollout_items(
    builder: &crate::ThreadMetadataBuilder,
    items: &[RolloutItem],
    existing: Option<&ThreadConfigBaselineSnapshot>,
    default_provider: &str,
) -> Option<ThreadConfigBaselineSnapshot> {
    let mut accumulator = existing
        .map(ThreadConfigBaselineAccumulator::from_snapshot)
        .unwrap_or_else(|| {
            ThreadConfigBaselineAccumulator::from_builder(builder, default_provider)
        });
    for item in items {
        accumulator.apply_rollout_item(item);
    }
    accumulator.into_snapshot()
}

fn restore_override_only_fields_from_existing(
    snapshot: &mut ThreadConfigBaselineSnapshot,
    existing: &ThreadConfigBaselineSnapshot,
) {
    if existing.personality_overrides_rollout || snapshot.personality.is_none() {
        snapshot.personality = existing.personality;
        snapshot.personality_overrides_rollout = existing.personality_overrides_rollout;
    }
    snapshot.base_instructions = existing.base_instructions.clone();
    if existing.developer_instructions_overrides_rollout
        || snapshot.developer_instructions.is_none()
    {
        snapshot.developer_instructions = existing.developer_instructions.clone();
        snapshot.developer_instructions_overrides_rollout =
            existing.developer_instructions_overrides_rollout;
    }
}

impl StateRuntime {
    /// Load the durable config baseline snapshot for a thread, if one exists.
    pub async fn get_thread_config_baseline(
        &self,
        thread_id: ThreadId,
    ) -> anyhow::Result<Option<ThreadConfigBaselineSnapshot>> {
        let row = sqlx::query(
            r#"
SELECT
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
    developer_instructions_overrides_rollout
FROM thread_config_baselines
WHERE thread_id = ?
            "#,
        )
        .bind(thread_id.to_string())
        .fetch_optional(self.pool.as_ref())
        .await?;
        row.map(|row| {
            ThreadConfigBaselineRow::try_from_row(&row)
                .and_then(ThreadConfigBaselineSnapshot::try_from)
        })
        .transpose()
    }

    /// Persist the latest config baseline snapshot for a thread.
    pub async fn upsert_thread_config_baseline(
        &self,
        snapshot: &ThreadConfigBaselineSnapshot,
    ) -> anyhow::Result<()> {
        self.upsert_thread_config_baseline_with_conflict_policy(
            snapshot,
            ThreadConfigBaselineConflictPolicy::ReplaceAll,
        )
        .await
    }

    async fn upsert_thread_config_baseline_preserving_override_only_fields(
        &self,
        snapshot: &ThreadConfigBaselineSnapshot,
    ) -> anyhow::Result<()> {
        self.upsert_thread_config_baseline_with_conflict_policy(
            snapshot,
            ThreadConfigBaselineConflictPolicy::PreserveOverrideOnlyFields,
        )
        .await
    }

    async fn upsert_thread_config_baseline_with_conflict_policy(
        &self,
        snapshot: &ThreadConfigBaselineSnapshot,
        conflict_policy: ThreadConfigBaselineConflictPolicy,
    ) -> anyhow::Result<()> {
        let conflict_assignments = match conflict_policy {
            ThreadConfigBaselineConflictPolicy::ReplaceAll => {
                r#"
    model = excluded.model,
    model_provider_id = excluded.model_provider_id,
    service_tier = excluded.service_tier,
    approval_policy = excluded.approval_policy,
    approvals_reviewer = excluded.approvals_reviewer,
    sandbox_policy = excluded.sandbox_policy,
    cwd = excluded.cwd,
    reasoning_effort = excluded.reasoning_effort,
    personality = excluded.personality,
    personality_overrides_rollout = excluded.personality_overrides_rollout,
    base_instructions = excluded.base_instructions,
    developer_instructions = excluded.developer_instructions,
    developer_instructions_overrides_rollout = excluded.developer_instructions_overrides_rollout
                "#
            }
            ThreadConfigBaselineConflictPolicy::PreserveOverrideOnlyFields => {
                r#"
    model = excluded.model,
    model_provider_id = excluded.model_provider_id,
    service_tier = excluded.service_tier,
    approval_policy = excluded.approval_policy,
    approvals_reviewer = excluded.approvals_reviewer,
    sandbox_policy = excluded.sandbox_policy,
    cwd = excluded.cwd,
    reasoning_effort = excluded.reasoning_effort,
    personality = CASE
        WHEN thread_config_baselines.personality_overrides_rollout OR excluded.personality IS NULL
        THEN thread_config_baselines.personality
        ELSE excluded.personality
    END,
    personality_overrides_rollout = CASE
        WHEN thread_config_baselines.personality_overrides_rollout OR excluded.personality IS NULL
        THEN thread_config_baselines.personality_overrides_rollout
        ELSE excluded.personality_overrides_rollout
    END,
    base_instructions = thread_config_baselines.base_instructions,
    developer_instructions = CASE
        WHEN thread_config_baselines.developer_instructions_overrides_rollout OR excluded.developer_instructions IS NULL
        THEN thread_config_baselines.developer_instructions
        ELSE excluded.developer_instructions
    END,
    developer_instructions_overrides_rollout = CASE
        WHEN thread_config_baselines.developer_instructions_overrides_rollout OR excluded.developer_instructions IS NULL
        THEN thread_config_baselines.developer_instructions_overrides_rollout
        ELSE excluded.developer_instructions_overrides_rollout
    END
                "#
            }
        };
        let sql = format!(
            r#"
INSERT INTO thread_config_baselines (
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
    developer_instructions_overrides_rollout
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(thread_id) DO UPDATE SET
{conflict_assignments}
            "#,
        );
        sqlx::query(sql.as_str())
            .bind(snapshot.thread_id.to_string())
            .bind(snapshot.model.as_str())
            .bind(snapshot.model_provider_id.as_str())
            .bind(
                snapshot
                    .service_tier
                    .as_ref()
                    .map(crate::extract::enum_to_string),
            )
            .bind(crate::extract::enum_to_string(&snapshot.approval_policy))
            .bind(crate::extract::enum_to_string(&snapshot.approvals_reviewer))
            .bind(crate::extract::enum_to_string(&snapshot.sandbox_policy))
            .bind(snapshot.cwd.display().to_string())
            .bind(
                snapshot
                    .reasoning_effort
                    .as_ref()
                    .map(crate::extract::enum_to_string),
            )
            .bind(
                snapshot
                    .personality
                    .as_ref()
                    .map(crate::extract::enum_to_string),
            )
            .bind(snapshot.personality_overrides_rollout)
            .bind(snapshot.base_instructions.as_deref())
            .bind(snapshot.developer_instructions.as_deref())
            .bind(snapshot.developer_instructions_overrides_rollout)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::test_support::test_thread_metadata;
    use crate::runtime::test_support::unique_temp_dir;
    use chrono::Utc;
    use codex_protocol::config_types::ApprovalsReviewer;
    use codex_protocol::config_types::Personality;
    use codex_protocol::config_types::ReasoningSummary as ReasoningSummaryConfig;
    use codex_protocol::config_types::ServiceTier;
    use codex_protocol::models::BaseInstructions;
    use codex_protocol::openai_models::ReasoningEffort;
    use codex_protocol::protocol::AskForApproval;
    use codex_protocol::protocol::EventMsg;
    use codex_protocol::protocol::RolloutItem;
    use codex_protocol::protocol::SandboxPolicy;
    use codex_protocol::protocol::SessionConfiguredEvent;
    use codex_protocol::protocol::SessionMeta;
    use codex_protocol::protocol::SessionMetaLine;
    use pretty_assertions::assert_eq;

    fn test_snapshot(thread_id: ThreadId) -> ThreadConfigBaselineSnapshot {
        ThreadConfigBaselineSnapshot {
            thread_id,
            model: "gpt-5".to_string(),
            model_provider_id: "test-provider".to_string(),
            service_tier: Some(ServiceTier::Flex),
            approval_policy: AskForApproval::Never,
            approvals_reviewer: ApprovalsReviewer::User,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            cwd: PathBuf::from("/tmp/latest-cwd"),
            reasoning_effort: Some(ReasoningEffort::High),
            personality: Some(Personality::Pragmatic),
            personality_overrides_rollout: false,
            base_instructions: Some("be precise".to_string()),
            developer_instructions: Some("review carefully".to_string()),
            developer_instructions_overrides_rollout: false,
        }
    }

    #[tokio::test]
    async fn thread_config_baseline_round_trips_and_replaces_prior_values() -> anyhow::Result<()> {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
        let thread_id = ThreadId::from_string("2b9bd0a1-c35c-4f39-915c-d3c6458ad782")?;
        let metadata = test_thread_metadata(&codex_home, thread_id, codex_home.clone());
        runtime.upsert_thread(&metadata).await?;

        let initial = test_snapshot(thread_id);
        runtime.upsert_thread_config_baseline(&initial).await?;

        let mut updated = initial.clone();
        updated.service_tier = None;
        updated.reasoning_effort = Some(ReasoningEffort::Low);
        updated.personality = None;
        updated.base_instructions = None;
        updated.developer_instructions = None;
        updated.cwd = PathBuf::from("/tmp/replaced-cwd");
        runtime.upsert_thread_config_baseline(&updated).await?;

        assert_eq!(
            runtime.get_thread_config_baseline(thread_id).await?,
            Some(updated)
        );
        Ok(())
    }

    #[tokio::test]
    async fn apply_rollout_items_persists_latest_config_baseline_from_rollout() -> anyhow::Result<()>
    {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
        let thread_id = ThreadId::from_string("c1d6bbf8-c5e1-459b-a9b9-6b53c2fe3822")?;
        let rollout_path = codex_home.join("sessions").join("rollout-test.jsonl");

        let mut builder = crate::ThreadMetadataBuilder::new(
            thread_id,
            rollout_path.clone(),
            Utc::now(),
            codex_protocol::protocol::SessionSource::Cli,
        );
        builder.model_provider = Some("session-meta-provider".to_string());
        builder.cwd = PathBuf::from("/tmp/session-meta-cwd");
        builder.approval_mode = AskForApproval::OnRequest;
        builder.sandbox_policy = SandboxPolicy::new_read_only_policy();

        let items = vec![
            RolloutItem::SessionMeta(SessionMetaLine {
                meta: SessionMeta {
                    id: thread_id,
                    timestamp: "2026-04-23T00:00:00Z".to_string(),
                    cwd: PathBuf::from("/tmp/session-meta-cwd"),
                    originator: "codex".to_string(),
                    cli_version: "1.0.0".to_string(),
                    source: codex_protocol::protocol::SessionSource::Cli,
                    model_provider: Some("session-meta-provider".to_string()),
                    base_instructions: Some(BaseInstructions {
                        text: "session meta base instructions".to_string(),
                    }),
                    ..Default::default()
                },
                git: None,
            }),
            RolloutItem::EventMsg(EventMsg::SessionConfigured(SessionConfiguredEvent {
                session_id: thread_id,
                forked_from_id: None,
                thread_name: None,
                model: "gpt-5-session-configured".to_string(),
                model_provider_id: "session-configured-provider".to_string(),
                service_tier: Some(ServiceTier::Flex),
                approval_policy: AskForApproval::Never,
                approvals_reviewer: ApprovalsReviewer::User,
                sandbox_policy: SandboxPolicy::DangerFullAccess,
                permission_profile: None,
                cwd: PathBuf::from("/tmp/session-configured-cwd")
                    .try_into()
                    .expect("session configured cwd is absolute"),
                reasoning_effort: Some(ReasoningEffort::High),
                history_log_id: 0,
                history_entry_count: 0,
                initial_messages: None,
                network_proxy: None,
                rollout_path: Some(rollout_path.clone()),
            })),
            RolloutItem::TurnContext(codex_protocol::protocol::TurnContextItem {
                turn_id: Some("turn-1".to_string()),
                trace_id: None,
                cwd: PathBuf::from("/tmp/turn-context-cwd"),
                current_date: None,
                timezone: None,
                approval_policy: AskForApproval::UnlessTrusted,
                approvals_reviewer: Some(ApprovalsReviewer::User),
                sandbox_policy: SandboxPolicy::new_read_only_policy(),
                permission_profile: None,
                network: None,
                file_system_sandbox_policy: None,
                model: "gpt-5-turn-context".to_string(),
                service_tier: Some(ServiceTier::Fast),
                personality: Some(Personality::Pragmatic),
                collaboration_mode: None,
                realtime_active: Some(false),
                effort: Some(ReasoningEffort::Low),
                summary: ReasoningSummaryConfig::Auto,
                user_instructions: None,
                developer_instructions: Some("latest developer instructions".to_string()),
                final_output_json_schema: None,
                truncation_policy: None,
            }),
        ];

        runtime
            .apply_rollout_items(
                &builder,
                &items,
                /*new_thread_memory_mode*/ None,
                Some(Utc::now()),
            )
            .await?;

        assert_eq!(
            runtime.get_thread_config_baseline(thread_id).await?,
            Some(ThreadConfigBaselineSnapshot {
                thread_id,
                model: "gpt-5-turn-context".to_string(),
                model_provider_id: "session-configured-provider".to_string(),
                service_tier: Some(ServiceTier::Fast),
                approval_policy: AskForApproval::UnlessTrusted,
                approvals_reviewer: ApprovalsReviewer::User,
                sandbox_policy: SandboxPolicy::new_read_only_policy(),
                cwd: PathBuf::from("/tmp/turn-context-cwd"),
                reasoning_effort: Some(ReasoningEffort::Low),
                personality: Some(Personality::Pragmatic),
                personality_overrides_rollout: false,
                base_instructions: Some("session meta base instructions".to_string()),
                developer_instructions: Some("latest developer instructions".to_string()),
                developer_instructions_overrides_rollout: false,
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn apply_rollout_items_updates_existing_override_only_baseline_fields_from_live_turn()
    -> anyhow::Result<()> {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
        let thread_id = ThreadId::from_string("d5c9a924-d819-42e9-aa57-4aaf563088f2")?;
        let rollout_path = codex_home
            .join("sessions")
            .join("rollout-preserve-test.jsonl");
        let metadata = test_thread_metadata(&codex_home, thread_id, codex_home.clone());
        runtime.upsert_thread(&metadata).await?;

        let mut builder = crate::ThreadMetadataBuilder::new(
            thread_id,
            rollout_path.clone(),
            Utc::now(),
            codex_protocol::protocol::SessionSource::Cli,
        );
        builder.model_provider = Some("session-meta-provider".to_string());
        builder.cwd = PathBuf::from("/tmp/session-meta-cwd");
        builder.approval_mode = AskForApproval::OnRequest;
        builder.sandbox_policy = SandboxPolicy::new_read_only_policy();
        runtime
            .upsert_thread_config_baseline(&ThreadConfigBaselineSnapshot {
                thread_id,
                model: "persisted-model".to_string(),
                model_provider_id: "persisted-provider".to_string(),
                service_tier: Some(ServiceTier::Flex),
                approval_policy: AskForApproval::OnFailure,
                approvals_reviewer: ApprovalsReviewer::User,
                sandbox_policy: SandboxPolicy::DangerFullAccess,
                cwd: PathBuf::from("/tmp/persisted-cwd"),
                reasoning_effort: Some(ReasoningEffort::Medium),
                personality: Some(Personality::Friendly),
                personality_overrides_rollout: true,
                base_instructions: Some("persisted base instructions".to_string()),
                developer_instructions: Some("persisted developer instructions".to_string()),
                developer_instructions_overrides_rollout: true,
            })
            .await?;

        let items = vec![
            RolloutItem::SessionMeta(SessionMetaLine {
                meta: SessionMeta {
                    id: thread_id,
                    timestamp: "2026-04-24T00:00:00Z".to_string(),
                    cwd: PathBuf::from("/tmp/session-meta-cwd"),
                    originator: "codex".to_string(),
                    cli_version: "1.0.0".to_string(),
                    source: codex_protocol::protocol::SessionSource::Cli,
                    model_provider: Some("session-meta-provider".to_string()),
                    base_instructions: Some(BaseInstructions {
                        text: "rollout base instructions".to_string(),
                    }),
                    ..Default::default()
                },
                git: None,
            }),
            RolloutItem::EventMsg(EventMsg::SessionConfigured(SessionConfiguredEvent {
                session_id: thread_id,
                forked_from_id: None,
                thread_name: None,
                model: "gpt-5-session-configured".to_string(),
                model_provider_id: "session-configured-provider".to_string(),
                service_tier: Some(ServiceTier::Flex),
                approval_policy: AskForApproval::Never,
                approvals_reviewer: ApprovalsReviewer::User,
                sandbox_policy: SandboxPolicy::DangerFullAccess,
                permission_profile: None,
                cwd: PathBuf::from("/tmp/session-configured-cwd")
                    .try_into()
                    .expect("session configured cwd is absolute"),
                reasoning_effort: Some(ReasoningEffort::High),
                history_log_id: 0,
                history_entry_count: 0,
                initial_messages: None,
                network_proxy: None,
                rollout_path: Some(rollout_path.clone()),
            })),
            RolloutItem::TurnContext(codex_protocol::protocol::TurnContextItem {
                turn_id: Some("turn-1".to_string()),
                trace_id: None,
                cwd: PathBuf::from("/tmp/turn-context-cwd"),
                current_date: None,
                timezone: None,
                approval_policy: AskForApproval::UnlessTrusted,
                approvals_reviewer: Some(ApprovalsReviewer::User),
                sandbox_policy: SandboxPolicy::new_read_only_policy(),
                permission_profile: None,
                network: None,
                file_system_sandbox_policy: None,
                model: "gpt-5-turn-context".to_string(),
                service_tier: Some(ServiceTier::Fast),
                personality: Some(Personality::Pragmatic),
                collaboration_mode: None,
                realtime_active: Some(false),
                effort: Some(ReasoningEffort::Low),
                summary: ReasoningSummaryConfig::Auto,
                user_instructions: None,
                developer_instructions: Some("rollout developer instructions".to_string()),
                final_output_json_schema: None,
                truncation_policy: None,
            }),
        ];

        runtime
            .apply_rollout_items(
                &builder,
                &items,
                /*new_thread_memory_mode*/ None,
                Some(Utc::now()),
            )
            .await?;

        assert_eq!(
            runtime.get_thread_config_baseline(thread_id).await?,
            Some(ThreadConfigBaselineSnapshot {
                thread_id,
                model: "gpt-5-turn-context".to_string(),
                model_provider_id: "session-configured-provider".to_string(),
                service_tier: Some(ServiceTier::Fast),
                approval_policy: AskForApproval::UnlessTrusted,
                approvals_reviewer: ApprovalsReviewer::User,
                sandbox_policy: SandboxPolicy::new_read_only_policy(),
                cwd: PathBuf::from("/tmp/turn-context-cwd"),
                reasoning_effort: Some(ReasoningEffort::Low),
                personality: Some(Personality::Pragmatic),
                personality_overrides_rollout: false,
                base_instructions: Some("rollout base instructions".to_string()),
                developer_instructions: Some("rollout developer instructions".to_string()),
                developer_instructions_overrides_rollout: false,
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn historical_backfill_preserves_override_only_fields_when_stale_snapshot_conflicts_with_live_row()
    -> anyhow::Result<()> {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
        let thread_id = ThreadId::from_string("af8e76c7-8e49-4a50-92f4-973ac9e80f8a")?;
        let metadata = test_thread_metadata(&codex_home, thread_id, codex_home.clone());
        runtime.upsert_thread(&metadata).await?;

        let mut stale_historical_snapshot = test_snapshot(thread_id);
        stale_historical_snapshot.model = "historical-model".to_string();
        stale_historical_snapshot.personality = Some(Personality::Pragmatic);
        stale_historical_snapshot.personality_overrides_rollout = false;
        stale_historical_snapshot.base_instructions =
            Some("historical base instructions".to_string());
        stale_historical_snapshot.developer_instructions =
            Some("historical developer instructions".to_string());
        stale_historical_snapshot.developer_instructions_overrides_rollout = false;

        let mut live_snapshot = test_snapshot(thread_id);
        live_snapshot.model = "live-model".to_string();
        live_snapshot.personality = Some(Personality::Friendly);
        live_snapshot.personality_overrides_rollout = true;
        live_snapshot.base_instructions = Some("live base instructions".to_string());
        live_snapshot.developer_instructions = Some("live developer instructions".to_string());
        live_snapshot.developer_instructions_overrides_rollout = true;
        runtime
            .upsert_thread_config_baseline(&live_snapshot)
            .await?;

        runtime
            .upsert_thread_config_baseline_preserving_override_only_fields(
                &stale_historical_snapshot,
            )
            .await?;

        let mut expected = stale_historical_snapshot;
        expected.personality = live_snapshot.personality;
        expected.personality_overrides_rollout = live_snapshot.personality_overrides_rollout;
        expected.base_instructions = live_snapshot.base_instructions;
        expected.developer_instructions = live_snapshot.developer_instructions;
        expected.developer_instructions_overrides_rollout =
            live_snapshot.developer_instructions_overrides_rollout;
        assert_eq!(
            runtime.get_thread_config_baseline(thread_id).await?,
            Some(expected)
        );
        Ok(())
    }

    #[tokio::test]
    async fn historical_backfill_uses_rollout_fields_when_live_row_is_not_marked_override()
    -> anyhow::Result<()> {
        let codex_home = unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
        let thread_id = ThreadId::from_string("f3d74c43-1fa7-48b8-b1c4-ec903627b94b")?;
        let metadata = test_thread_metadata(&codex_home, thread_id, codex_home.clone());
        runtime.upsert_thread(&metadata).await?;

        let mut live_snapshot = test_snapshot(thread_id);
        live_snapshot.model = "live-model".to_string();
        live_snapshot.personality = Some(Personality::Friendly);
        live_snapshot.personality_overrides_rollout = false;
        live_snapshot.base_instructions = Some("live base instructions".to_string());
        live_snapshot.developer_instructions = Some("live developer instructions".to_string());
        live_snapshot.developer_instructions_overrides_rollout = false;
        runtime
            .upsert_thread_config_baseline(&live_snapshot)
            .await?;

        let mut historical_snapshot = test_snapshot(thread_id);
        historical_snapshot.model = "historical-model".to_string();
        historical_snapshot.personality = Some(Personality::Pragmatic);
        historical_snapshot.personality_overrides_rollout = false;
        historical_snapshot.base_instructions = Some("historical base instructions".to_string());
        historical_snapshot.developer_instructions =
            Some("historical developer instructions".to_string());
        historical_snapshot.developer_instructions_overrides_rollout = false;

        runtime
            .upsert_thread_config_baseline_preserving_override_only_fields(&historical_snapshot)
            .await?;

        let mut expected = historical_snapshot;
        expected.base_instructions = live_snapshot.base_instructions;
        assert_eq!(
            runtime.get_thread_config_baseline(thread_id).await?,
            Some(expected)
        );
        Ok(())
    }
}
