#![allow(warnings, clippy::all)]

use super::*;
use crate::config::RolloutConfig;
use chrono::DateTime;
use chrono::Duration;
use chrono::NaiveDateTime;
use chrono::Timelike;
use chrono::Utc;
use codex_protocol::ThreadId;
use codex_protocol::config_types::ApprovalsReviewer;
use codex_protocol::config_types::Personality;
use codex_protocol::config_types::ReasoningSummary as ReasoningSummaryConfig;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_protocol::models::BaseInstructions;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::CompactedItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::GitInfo;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::RolloutLine;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionConfiguredEvent;
use codex_protocol::protocol::SessionMeta;
use codex_protocol::protocol::SessionMetaLine;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::TurnContextItem;
use codex_state::BackfillStatus;
use codex_state::ThreadConfigBaselineSnapshot;
use codex_state::ThreadMetadataBuilder;
use pretty_assertions::assert_eq;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use tempfile::tempdir;
use uuid::Uuid;

fn test_config(codex_home: PathBuf) -> RolloutConfig {
    RolloutConfig {
        sqlite_home: codex_home.clone(),
        cwd: codex_home.clone(),
        codex_home,
        model_provider_id: "test-provider".to_string(),
        generate_memories: true,
    }
}

#[tokio::test]
async fn extract_metadata_from_rollout_uses_session_meta() {
    let dir = tempdir().expect("tempdir");
    let uuid = Uuid::new_v4();
    let id = ThreadId::from_string(&uuid.to_string()).expect("thread id");
    let path = dir
        .path()
        .join(format!("rollout-2026-01-27T12-34-56-{uuid}.jsonl"));

    let session_meta = SessionMeta {
        id,
        forked_from_id: None,
        timestamp: "2026-01-27T12:34:56Z".to_string(),
        cwd: dir.path().to_path_buf(),
        originator: "cli".to_string(),
        cli_version: "0.0.0".to_string(),
        source: SessionSource::default(),
        agent_path: None,
        agent_nickname: None,
        agent_role: None,
        model_provider: Some("openai".to_string()),
        base_instructions: None,
        dynamic_tools: None,
        memory_mode: None,
    };
    let session_meta_line = SessionMetaLine {
        meta: session_meta,
        git: None,
    };
    let rollout_line = RolloutLine {
        timestamp: "2026-01-27T12:34:56Z".to_string(),
        item: RolloutItem::SessionMeta(session_meta_line.clone()),
    };
    let json = serde_json::to_string(&rollout_line).expect("rollout json");
    let mut file = File::create(&path).expect("create rollout");
    writeln!(file, "{json}").expect("write rollout");

    let outcome = extract_metadata_from_rollout(&path, "openai")
        .await
        .expect("extract");

    let builder = builder_from_session_meta(&session_meta_line, path.as_path()).expect("builder");
    let mut expected = builder.build("openai");
    apply_rollout_item(&mut expected, &rollout_line.item, "openai");
    expected.updated_at = file_modified_time_utc(&path).await.expect("mtime");

    assert_eq!(outcome.metadata, expected);
    assert_eq!(outcome.memory_mode, None);
    assert_eq!(outcome.parse_errors, 0);
}

#[tokio::test]
async fn extract_metadata_from_rollout_returns_latest_memory_mode() {
    let dir = tempdir().expect("tempdir");
    let uuid = Uuid::new_v4();
    let id = ThreadId::from_string(&uuid.to_string()).expect("thread id");
    let path = dir
        .path()
        .join(format!("rollout-2026-01-27T12-34-56-{uuid}.jsonl"));

    let session_meta = SessionMeta {
        id,
        forked_from_id: None,
        timestamp: "2026-01-27T12:34:56Z".to_string(),
        cwd: dir.path().to_path_buf(),
        originator: "cli".to_string(),
        cli_version: "0.0.0".to_string(),
        source: SessionSource::default(),
        agent_path: None,
        agent_nickname: None,
        agent_role: None,
        model_provider: Some("openai".to_string()),
        base_instructions: None,
        dynamic_tools: None,
        memory_mode: None,
    };
    let polluted_meta = SessionMeta {
        memory_mode: Some("polluted".to_string()),
        ..session_meta.clone()
    };
    let lines = vec![
        RolloutLine {
            timestamp: "2026-01-27T12:34:56Z".to_string(),
            item: RolloutItem::SessionMeta(SessionMetaLine {
                meta: session_meta,
                git: None,
            }),
        },
        RolloutLine {
            timestamp: "2026-01-27T12:35:00Z".to_string(),
            item: RolloutItem::SessionMeta(SessionMetaLine {
                meta: polluted_meta,
                git: None,
            }),
        },
    ];
    let mut file = File::create(&path).expect("create rollout");
    for line in lines {
        writeln!(
            file,
            "{}",
            serde_json::to_string(&line).expect("serialize rollout line")
        )
        .expect("write rollout line");
    }

    let outcome = extract_metadata_from_rollout(&path, "openai")
        .await
        .expect("extract");

    assert_eq!(outcome.memory_mode.as_deref(), Some("polluted"));
}

#[test]
fn builder_from_items_falls_back_to_filename() {
    let dir = tempdir().expect("tempdir");
    let uuid = Uuid::new_v4();
    let path = dir
        .path()
        .join(format!("rollout-2026-01-27T12-34-56-{uuid}.jsonl"));
    let items = vec![RolloutItem::Compacted(CompactedItem {
        message: "noop".to_string(),
        replacement_history: None,
    })];

    let builder = builder_from_items(items.as_slice(), path.as_path()).expect("builder");
    let naive = NaiveDateTime::parse_from_str("2026-01-27T12-34-56", "%Y-%m-%dT%H-%M-%S")
        .expect("timestamp");
    let created_at = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)
        .with_nanosecond(0)
        .expect("nanosecond");
    let expected = ThreadMetadataBuilder::new(
        ThreadId::from_string(&uuid.to_string()).expect("thread id"),
        path,
        created_at,
        SessionSource::default(),
    );

    assert_eq!(builder, expected);
}

#[test]
fn builder_from_items_falls_back_to_filename_when_only_mismatched_session_meta_exists() {
    let dir = tempdir().expect("tempdir");
    let source_uuid = Uuid::new_v4();
    let child_uuid = Uuid::new_v4();
    let source_id = ThreadId::from_string(&source_uuid.to_string()).expect("source thread id");
    let child_id = ThreadId::from_string(&child_uuid.to_string()).expect("child thread id");
    let path = dir
        .path()
        .join(format!("rollout-2026-01-27T12-34-56-{child_uuid}.jsonl"));
    let items = vec![RolloutItem::SessionMeta(SessionMetaLine {
        meta: SessionMeta {
            id: source_id,
            forked_from_id: None,
            timestamp: "2025-12-01T01:02:03Z".to_string(),
            cwd: dir.path().join("source-cwd"),
            originator: "cli".to_string(),
            cli_version: "0.0.0".to_string(),
            source: SessionSource::default(),
            agent_path: None,
            agent_nickname: None,
            agent_role: None,
            model_provider: Some("openai".to_string()),
            base_instructions: Some(BaseInstructions {
                text: "source base instructions".to_string(),
            }),
            dynamic_tools: None,
            memory_mode: None,
        },
        git: Some(GitInfo {
            commit_hash: Some(codex_git_utils::GitSha::new("source-sha")),
            branch: Some("source-branch".to_string()),
            repository_url: Some("git@example.com:openai/codex.git".to_string()),
        }),
    })];

    let builder = builder_from_items(items.as_slice(), path.as_path()).expect("builder");
    let naive = NaiveDateTime::parse_from_str("2026-01-27T12-34-56", "%Y-%m-%dT%H-%M-%S")
        .expect("timestamp");
    let expected = ThreadMetadataBuilder::new(
        child_id,
        path,
        DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)
            .with_nanosecond(0)
            .expect("nanosecond"),
        SessionSource::default(),
    );

    assert_eq!(builder, expected);
}

#[test]
fn builder_from_items_prefers_canonical_matching_session_meta_over_mismatched_first_session_meta() {
    let dir = tempdir().expect("tempdir");
    let source_uuid = Uuid::new_v4();
    let child_uuid = Uuid::new_v4();
    let source_id = ThreadId::from_string(&source_uuid.to_string()).expect("source thread id");
    let child_id = ThreadId::from_string(&child_uuid.to_string()).expect("child thread id");
    let path = dir
        .path()
        .join(format!("rollout-2026-01-27T12-34-56-{child_uuid}.jsonl"));
    let child_meta_line = SessionMetaLine {
        meta: SessionMeta {
            id: child_id,
            forked_from_id: None,
            timestamp: "2026-01-27T12:34:56Z".to_string(),
            cwd: dir.path().join("child-cwd"),
            originator: "cli".to_string(),
            cli_version: "1.2.3".to_string(),
            source: SessionSource::Cli,
            agent_path: None,
            agent_nickname: None,
            agent_role: None,
            model_provider: Some("child-provider".to_string()),
            base_instructions: Some(BaseInstructions {
                text: "child base instructions".to_string(),
            }),
            dynamic_tools: None,
            memory_mode: None,
        },
        git: Some(GitInfo {
            commit_hash: Some(codex_git_utils::GitSha::new("child-sha")),
            branch: Some("child-branch".to_string()),
            repository_url: Some("git@example.com:openai/child.git".to_string()),
        }),
    };
    let items = vec![
        RolloutItem::SessionMeta(SessionMetaLine {
            meta: SessionMeta {
                id: source_id,
                forked_from_id: None,
                timestamp: "2025-12-01T01:02:03Z".to_string(),
                cwd: dir.path().join("source-cwd"),
                originator: "cli".to_string(),
                cli_version: "0.0.0".to_string(),
                source: SessionSource::default(),
                agent_path: None,
                agent_nickname: None,
                agent_role: None,
                model_provider: Some("source-provider".to_string()),
                base_instructions: Some(BaseInstructions {
                    text: "source base instructions".to_string(),
                }),
                dynamic_tools: None,
                memory_mode: None,
            },
            git: Some(GitInfo {
                commit_hash: Some(codex_git_utils::GitSha::new("source-sha")),
                branch: Some("source-branch".to_string()),
                repository_url: Some("git@example.com:openai/source.git".to_string()),
            }),
        }),
        RolloutItem::SessionMeta(child_meta_line.clone()),
    ];

    let builder = builder_from_items(items.as_slice(), path.as_path()).expect("builder");
    let expected = builder_from_session_meta(&child_meta_line, path.as_path()).expect("builder");

    assert_eq!(builder, expected);
}

#[tokio::test]
async fn backfill_sessions_prefers_filename_thread_id_over_mismatched_session_meta() {
    let dir = tempdir().expect("tempdir");
    let codex_home = dir.path().to_path_buf();
    let source_uuid = Uuid::new_v4();
    let child_uuid = Uuid::new_v4();
    let source_id = ThreadId::from_string(&source_uuid.to_string()).expect("source thread id");
    let child_id = ThreadId::from_string(&child_uuid.to_string()).expect("child thread id");
    let sessions_dir = codex_home.join("sessions");
    std::fs::create_dir_all(sessions_dir.as_path()).expect("create sessions dir");
    let rollout_path = sessions_dir.join(format!("rollout-2026-01-27T12-34-56-{child_uuid}.jsonl"));

    let mut file = File::create(&rollout_path).expect("create rollout");
    let line = RolloutLine {
        timestamp: "2026-01-27T12:34:56Z".to_string(),
        item: RolloutItem::SessionMeta(SessionMetaLine {
            meta: SessionMeta {
                id: source_id,
                forked_from_id: None,
                timestamp: "2025-12-01T01:02:03Z".to_string(),
                cwd: codex_home.join("source-cwd"),
                originator: "cli".to_string(),
                cli_version: "0.0.0".to_string(),
                source: SessionSource::default(),
                agent_path: None,
                agent_nickname: None,
                agent_role: None,
                model_provider: Some("test-provider".to_string()),
                base_instructions: None,
                dynamic_tools: None,
                memory_mode: None,
            },
            git: None,
        }),
    };
    writeln!(
        file,
        "{}",
        serde_json::to_string(&line).expect("serialize rollout line")
    )
    .expect("write rollout line");

    let runtime = codex_state::StateRuntime::init(codex_home.clone(), "test-provider".to_string())
        .await
        .expect("initialize runtime");

    let config = test_config(codex_home.clone());
    backfill_sessions(runtime.as_ref(), &config).await;

    let child_thread = runtime
        .get_thread(child_id)
        .await
        .expect("get child thread")
        .expect("child thread exists");
    let source_thread = runtime
        .get_thread(source_id)
        .await
        .expect("get source thread");
    let naive = NaiveDateTime::parse_from_str("2026-01-27T12-34-56", "%Y-%m-%dT%H-%M-%S")
        .expect("timestamp");
    let created_at = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)
        .with_nanosecond(0)
        .expect("nanosecond");
    assert_eq!(source_thread, None);
    assert_eq!(child_thread.id, child_id);
    assert_eq!(child_thread.created_at, created_at);
    assert_eq!(child_thread.source, "vscode");
    assert_eq!(child_thread.model_provider, "test-provider");
    assert_eq!(child_thread.cli_version, "");
    assert_eq!(child_thread.cwd, PathBuf::new());
    assert_eq!(child_thread.git_sha, None);
    assert_eq!(child_thread.git_branch, None);
    assert_eq!(child_thread.git_origin_url, None);
}

#[tokio::test]
async fn backfill_sessions_ignores_dynamic_tools_from_mismatched_session_meta() {
    let dir = tempdir().expect("tempdir");
    let codex_home = dir.path().to_path_buf();
    let source_uuid = Uuid::new_v4();
    let child_uuid = Uuid::new_v4();
    let source_id = ThreadId::from_string(&source_uuid.to_string()).expect("source thread id");
    let child_id = ThreadId::from_string(&child_uuid.to_string()).expect("child thread id");
    let sessions_dir = codex_home.join("sessions");
    std::fs::create_dir_all(sessions_dir.as_path()).expect("create sessions dir");
    let rollout_path = sessions_dir.join(format!("rollout-2026-01-27T12-34-56-{child_uuid}.jsonl"));
    let dynamic_tools = vec![DynamicToolSpec {
        namespace: None,
        name: "lookup_ticket".to_string(),
        description: "Fetch a ticket".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
        }),
        defer_loading: false,
    }];

    let mut file = File::create(&rollout_path).expect("create rollout");
    let line = RolloutLine {
        timestamp: "2026-01-27T12:34:56Z".to_string(),
        item: RolloutItem::SessionMeta(SessionMetaLine {
            meta: SessionMeta {
                id: source_id,
                forked_from_id: None,
                timestamp: "2025-12-01T01:02:03Z".to_string(),
                cwd: codex_home.join("source-cwd"),
                originator: "cli".to_string(),
                cli_version: "0.0.0".to_string(),
                source: SessionSource::default(),
                agent_path: None,
                agent_nickname: None,
                agent_role: None,
                model_provider: Some("test-provider".to_string()),
                base_instructions: None,
                dynamic_tools: Some(dynamic_tools.clone()),
                memory_mode: None,
            },
            git: None,
        }),
    };
    writeln!(
        file,
        "{}",
        serde_json::to_string(&line).expect("serialize rollout line")
    )
    .expect("write rollout line");

    let runtime = codex_state::StateRuntime::init(codex_home.clone(), "test-provider".to_string())
        .await
        .expect("initialize runtime");

    let config = test_config(codex_home.clone());
    backfill_sessions(runtime.as_ref(), &config).await;

    assert_eq!(
        runtime
            .get_dynamic_tools(child_id)
            .await
            .expect("get child dynamic tools"),
        None
    );
    assert_eq!(
        runtime
            .get_dynamic_tools(source_id)
            .await
            .expect("get source dynamic tools"),
        None
    );
}

#[tokio::test]
async fn backfill_sessions_resumes_from_watermark_and_marks_complete() {
    let dir = tempdir().expect("tempdir");
    let codex_home = dir.path().to_path_buf();
    let first_uuid = Uuid::new_v4();
    let second_uuid = Uuid::new_v4();
    let first_path = write_rollout_in_sessions(
        codex_home.as_path(),
        "2026-01-27T12-34-56",
        "2026-01-27T12:34:56Z",
        first_uuid,
        /*git*/ None,
    );
    let second_path = write_rollout_in_sessions(
        codex_home.as_path(),
        "2026-01-27T12-35-56",
        "2026-01-27T12:35:56Z",
        second_uuid,
        /*git*/ None,
    );

    let runtime = codex_state::StateRuntime::init(codex_home.clone(), "test-provider".to_string())
        .await
        .expect("initialize runtime");
    let first_watermark = backfill_watermark_for_path(codex_home.as_path(), first_path.as_path());
    runtime.mark_backfill_running().await.expect("mark running");
    runtime
        .checkpoint_backfill(first_watermark.as_str())
        .await
        .expect("checkpoint first watermark");
    tokio::time::sleep(std::time::Duration::from_secs(
        (BACKFILL_LEASE_SECONDS + 1) as u64,
    ))
    .await;

    let config = test_config(codex_home.clone());
    backfill_sessions(runtime.as_ref(), &config).await;

    let first_id = ThreadId::from_string(&first_uuid.to_string()).expect("first thread id");
    let second_id = ThreadId::from_string(&second_uuid.to_string()).expect("second thread id");
    assert_eq!(
        runtime
            .get_thread(first_id)
            .await
            .expect("get first thread"),
        None
    );
    assert!(
        runtime
            .get_thread(second_id)
            .await
            .expect("get second thread")
            .is_some()
    );

    let state = runtime
        .get_backfill_state()
        .await
        .expect("get backfill state");
    assert_eq!(state.status, BackfillStatus::Complete);
    assert_eq!(
        state.last_watermark,
        Some(backfill_watermark_for_path(
            codex_home.as_path(),
            second_path.as_path()
        ))
    );
    assert!(state.last_success_at.is_some());
}

#[tokio::test]
async fn backfill_sessions_preserves_existing_git_branch_and_fills_missing_git_fields() {
    let dir = tempdir().expect("tempdir");
    let codex_home = dir.path().to_path_buf();
    let thread_uuid = Uuid::new_v4();
    let rollout_path = write_rollout_in_sessions(
        codex_home.as_path(),
        "2026-01-27T12-34-56",
        "2026-01-27T12:34:56Z",
        thread_uuid,
        Some(GitInfo {
            commit_hash: Some(codex_git_utils::GitSha::new("rollout-sha")),
            branch: Some("rollout-branch".to_string()),
            repository_url: Some("git@example.com:openai/codex.git".to_string()),
        }),
    );

    let runtime = codex_state::StateRuntime::init(codex_home.clone(), "test-provider".to_string())
        .await
        .expect("initialize runtime");
    let thread_id = ThreadId::from_string(&thread_uuid.to_string()).expect("thread id");
    let mut existing = extract_metadata_from_rollout(&rollout_path, "test-provider")
        .await
        .expect("extract")
        .metadata;
    existing.git_sha = None;
    existing.git_branch = Some("sqlite-branch".to_string());
    existing.git_origin_url = None;
    runtime
        .upsert_thread(&existing)
        .await
        .expect("existing metadata upsert");

    let config = test_config(codex_home.clone());
    backfill_sessions(runtime.as_ref(), &config).await;

    let persisted = runtime
        .get_thread(thread_id)
        .await
        .expect("get thread")
        .expect("thread exists");
    assert_eq!(persisted.git_sha.as_deref(), Some("rollout-sha"));
    assert_eq!(persisted.git_branch.as_deref(), Some("sqlite-branch"));
    assert_eq!(
        persisted.git_origin_url.as_deref(),
        Some("git@example.com:openai/codex.git")
    );
}

#[tokio::test]
async fn backfill_sessions_normalizes_cwd_before_upsert() {
    let dir = tempdir().expect("tempdir");
    let codex_home = dir.path().to_path_buf();
    let thread_uuid = Uuid::new_v4();
    let session_cwd = codex_home.join(".");
    let rollout_path = write_rollout_in_sessions_with_cwd(
        codex_home.as_path(),
        "2026-01-27T12-34-56",
        "2026-01-27T12:34:56Z",
        thread_uuid,
        session_cwd.clone(),
        /*git*/ None,
    );

    let runtime = codex_state::StateRuntime::init(codex_home.clone(), "test-provider".to_string())
        .await
        .expect("initialize runtime");

    let config = test_config(codex_home.clone());
    backfill_sessions(runtime.as_ref(), &config).await;

    let thread_id = ThreadId::from_string(&thread_uuid.to_string()).expect("thread id");
    let stored = runtime
        .get_thread(thread_id)
        .await
        .expect("get thread")
        .expect("thread should be backfilled");

    assert_eq!(stored.rollout_path, rollout_path);
    assert_eq!(stored.cwd, normalize_cwd_for_state_db(&session_cwd));
}

#[tokio::test]
async fn backfill_sessions_persists_config_baseline_for_preexisting_thread() {
    let dir = tempdir().expect("tempdir");
    let codex_home = dir.path().to_path_buf();
    let thread_uuid = Uuid::new_v4();
    let thread_id = ThreadId::from_string(&thread_uuid.to_string()).expect("thread id");
    let sessions_dir = codex_home.join("sessions");
    std::fs::create_dir_all(sessions_dir.as_path()).expect("create sessions dir");
    let rollout_path =
        sessions_dir.join(format!("rollout-2026-01-27T12-34-56-{thread_uuid}.jsonl"));

    let lines = vec![
        RolloutLine {
            timestamp: "2026-01-27T12:34:56Z".to_string(),
            item: RolloutItem::SessionMeta(SessionMetaLine {
                meta: SessionMeta {
                    id: thread_id,
                    forked_from_id: None,
                    timestamp: "2026-01-27T12:34:56Z".to_string(),
                    cwd: codex_home.clone(),
                    originator: "cli".to_string(),
                    cli_version: "0.0.0".to_string(),
                    source: SessionSource::default(),
                    agent_path: None,
                    agent_nickname: None,
                    agent_role: None,
                    model_provider: Some("source-provider".to_string()),
                    base_instructions: Some(BaseInstructions {
                        text: "source base instructions".to_string(),
                    }),
                    dynamic_tools: None,
                    memory_mode: None,
                },
                git: None,
            }),
        },
        RolloutLine {
            timestamp: "2026-01-27T12:34:57Z".to_string(),
            item: RolloutItem::EventMsg(EventMsg::SessionConfigured(SessionConfiguredEvent {
                session_id: thread_id,
                forked_from_id: None,
                thread_name: None,
                model: "gpt-5-session-configured".to_string(),
                model_provider_id: "source-provider".to_string(),
                service_tier: Some(ServiceTier::Flex),
                approval_policy: AskForApproval::OnRequest,
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
        },
        RolloutLine {
            timestamp: "2026-01-27T12:34:58Z".to_string(),
            item: RolloutItem::TurnContext(TurnContextItem {
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
                developer_instructions: Some("turn context developer instructions".to_string()),
                final_output_json_schema: None,
                truncation_policy: None,
            }),
        },
    ];

    let mut file = File::create(&rollout_path).expect("create rollout");
    for line in lines {
        writeln!(
            file,
            "{}",
            serde_json::to_string(&line).expect("serialize rollout line")
        )
        .expect("write rollout line");
    }

    let runtime = codex_state::StateRuntime::init(codex_home.clone(), "test-provider".to_string())
        .await
        .expect("initialize runtime");
    let mut existing = extract_metadata_from_rollout(&rollout_path, "test-provider")
        .await
        .expect("extract metadata")
        .metadata;
    existing.updated_at -= Duration::seconds(60);
    runtime
        .upsert_thread(&existing)
        .await
        .expect("upsert preexisting thread");

    let config = test_config(codex_home.clone());
    backfill_sessions(runtime.as_ref(), &config).await;

    assert_eq!(
        runtime
            .get_thread_config_baseline(thread_id)
            .await
            .expect("get thread config baseline"),
        Some(ThreadConfigBaselineSnapshot {
            thread_id,
            model: "gpt-5-turn-context".to_string(),
            model_provider_id: "source-provider".to_string(),
            service_tier: Some(ServiceTier::Fast),
            approval_policy: AskForApproval::UnlessTrusted,
            approvals_reviewer: ApprovalsReviewer::User,
            sandbox_policy: SandboxPolicy::new_read_only_policy(),
            cwd: PathBuf::from("/tmp/turn-context-cwd"),
            reasoning_effort: Some(ReasoningEffort::Low),
            personality: Some(Personality::Pragmatic),
            personality_overrides_rollout: false,
            base_instructions: Some("source base instructions".to_string()),
            developer_instructions: Some("turn context developer instructions".to_string()),
            developer_instructions_overrides_rollout: false,
        })
    );
}

#[tokio::test]
async fn backfill_sessions_preserves_newer_persisted_override_fields_for_preexisting_thread() {
    let dir = tempdir().expect("tempdir");
    let codex_home = dir.path().to_path_buf();
    let thread_uuid = Uuid::new_v4();
    let thread_id = ThreadId::from_string(&thread_uuid.to_string()).expect("thread id");
    let sessions_dir = codex_home.join("sessions");
    std::fs::create_dir_all(sessions_dir.as_path()).expect("create sessions dir");
    let rollout_path =
        sessions_dir.join(format!("rollout-2026-01-27T12-34-56-{thread_uuid}.jsonl"));

    let lines = vec![
        RolloutLine {
            timestamp: "2026-01-27T12:34:56Z".to_string(),
            item: RolloutItem::SessionMeta(SessionMetaLine {
                meta: SessionMeta {
                    id: thread_id,
                    forked_from_id: None,
                    timestamp: "2026-01-27T12:34:56Z".to_string(),
                    cwd: codex_home.clone(),
                    originator: "cli".to_string(),
                    cli_version: "0.0.0".to_string(),
                    source: SessionSource::default(),
                    agent_path: None,
                    agent_nickname: None,
                    agent_role: None,
                    model_provider: Some("source-provider".to_string()),
                    base_instructions: Some(BaseInstructions {
                        text: "source base instructions".to_string(),
                    }),
                    dynamic_tools: None,
                    memory_mode: None,
                },
                git: None,
            }),
        },
        RolloutLine {
            timestamp: "2026-01-27T12:34:57Z".to_string(),
            item: RolloutItem::EventMsg(EventMsg::SessionConfigured(SessionConfiguredEvent {
                session_id: thread_id,
                forked_from_id: None,
                thread_name: None,
                model: "gpt-5-session-configured".to_string(),
                model_provider_id: "source-provider".to_string(),
                service_tier: Some(ServiceTier::Flex),
                approval_policy: AskForApproval::OnRequest,
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
        },
        RolloutLine {
            timestamp: "2026-01-27T12:34:58Z".to_string(),
            item: RolloutItem::TurnContext(TurnContextItem {
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
                developer_instructions: Some("turn context developer instructions".to_string()),
                final_output_json_schema: None,
                truncation_policy: None,
            }),
        },
    ];

    let mut file = File::create(&rollout_path).expect("create rollout");
    for line in lines {
        writeln!(
            file,
            "{}",
            serde_json::to_string(&line).expect("serialize rollout line")
        )
        .expect("write rollout line");
    }

    let runtime = codex_state::StateRuntime::init(codex_home.clone(), "test-provider".to_string())
        .await
        .expect("initialize runtime");
    let existing = extract_metadata_from_rollout(&rollout_path, "test-provider")
        .await
        .expect("extract metadata")
        .metadata;
    runtime
        .upsert_thread(&existing)
        .await
        .expect("upsert preexisting thread");
    runtime
        .upsert_thread_config_baseline(&ThreadConfigBaselineSnapshot {
            thread_id,
            model: "gpt-5-persisted".to_string(),
            model_provider_id: "source-provider".to_string(),
            service_tier: Some(ServiceTier::Flex),
            approval_policy: AskForApproval::OnRequest,
            approvals_reviewer: ApprovalsReviewer::User,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            cwd: PathBuf::from("/tmp/persisted-cwd"),
            reasoning_effort: Some(ReasoningEffort::High),
            personality: Some(Personality::Friendly),
            personality_overrides_rollout: true,
            base_instructions: Some("persisted base instructions".to_string()),
            developer_instructions: Some("persisted developer instructions".to_string()),
            developer_instructions_overrides_rollout: true,
        })
        .await
        .expect("persist override baseline");

    let config = test_config(codex_home.clone());
    backfill_sessions(runtime.as_ref(), &config).await;

    assert_eq!(
        runtime
            .get_thread_config_baseline(thread_id)
            .await
            .expect("get thread config baseline"),
        Some(ThreadConfigBaselineSnapshot {
            thread_id,
            model: "gpt-5-turn-context".to_string(),
            model_provider_id: "source-provider".to_string(),
            service_tier: Some(ServiceTier::Fast),
            approval_policy: AskForApproval::UnlessTrusted,
            approvals_reviewer: ApprovalsReviewer::User,
            sandbox_policy: SandboxPolicy::new_read_only_policy(),
            cwd: PathBuf::from("/tmp/turn-context-cwd"),
            reasoning_effort: Some(ReasoningEffort::Low),
            personality: Some(Personality::Friendly),
            personality_overrides_rollout: true,
            base_instructions: Some("persisted base instructions".to_string()),
            developer_instructions: Some("persisted developer instructions".to_string()),
            developer_instructions_overrides_rollout: true,
        })
    );
}

fn write_rollout_in_sessions(
    codex_home: &Path,
    filename_ts: &str,
    event_ts: &str,
    thread_uuid: Uuid,
    git: Option<GitInfo>,
) -> PathBuf {
    write_rollout_in_sessions_with_cwd(
        codex_home,
        filename_ts,
        event_ts,
        thread_uuid,
        codex_home.to_path_buf(),
        git,
    )
}

fn write_rollout_in_sessions_with_cwd(
    codex_home: &Path,
    filename_ts: &str,
    event_ts: &str,
    thread_uuid: Uuid,
    cwd: PathBuf,
    git: Option<GitInfo>,
) -> PathBuf {
    let id = ThreadId::from_string(&thread_uuid.to_string()).expect("thread id");
    let sessions_dir = codex_home.join("sessions");
    std::fs::create_dir_all(sessions_dir.as_path()).expect("create sessions dir");
    let path = sessions_dir.join(format!("rollout-{filename_ts}-{thread_uuid}.jsonl"));
    let session_meta = SessionMeta {
        id,
        forked_from_id: None,
        timestamp: event_ts.to_string(),
        cwd,
        originator: "cli".to_string(),
        cli_version: "0.0.0".to_string(),
        source: SessionSource::default(),
        agent_path: None,
        agent_nickname: None,
        agent_role: None,
        model_provider: Some("test-provider".to_string()),
        base_instructions: None,
        dynamic_tools: None,
        memory_mode: None,
    };
    let session_meta_line = SessionMetaLine {
        meta: session_meta,
        git,
    };
    let rollout_line = RolloutLine {
        timestamp: event_ts.to_string(),
        item: RolloutItem::SessionMeta(session_meta_line),
    };
    let json = serde_json::to_string(&rollout_line).expect("serialize rollout");
    let mut file = File::create(&path).expect("create rollout");
    writeln!(file, "{json}").expect("write rollout");
    path
}
