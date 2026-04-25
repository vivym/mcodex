use anyhow::Result;
use app_test_support::ChatGptAuthFixture;
use app_test_support::McpProcess;
use app_test_support::create_fake_rollout;
use app_test_support::create_mock_responses_server_repeating_assistant;
use app_test_support::to_response;
use app_test_support::write_chatgpt_auth;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::SandboxPolicy;
use codex_app_server_protocol::SessionSource;
use codex_app_server_protocol::ThreadForkParams;
use codex_app_server_protocol::ThreadForkResponse;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadListParams;
use codex_app_server_protocol::ThreadListResponse;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::ThreadStartedNotification;
use codex_app_server_protocol::ThreadStatus;
use codex_app_server_protocol::ThreadStatusChangedNotification;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::TurnStatus;
use codex_app_server_protocol::UserInput;
use codex_config::types::AuthCredentialsStoreMode;
use codex_login::REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR;
use codex_protocol::ThreadId;
use codex_protocol::config_types::Personality;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::models::BaseInstructions;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::AskForApproval as RolloutAskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::RolloutLine;
use codex_protocol::protocol::SandboxPolicy as RolloutSandboxPolicy;
use codex_protocol::protocol::SessionMeta;
use codex_protocol::protocol::SessionMetaLine;
use codex_protocol::protocol::SessionSource as RolloutSessionSource;
use codex_protocol::protocol::TurnContextItem;
use codex_state::StateRuntime;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::path::Path;
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

use super::analytics::assert_basic_thread_initialized_event;
use super::analytics::enable_analytics_capture;
use super::analytics::thread_initialized_event;
use super::analytics::wait_for_analytics_payload;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

async fn wait_for_responses_request_count(
    server: &MockServer,
    expected_count: usize,
) -> Result<()> {
    timeout(DEFAULT_READ_TIMEOUT, async {
        loop {
            let Some(requests) = server.received_requests().await else {
                anyhow::bail!("wiremock did not record requests");
            };
            let responses_request_count = requests
                .iter()
                .filter(|request| {
                    request.method == "POST" && request.url.path().ends_with("/responses")
                })
                .count();
            if responses_request_count == expected_count {
                return Ok::<(), anyhow::Error>(());
            }
            if responses_request_count > expected_count {
                anyhow::bail!(
                    "expected exactly {expected_count} /responses requests, got {responses_request_count}"
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn thread_fork_creates_new_thread_and_emits_started() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let preview = "Saved user message";
    let conversation_id = create_fake_rollout(
        codex_home.path(),
        "2025-01-05T12-00-00",
        "2025-01-05T12:00:00Z",
        preview,
        Some("mock_provider"),
        /*git_info*/ None,
    )?;

    let original_path = codex_home
        .path()
        .join("sessions")
        .join("2025")
        .join("01")
        .join("05")
        .join(format!(
            "rollout-2025-01-05T12-00-00-{conversation_id}.jsonl"
        ));
    assert!(
        original_path.exists(),
        "expected original rollout to exist at {}",
        original_path.display()
    );
    let original_contents = std::fs::read_to_string(&original_path)?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let fork_id = mcp
        .send_thread_fork_request(ThreadForkParams {
            thread_id: conversation_id.clone(),
            ..Default::default()
        })
        .await?;
    let fork_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(fork_id)),
    )
    .await??;
    let fork_result = fork_resp.result.clone();
    let ThreadForkResponse { thread, .. } = to_response::<ThreadForkResponse>(fork_resp)?;

    // Wire contract: thread title field is `name`, serialized as null when unset.
    let thread_json = fork_result
        .get("thread")
        .and_then(Value::as_object)
        .expect("thread/fork result.thread must be an object");
    assert_eq!(
        thread_json.get("name"),
        Some(&Value::Null),
        "forked threads do not inherit a name; expected `name: null`"
    );

    let after_contents = std::fs::read_to_string(&original_path)?;
    assert_eq!(
        after_contents, original_contents,
        "fork should not mutate the original rollout file"
    );

    assert_ne!(thread.id, conversation_id);
    assert_eq!(thread.forked_from_id, Some(conversation_id.clone()));
    assert_eq!(thread.preview, preview);
    assert_eq!(thread.model_provider, "mock_provider");
    assert_eq!(thread.status, ThreadStatus::Idle);
    let thread_path = thread.path.clone().expect("thread path");
    assert!(thread_path.is_absolute());
    assert_ne!(thread_path, original_path);
    assert!(thread.cwd.is_absolute());
    assert_eq!(thread.source, SessionSource::VsCode);
    assert_eq!(thread.name, None);

    assert_eq!(
        thread.turns.len(),
        1,
        "expected forked thread to include one turn"
    );
    let turn = &thread.turns[0];
    assert_eq!(turn.status, TurnStatus::Interrupted);
    assert_eq!(turn.items.len(), 1, "expected user message item");
    match &turn.items[0] {
        ThreadItem::UserMessage { content, .. } => {
            assert_eq!(
                content,
                &vec![UserInput::Text {
                    text: preview.to_string(),
                    text_elements: Vec::new(),
                }]
            );
        }
        other => panic!("expected user message item, got {other:?}"),
    }

    // A corresponding thread/started notification should arrive.
    let deadline = tokio::time::Instant::now() + DEFAULT_READ_TIMEOUT;
    let notif = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let message = timeout(remaining, mcp.read_next_message()).await??;
        let JSONRPCMessage::Notification(notif) = message else {
            continue;
        };
        if notif.method == "thread/status/changed" {
            let status_changed: ThreadStatusChangedNotification =
                serde_json::from_value(notif.params.expect("params must be present"))?;
            if status_changed.thread_id == thread.id {
                anyhow::bail!(
                    "thread/fork should introduce the thread without a preceding thread/status/changed"
                );
            }
            continue;
        }
        if notif.method == "thread/started" {
            break notif;
        }
    };
    let started_params = notif.params.clone().expect("params must be present");
    let started_thread_json = started_params
        .get("thread")
        .and_then(Value::as_object)
        .expect("thread/started params.thread must be an object");
    assert_eq!(
        started_thread_json.get("name"),
        Some(&Value::Null),
        "thread/started must serialize `name: null` when unset"
    );
    let started: ThreadStartedNotification =
        serde_json::from_value(notif.params.expect("params must be present"))?;
    assert_eq!(started.thread, thread);

    Ok(())
}

#[tokio::test]
async fn thread_fork_by_thread_id_preserves_source_config_baseline_after_config_changes()
-> Result<()> {
    assert_thread_fork_preserves_source_config_baseline(ForkSource::ThreadId).await
}

#[tokio::test]
async fn thread_fork_by_path_preserves_source_config_baseline_after_config_changes() -> Result<()> {
    assert_thread_fork_preserves_source_config_baseline(ForkSource::Path).await
}

#[tokio::test]
async fn thread_fork_by_thread_id_preserves_loaded_zero_turn_start_config_baseline_after_start_override()
-> Result<()> {
    let source_server = responses::start_mock_server().await;
    let source_mock = responses::mount_sse_once(
        &source_server,
        source_config_sse("source-fork-resp", "source-fork-msg"),
    )
    .await;
    let current_server = responses::start_mock_server().await;
    mount_current_provider_responder(&current_server).await;
    let codex_home = TempDir::new()?;
    write_dual_provider_config_toml(
        codex_home.path(),
        DualProviderConfig {
            default_provider: "source_provider",
            model: "gpt-5.2-codex",
            model_reasoning_effort: "high",
            service_tier: "flex",
            approval_policy: "on-request",
            sandbox_mode: "danger-full-access",
            instructions: "source base instructions",
            developer_instructions: "source developer instructions",
            source_server_uri: &source_server.uri(),
            current_server_uri: &current_server.uri(),
        },
    )?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
    let start_id = mcp
        .send_thread_start_request(ThreadStartParams {
            base_instructions: Some("zero-turn baseline base instructions".to_string()),
            developer_instructions: Some("zero-turn baseline developer instructions".to_string()),
            personality: Some(Personality::Friendly),
            persist_extended_history: false,
            ..Default::default()
        })
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(start_resp)?;

    let fork_id = mcp
        .send_thread_fork_request(ThreadForkParams {
            thread_id: thread.id.clone(),
            ..Default::default()
        })
        .await?;
    let fork_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(fork_id)),
    )
    .await??;
    let ThreadForkResponse {
        thread: forked,
        model,
        model_provider,
        service_tier,
        approval_policy,
        sandbox,
        reasoning_effort,
        ..
    } = to_response::<ThreadForkResponse>(fork_resp)?;
    assert_eq!(forked.forked_from_id, Some(thread.id));
    assert_eq!(model, "gpt-5.2-codex");
    assert_eq!(model_provider, "source_provider");
    assert_eq!(service_tier, Some(ServiceTier::Flex));
    assert_eq!(
        approval_policy,
        codex_app_server_protocol::AskForApproval::OnRequest
    );
    assert_eq!(sandbox, SandboxPolicy::DangerFullAccess);
    assert_eq!(reasoning_effort, Some(ReasoningEffort::High));

    let turn_id = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: forked.id,
            input: vec![UserInput::Text {
                text: "continue zero-turn fork with start override".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_id)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let source_requests = source_mock.requests();
    assert_eq!(source_requests.len(), 1);
    assert_no_responses_requests(&current_server).await?;
    assert_zero_turn_start_override_request(source_requests.last().expect("forked turn request"));

    Ok(())
}

#[tokio::test]
async fn thread_fork_by_thread_id_preserves_unloaded_zero_turn_start_config_baseline_after_config_changes()
-> Result<()> {
    assert_thread_fork_preserves_unloaded_zero_turn_start_config_baseline_after_config_changes(
        ForkSource::ThreadId,
        BackfillStateBeforeFork::Complete,
    )
    .await
}

#[tokio::test]
async fn thread_fork_by_path_preserves_unloaded_zero_turn_start_config_baseline_after_config_changes()
-> Result<()> {
    assert_thread_fork_preserves_unloaded_zero_turn_start_config_baseline_after_config_changes(
        ForkSource::Path,
        BackfillStateBeforeFork::Complete,
    )
    .await
}

#[tokio::test]
async fn thread_fork_by_path_preserves_unloaded_zero_turn_start_config_baseline_while_backfill_running()
-> Result<()> {
    assert_thread_fork_preserves_unloaded_zero_turn_start_config_baseline_after_config_changes(
        ForkSource::Path,
        BackfillStateBeforeFork::Running,
    )
    .await
}

enum BackfillStateBeforeFork {
    Complete,
    Running,
}

async fn assert_thread_fork_preserves_unloaded_zero_turn_start_config_baseline_after_config_changes(
    source: ForkSource,
    backfill_state_before_fork: BackfillStateBeforeFork,
) -> Result<()> {
    let source_server = responses::start_mock_server().await;
    let source_mock = responses::mount_sse_once(
        &source_server,
        source_config_sse("source-fork-resp", "source-fork-msg"),
    )
    .await;
    let current_server = responses::start_mock_server().await;
    mount_current_provider_responder(&current_server).await;
    let codex_home = TempDir::new()?;
    write_dual_provider_config_toml(
        codex_home.path(),
        DualProviderConfig {
            default_provider: "source_provider",
            model: "gpt-5.2-codex",
            model_reasoning_effort: "high",
            service_tier: "flex",
            approval_policy: "on-request",
            sandbox_mode: "danger-full-access",
            instructions: "source base instructions",
            developer_instructions: "source developer instructions",
            source_server_uri: &source_server.uri(),
            current_server_uri: &current_server.uri(),
        },
    )?;
    let runtime = StateRuntime::init(
        codex_home.path().to_path_buf(),
        "source_provider".to_string(),
    )
    .await?;
    if matches!(backfill_state_before_fork, BackfillStateBeforeFork::Running) {
        runtime
            .mark_backfill_complete(/*last_watermark*/ None)
            .await?;
    }

    let mut primary = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, primary.initialize()).await??;
    let start_id = primary
        .send_thread_start_request(ThreadStartParams {
            base_instructions: Some("zero-turn baseline base instructions".to_string()),
            developer_instructions: Some("zero-turn baseline developer instructions".to_string()),
            personality: Some(Personality::Friendly),
            persist_extended_history: false,
            ..Default::default()
        })
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(start_resp)?;
    let source_thread_id = thread.id;
    let Some(source_thread_path) = thread.path else {
        anyhow::bail!("source thread path missing");
    };
    drop(primary);
    if matches!(backfill_state_before_fork, BackfillStateBeforeFork::Running) {
        runtime.mark_backfill_running().await?;
    }

    write_dual_provider_config_toml(
        codex_home.path(),
        DualProviderConfig {
            default_provider: "current_provider",
            model: "gpt-5.1-codex-max",
            model_reasoning_effort: "low",
            service_tier: "fast",
            approval_policy: "never",
            sandbox_mode: "read-only",
            instructions: "current base instructions",
            developer_instructions: "current developer instructions",
            source_server_uri: &source_server.uri(),
            current_server_uri: &current_server.uri(),
        },
    )?;

    let mut secondary = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, secondary.initialize()).await??;
    let fork_params = match source {
        ForkSource::ThreadId => ThreadForkParams {
            thread_id: source_thread_id.clone(),
            ..Default::default()
        },
        ForkSource::Path => ThreadForkParams {
            thread_id: "ignored-when-path-is-present".to_string(),
            path: Some(source_thread_path),
            ..Default::default()
        },
    };
    let fork_id = secondary.send_thread_fork_request(fork_params).await?;
    let fork_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_response_message(RequestId::Integer(fork_id)),
    )
    .await??;
    let ThreadForkResponse {
        thread: forked,
        model,
        model_provider,
        service_tier,
        approval_policy,
        sandbox,
        reasoning_effort,
        ..
    } = to_response::<ThreadForkResponse>(fork_resp)?;
    assert_eq!(forked.forked_from_id, Some(source_thread_id));
    assert_eq!(model, "gpt-5.2-codex");
    assert_eq!(model_provider, "source_provider");
    assert_eq!(service_tier, Some(ServiceTier::Flex));
    assert_eq!(
        approval_policy,
        codex_app_server_protocol::AskForApproval::OnRequest
    );
    assert_eq!(sandbox, SandboxPolicy::DangerFullAccess);
    assert_eq!(reasoning_effort, Some(ReasoningEffort::High));

    let turn_id = secondary
        .send_turn_start_request(TurnStartParams {
            thread_id: forked.id,
            input: vec![UserInput::Text {
                text: "continue zero-turn fork with start override".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_response_message(RequestId::Integer(turn_id)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let source_requests = source_mock.requests();
    assert_eq!(source_requests.len(), 1);
    assert_no_responses_requests(&current_server).await?;
    let [source_request] = source_requests.as_slice() else {
        anyhow::bail!("expected one forked turn request");
    };
    assert_zero_turn_start_override_request(source_request);

    Ok(())
}

#[tokio::test]
async fn thread_fork_by_path_ignores_mismatched_source_first_session_meta() -> Result<()> {
    let source_server = responses::start_mock_server().await;
    let current_server = responses::start_mock_server().await;
    let source_mock = responses::mount_sse_sequence(
        &source_server,
        vec![
            source_config_sse("source-seed-resp", "source-seed-msg"),
            source_config_sse("source-fork-resp", "source-fork-msg"),
        ],
    )
    .await;
    mount_current_provider_responder(&current_server).await;
    let codex_home = TempDir::new()?;
    write_dual_provider_config_toml(
        codex_home.path(),
        DualProviderConfig {
            default_provider: "source_provider",
            model: "gpt-5.2-codex",
            model_reasoning_effort: "high",
            service_tier: "flex",
            approval_policy: "on-request",
            sandbox_mode: "danger-full-access",
            instructions: "source base instructions",
            developer_instructions: "source developer instructions",
            source_server_uri: &source_server.uri(),
            current_server_uri: &current_server.uri(),
        },
    )?;

    let mut primary = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, primary.initialize()).await??;
    let start_id = primary
        .send_thread_start_request(ThreadStartParams {
            persist_extended_history: false,
            ..Default::default()
        })
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(start_resp)?;
    let materialize_id = primary
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "seed source config".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_response_message(RequestId::Integer(materialize_id)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_notification_message("turn/completed"),
    )
    .await??;
    let source_thread_id = thread.id.clone();
    let source_thread_path = thread
        .path
        .clone()
        .ok_or_else(|| anyhow::anyhow!("source thread path"))?;
    prepend_mismatched_fork_session_meta(source_thread_path.as_path())?;
    drop(primary);

    write_dual_provider_config_toml(
        codex_home.path(),
        DualProviderConfig {
            default_provider: "current_provider",
            model: "gpt-5.1-codex-max",
            model_reasoning_effort: "low",
            service_tier: "fast",
            approval_policy: "never",
            sandbox_mode: "read-only",
            instructions: "current base instructions",
            developer_instructions: "current developer instructions",
            source_server_uri: &source_server.uri(),
            current_server_uri: &current_server.uri(),
        },
    )?;

    let mut secondary = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, secondary.initialize()).await??;
    let fork_id = secondary
        .send_thread_fork_request(ThreadForkParams {
            thread_id: "ignored-when-path-is-present".to_string(),
            path: Some(source_thread_path),
            ..Default::default()
        })
        .await?;
    let fork_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_response_message(RequestId::Integer(fork_id)),
    )
    .await??;
    let ThreadForkResponse {
        thread: forked,
        model,
        model_provider,
        service_tier,
        approval_policy,
        sandbox,
        reasoning_effort,
        ..
    } = to_response::<ThreadForkResponse>(fork_resp)?;
    assert_eq!(forked.forked_from_id, Some(source_thread_id));
    assert_eq!(model, "gpt-5.2-codex");
    assert_eq!(model_provider, "source_provider");
    assert_eq!(service_tier, Some(ServiceTier::Flex));
    assert_eq!(
        approval_policy,
        codex_app_server_protocol::AskForApproval::OnRequest
    );
    assert_eq!(sandbox, SandboxPolicy::DangerFullAccess);
    assert_eq!(reasoning_effort, Some(ReasoningEffort::High));

    let turn_id = secondary
        .send_turn_start_request(TurnStartParams {
            thread_id: forked.id,
            input: vec![UserInput::Text {
                text: "continue fork with canonical child config".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_response_message(RequestId::Integer(turn_id)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let source_requests = source_mock.requests();
    assert_eq!(source_requests.len(), 2);
    wait_for_responses_request_count(&current_server, /*expected_count*/ 0).await?;
    let forked_turn_request = source_requests.last().expect("forked turn request");
    assert_source_config_request(forked_turn_request);
    let instructions_text = forked_turn_request.instructions_text();
    assert!(
        !instructions_text.contains("polluted base instructions"),
        "forked turn should ignore prepended mismatched session metadata: {instructions_text:?}"
    );

    Ok(())
}

#[tokio::test]
async fn thread_fork_tracks_thread_initialized_analytics() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;

    let codex_home = TempDir::new()?;
    create_config_toml_with_chatgpt_base_url(
        codex_home.path(),
        &server.uri(),
        &server.uri(),
        /*general_analytics_enabled*/ true,
    )?;
    enable_analytics_capture(&server, codex_home.path()).await?;

    let conversation_id = create_fake_rollout(
        codex_home.path(),
        "2025-01-05T12-00-00",
        "2025-01-05T12:00:00Z",
        "Saved user message",
        Some("mock_provider"),
        /*git_info*/ None,
    )?;

    let mut mcp = McpProcess::new_without_managed_config(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let fork_id = mcp
        .send_thread_fork_request(ThreadForkParams {
            thread_id: conversation_id,
            ..Default::default()
        })
        .await?;
    let fork_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(fork_id)),
    )
    .await??;
    let ThreadForkResponse { thread, .. } = to_response::<ThreadForkResponse>(fork_resp)?;

    let payload = wait_for_analytics_payload(&server, DEFAULT_READ_TIMEOUT).await?;
    let event = thread_initialized_event(&payload)?;
    assert_basic_thread_initialized_event(event, &thread.id, "mock-model", "forked");
    Ok(())
}

#[tokio::test]
async fn thread_fork_accepts_materialized_zero_turn_thread() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let start_id = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(start_resp)?;
    let source_thread_id = thread.id;

    let fork_id = mcp
        .send_thread_fork_request(ThreadForkParams {
            thread_id: source_thread_id.clone(),
            ..Default::default()
        })
        .await?;
    let fork_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(fork_id)),
    )
    .await??;
    let ThreadForkResponse { thread: forked, .. } = to_response::<ThreadForkResponse>(fork_resp)?;
    assert_eq!(forked.forked_from_id, Some(source_thread_id));
    assert_eq!(forked.status, ThreadStatus::Idle);

    Ok(())
}

#[tokio::test]
async fn thread_fork_surfaces_cloud_requirements_load_errors() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/backend-api/wham/config/requirements"))
        .respond_with(
            ResponseTemplate::new(401)
                .insert_header("content-type", "text/html")
                .set_body_string("<html>nope</html>"),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": { "code": "refresh_token_invalidated" }
        })))
        .mount(&server)
        .await;

    let codex_home = TempDir::new()?;
    let model_server = create_mock_responses_server_repeating_assistant("Done").await;
    let chatgpt_base_url = format!("{}/backend-api", server.uri());
    create_config_toml_with_chatgpt_base_url(
        codex_home.path(),
        &model_server.uri(),
        &chatgpt_base_url,
        /*general_analytics_enabled*/ false,
    )?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("chatgpt-token")
            .refresh_token("stale-refresh-token")
            .plan_type("business")
            .chatgpt_user_id("user-123")
            .chatgpt_account_id("account-123")
            .account_id("account-123"),
        AuthCredentialsStoreMode::File,
    )?;

    let conversation_id = create_fake_rollout(
        codex_home.path(),
        "2025-01-05T12-00-00",
        "2025-01-05T12:00:00Z",
        "Saved user message",
        Some("mock_provider"),
        /*git_info*/ None,
    )?;

    let refresh_token_url = format!("{}/oauth/token", server.uri());
    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[
            ("OPENAI_API_KEY", None),
            (
                REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR,
                Some(refresh_token_url.as_str()),
            ),
        ],
    )
    .await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let fork_id = mcp
        .send_thread_fork_request(ThreadForkParams {
            thread_id: conversation_id,
            ..Default::default()
        })
        .await?;
    let fork_err: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(fork_id)),
    )
    .await??;

    assert!(
        fork_err
            .error
            .message
            .contains("failed to load configuration"),
        "unexpected fork error: {}",
        fork_err.error.message
    );
    assert_eq!(
        fork_err.error.data,
        Some(json!({
            "reason": "cloudRequirements",
            "errorCode": "Auth",
            "action": "relogin",
            "statusCode": 401,
            "detail": "Your access token could not be refreshed because your refresh token was revoked. Please log out and sign in again.",
        }))
    );

    Ok(())
}

#[tokio::test]
async fn thread_fork_ephemeral_remains_pathless_and_omits_listing() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let preview = "Saved user message";
    let conversation_id = create_fake_rollout(
        codex_home.path(),
        "2025-01-05T12-00-00",
        "2025-01-05T12:00:00Z",
        preview,
        Some("mock_provider"),
        /*git_info*/ None,
    )?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let fork_id = mcp
        .send_thread_fork_request(ThreadForkParams {
            thread_id: conversation_id.clone(),
            ephemeral: true,
            ..Default::default()
        })
        .await?;
    let fork_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(fork_id)),
    )
    .await??;
    let fork_result = fork_resp.result.clone();
    let ThreadForkResponse { thread, .. } = to_response::<ThreadForkResponse>(fork_resp)?;
    let fork_thread_id = thread.id.clone();

    assert!(
        thread.ephemeral,
        "ephemeral forks should be marked explicitly"
    );
    assert_eq!(
        thread.path, None,
        "ephemeral forks should not expose a path"
    );
    assert_eq!(thread.preview, preview);
    assert_eq!(thread.status, ThreadStatus::Idle);
    assert_eq!(thread.name, None);
    assert_eq!(thread.turns.len(), 1, "expected copied fork history");

    let turn = &thread.turns[0];
    assert_eq!(turn.status, TurnStatus::Completed);
    assert_eq!(turn.items.len(), 1, "expected user message item");
    match &turn.items[0] {
        ThreadItem::UserMessage { content, .. } => {
            assert_eq!(
                content,
                &vec![UserInput::Text {
                    text: preview.to_string(),
                    text_elements: Vec::new(),
                }]
            );
        }
        other => panic!("expected user message item, got {other:?}"),
    }

    let thread_json = fork_result
        .get("thread")
        .and_then(Value::as_object)
        .expect("thread/fork result.thread must be an object");
    assert_eq!(
        thread_json.get("ephemeral").and_then(Value::as_bool),
        Some(true),
        "ephemeral forks should serialize `ephemeral: true`"
    );

    let deadline = tokio::time::Instant::now() + DEFAULT_READ_TIMEOUT;
    let notif = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let message = timeout(remaining, mcp.read_next_message()).await??;
        let JSONRPCMessage::Notification(notif) = message else {
            continue;
        };
        if notif.method == "thread/status/changed" {
            let status_changed: ThreadStatusChangedNotification =
                serde_json::from_value(notif.params.expect("params must be present"))?;
            if status_changed.thread_id == fork_thread_id {
                anyhow::bail!(
                    "thread/fork should introduce the thread without a preceding thread/status/changed"
                );
            }
            continue;
        }
        if notif.method == "thread/started" {
            break notif;
        }
    };
    let started_params = notif.params.clone().expect("params must be present");
    let started_thread_json = started_params
        .get("thread")
        .and_then(Value::as_object)
        .expect("thread/started params.thread must be an object");
    assert_eq!(
        started_thread_json
            .get("ephemeral")
            .and_then(Value::as_bool),
        Some(true),
        "thread/started should serialize `ephemeral: true` for ephemeral forks"
    );
    let started: ThreadStartedNotification =
        serde_json::from_value(notif.params.expect("params must be present"))?;
    assert_eq!(started.thread, thread);

    let list_id = mcp
        .send_thread_list_request(ThreadListParams {
            cursor: None,
            limit: Some(10),
            sort_key: None,
            model_providers: None,
            source_kinds: None,
            archived: None,
            cwd: None,
            search_term: None,
        })
        .await?;
    let list_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(list_id)),
    )
    .await??;
    let ThreadListResponse { data, .. } = to_response::<ThreadListResponse>(list_resp)?;
    assert!(
        data.iter().all(|candidate| candidate.id != fork_thread_id),
        "ephemeral forks should not appear in thread/list"
    );
    assert!(
        data.iter().any(|candidate| candidate.id == conversation_id),
        "persistent source thread should remain listed"
    );

    let turn_id = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: fork_thread_id,
            input: vec![UserInput::Text {
                text: "continue".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    let turn_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_id)),
    )
    .await??;
    let _: TurnStartResponse = to_response::<TurnStartResponse>(turn_resp)?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    Ok(())
}

enum ForkSource {
    ThreadId,
    Path,
}

async fn assert_thread_fork_preserves_source_config_baseline(source: ForkSource) -> Result<()> {
    let source_server = responses::start_mock_server().await;
    let current_server = responses::start_mock_server().await;
    let source_mock = responses::mount_sse_sequence(
        &source_server,
        vec![
            source_config_sse("source-seed-resp", "source-seed-msg"),
            source_config_sse("source-fork-resp", "source-fork-msg"),
        ],
    )
    .await;
    mount_current_provider_responder(&current_server).await;
    let codex_home = TempDir::new()?;
    write_dual_provider_config_toml(
        codex_home.path(),
        DualProviderConfig {
            default_provider: "source_provider",
            model: "gpt-5.2-codex",
            model_reasoning_effort: "high",
            service_tier: "flex",
            approval_policy: "on-request",
            sandbox_mode: "danger-full-access",
            instructions: "source base instructions",
            developer_instructions: "source developer instructions",
            source_server_uri: &source_server.uri(),
            current_server_uri: &current_server.uri(),
        },
    )?;

    let mut primary = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, primary.initialize()).await??;
    let start_id = primary
        .send_thread_start_request(ThreadStartParams {
            persist_extended_history: false,
            ..Default::default()
        })
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(start_resp)?;
    let materialize_id = primary
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "seed source config".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_response_message(RequestId::Integer(materialize_id)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_notification_message("turn/completed"),
    )
    .await??;
    let source_thread_id = thread.id.clone();
    let source_thread_path = thread
        .path
        .clone()
        .ok_or_else(|| anyhow::anyhow!("source thread path"))?;
    drop(primary);

    write_dual_provider_config_toml(
        codex_home.path(),
        DualProviderConfig {
            default_provider: "current_provider",
            model: "gpt-5.1-codex-max",
            model_reasoning_effort: "low",
            service_tier: "fast",
            approval_policy: "never",
            sandbox_mode: "read-only",
            instructions: "current base instructions",
            developer_instructions: "current developer instructions",
            source_server_uri: &source_server.uri(),
            current_server_uri: &current_server.uri(),
        },
    )?;

    let mut secondary = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, secondary.initialize()).await??;
    let fork_params = match source {
        ForkSource::ThreadId => ThreadForkParams {
            thread_id: source_thread_id.clone(),
            ..Default::default()
        },
        ForkSource::Path => ThreadForkParams {
            thread_id: "ignored-when-path-is-present".to_string(),
            path: Some(source_thread_path),
            ..Default::default()
        },
    };
    let fork_id = secondary.send_thread_fork_request(fork_params).await?;
    let fork_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_response_message(RequestId::Integer(fork_id)),
    )
    .await??;
    let ThreadForkResponse {
        thread: forked,
        model,
        model_provider,
        service_tier,
        approval_policy,
        sandbox,
        reasoning_effort,
        ..
    } = to_response::<ThreadForkResponse>(fork_resp)?;
    assert_eq!(forked.forked_from_id, Some(source_thread_id));
    assert_eq!(model, "gpt-5.2-codex");
    assert_eq!(model_provider, "source_provider");
    assert_eq!(service_tier, Some(ServiceTier::Flex));
    assert_eq!(
        approval_policy,
        codex_app_server_protocol::AskForApproval::OnRequest
    );
    assert_eq!(sandbox, SandboxPolicy::DangerFullAccess);
    assert_eq!(reasoning_effort, Some(ReasoningEffort::High));

    let turn_id = secondary
        .send_turn_start_request(TurnStartParams {
            thread_id: forked.id,
            input: vec![UserInput::Text {
                text: "continue fork with source config".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_response_message(RequestId::Integer(turn_id)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let source_requests = source_mock.requests();
    assert_eq!(source_requests.len(), 2);
    assert_no_responses_requests(&current_server).await?;
    let forked_turn_request = source_requests
        .last()
        .ok_or_else(|| anyhow::anyhow!("forked turn request"))?;
    assert_source_config_request(forked_turn_request);

    Ok(())
}

#[tokio::test]
async fn thread_fork_by_path_prefers_latest_turn_context_over_session_configured() -> Result<()> {
    let source_server = responses::start_mock_server().await;
    let current_server = responses::start_mock_server().await;
    let source_mock = responses::mount_sse_sequence(
        &source_server,
        vec![
            source_config_sse("source-seed-resp", "source-seed-msg"),
            source_config_sse("source-fork-resp", "source-fork-msg"),
        ],
    )
    .await;
    mount_current_provider_responder(&current_server).await;
    let codex_home = TempDir::new()?;
    write_dual_provider_config_toml(
        codex_home.path(),
        DualProviderConfig {
            default_provider: "source_provider",
            model: "gpt-5.2-codex",
            model_reasoning_effort: "high",
            service_tier: "flex",
            approval_policy: "on-request",
            sandbox_mode: "danger-full-access",
            instructions: "source base instructions",
            developer_instructions: "source developer instructions",
            source_server_uri: &source_server.uri(),
            current_server_uri: &current_server.uri(),
        },
    )?;

    let mut primary = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, primary.initialize()).await??;
    let start_id = primary
        .send_thread_start_request(ThreadStartParams {
            persist_extended_history: false,
            ..Default::default()
        })
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(start_resp)?;
    let materialize_id = primary
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "seed source config".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_response_message(RequestId::Integer(materialize_id)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_notification_message("turn/completed"),
    )
    .await??;
    let source_thread_id = thread.id.clone();
    let source_thread_path = thread
        .path
        .clone()
        .ok_or_else(|| anyhow::anyhow!("source thread path"))?;
    rewrite_latest_turn_context_for_fork_override(source_thread_path.as_path())?;
    drop(primary);

    write_dual_provider_config_toml(
        codex_home.path(),
        DualProviderConfig {
            default_provider: "current_provider",
            model: "gpt-5.1-codex-max",
            model_reasoning_effort: "low",
            service_tier: "fast",
            approval_policy: "never",
            sandbox_mode: "read-only",
            instructions: "current base instructions",
            developer_instructions: "current developer instructions",
            source_server_uri: &source_server.uri(),
            current_server_uri: &current_server.uri(),
        },
    )?;

    let mut secondary = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, secondary.initialize()).await??;
    let fork_id = secondary
        .send_thread_fork_request(ThreadForkParams {
            thread_id: "ignored-when-path-is-present".to_string(),
            path: Some(source_thread_path),
            ..Default::default()
        })
        .await?;
    let fork_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_response_message(RequestId::Integer(fork_id)),
    )
    .await??;
    let ThreadForkResponse {
        thread: forked,
        model,
        model_provider,
        service_tier,
        approval_policy,
        sandbox,
        reasoning_effort,
        ..
    } = to_response::<ThreadForkResponse>(fork_resp)?;
    assert_eq!(forked.forked_from_id, Some(source_thread_id));
    assert_eq!(model, "gpt-5-turn-context-fork");
    assert_eq!(model_provider, "source_provider");
    assert_eq!(service_tier, Some(ServiceTier::Fast));
    assert_eq!(
        approval_policy,
        codex_app_server_protocol::AskForApproval::Never
    );
    assert_eq!(
        sandbox.to_core(),
        RolloutSandboxPolicy::new_read_only_policy()
    );
    assert_eq!(reasoning_effort, Some(ReasoningEffort::Low));

    let turn_id = secondary
        .send_turn_start_request(TurnStartParams {
            thread_id: forked.id,
            input: vec![UserInput::Text {
                text: "continue fork with turn context override".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_response_message(RequestId::Integer(turn_id)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let source_requests = source_mock.requests();
    assert_eq!(source_requests.len(), 2);
    assert_no_responses_requests(&current_server).await?;
    let forked_turn_request = source_requests
        .last()
        .ok_or_else(|| anyhow::anyhow!("forked turn request"))?;
    assert_turn_context_override_request(forked_turn_request);

    Ok(())
}

#[tokio::test]
async fn thread_fork_by_path_fallback_preserves_source_config_baseline_when_rollout_context_missing()
-> Result<()> {
    let source_server = responses::start_mock_server().await;
    let current_server = responses::start_mock_server().await;
    let source_mock = responses::mount_sse_sequence(
        &source_server,
        vec![
            source_config_sse("source-seed-resp", "source-seed-msg"),
            source_config_sse("source-fork-resp", "source-fork-msg"),
        ],
    )
    .await;
    mount_current_provider_responder(&current_server).await;
    let codex_home = TempDir::new()?;
    write_dual_provider_config_toml(
        codex_home.path(),
        DualProviderConfig {
            default_provider: "source_provider",
            model: "gpt-5.2-codex",
            model_reasoning_effort: "high",
            service_tier: "flex",
            approval_policy: "on-request",
            sandbox_mode: "danger-full-access",
            instructions: "source base instructions",
            developer_instructions: "source developer instructions",
            source_server_uri: &source_server.uri(),
            current_server_uri: &current_server.uri(),
        },
    )?;

    let mut primary = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, primary.initialize()).await??;
    let start_id = primary
        .send_thread_start_request(ThreadStartParams {
            persist_extended_history: false,
            ..Default::default()
        })
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(start_resp)?;
    let materialize_id = primary
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "seed source config".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_response_message(RequestId::Integer(materialize_id)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_notification_message("turn/completed"),
    )
    .await??;
    let source_thread_id = thread.id.clone();
    let source_thread_path = thread
        .path
        .clone()
        .ok_or_else(|| anyhow::anyhow!("source thread path"))?;
    strip_rollout_context_for_fork_fallback(source_thread_path.as_path())?;
    drop(primary);

    write_dual_provider_config_toml(
        codex_home.path(),
        DualProviderConfig {
            default_provider: "current_provider",
            model: "gpt-5.1-codex-max",
            model_reasoning_effort: "low",
            service_tier: "fast",
            approval_policy: "never",
            sandbox_mode: "read-only",
            instructions: "current base instructions",
            developer_instructions: "current developer instructions",
            source_server_uri: &source_server.uri(),
            current_server_uri: &current_server.uri(),
        },
    )?;

    let mut secondary = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, secondary.initialize()).await??;
    let fork_id = secondary
        .send_thread_fork_request(ThreadForkParams {
            thread_id: "ignored-when-path-is-present".to_string(),
            path: Some(source_thread_path),
            ..Default::default()
        })
        .await?;
    let fork_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_response_message(RequestId::Integer(fork_id)),
    )
    .await??;
    let ThreadForkResponse {
        thread: forked,
        model,
        model_provider,
        service_tier,
        approval_policy,
        sandbox,
        reasoning_effort,
        ..
    } = to_response::<ThreadForkResponse>(fork_resp)?;
    assert_eq!(forked.forked_from_id, Some(source_thread_id));
    assert_eq!(model, "gpt-5.2-codex");
    assert_eq!(model_provider, "source_provider");
    assert_eq!(service_tier, Some(ServiceTier::Flex));
    assert_eq!(
        approval_policy,
        codex_app_server_protocol::AskForApproval::OnRequest
    );
    assert_eq!(sandbox, SandboxPolicy::DangerFullAccess);
    assert_eq!(reasoning_effort, Some(ReasoningEffort::High));

    let turn_id = secondary
        .send_turn_start_request(TurnStartParams {
            thread_id: forked.id,
            input: vec![UserInput::Text {
                text: "continue fork with fallback baseline".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_response_message(RequestId::Integer(turn_id)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let source_requests = source_mock.requests();
    assert_eq!(source_requests.len(), 2);
    assert_no_responses_requests(&current_server).await?;
    let forked_turn_request = source_requests
        .last()
        .ok_or_else(|| anyhow::anyhow!("forked turn request"))?;
    assert_source_config_request(forked_turn_request);

    Ok(())
}

#[tokio::test]
async fn thread_fork_by_path_preserves_source_thread_id_without_session_meta() -> Result<()> {
    let source_server = responses::start_mock_server().await;
    let current_server = responses::start_mock_server().await;
    let source_mock = responses::mount_sse_sequence(
        &source_server,
        vec![
            source_config_sse("source-seed-resp", "source-seed-msg"),
            source_config_sse("source-fork-resp", "source-fork-msg"),
        ],
    )
    .await;
    mount_current_provider_responder(&current_server).await;
    let codex_home = TempDir::new()?;
    write_dual_provider_config_toml(
        codex_home.path(),
        DualProviderConfig {
            default_provider: "source_provider",
            model: "gpt-5.2-codex",
            model_reasoning_effort: "high",
            service_tier: "flex",
            approval_policy: "on-request",
            sandbox_mode: "danger-full-access",
            instructions: "source base instructions",
            developer_instructions: "source developer instructions",
            source_server_uri: &source_server.uri(),
            current_server_uri: &current_server.uri(),
        },
    )?;

    let mut primary = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, primary.initialize()).await??;
    let start_id = primary
        .send_thread_start_request(ThreadStartParams {
            persist_extended_history: false,
            ..Default::default()
        })
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(start_resp)?;
    let materialize_id = primary
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "seed source config".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_response_message(RequestId::Integer(materialize_id)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_notification_message("turn/completed"),
    )
    .await??;
    let source_thread_id = thread.id.clone();
    let source_thread_path = thread
        .path
        .clone()
        .ok_or_else(|| anyhow::anyhow!("source thread path"))?;
    strip_source_resolution_metadata_for_fork_path(source_thread_path.as_path())?;
    drop(primary);

    write_dual_provider_config_toml(
        codex_home.path(),
        DualProviderConfig {
            default_provider: "current_provider",
            model: "gpt-5.1-codex-max",
            model_reasoning_effort: "low",
            service_tier: "fast",
            approval_policy: "never",
            sandbox_mode: "read-only",
            instructions: "current base instructions",
            developer_instructions: "current developer instructions",
            source_server_uri: &source_server.uri(),
            current_server_uri: &current_server.uri(),
        },
    )?;

    let mut secondary = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, secondary.initialize()).await??;
    let fork_id = secondary
        .send_thread_fork_request(ThreadForkParams {
            thread_id: "ignored-when-path-is-present".to_string(),
            path: Some(source_thread_path),
            ..Default::default()
        })
        .await?;
    let fork_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_response_message(RequestId::Integer(fork_id)),
    )
    .await??;
    let ThreadForkResponse {
        thread: forked,
        model,
        model_provider,
        service_tier,
        approval_policy,
        sandbox,
        reasoning_effort,
        ..
    } = to_response::<ThreadForkResponse>(fork_resp)?;
    assert_eq!(forked.forked_from_id, Some(source_thread_id));
    assert_eq!(model, "gpt-5.2-codex");
    assert_eq!(model_provider, "source_provider");
    assert_eq!(service_tier, Some(ServiceTier::Flex));
    assert_eq!(
        approval_policy,
        codex_app_server_protocol::AskForApproval::OnRequest
    );
    assert_eq!(sandbox, SandboxPolicy::DangerFullAccess);
    assert_eq!(reasoning_effort, Some(ReasoningEffort::High));

    let turn_id = secondary
        .send_turn_start_request(TurnStartParams {
            thread_id: forked.id,
            input: vec![UserInput::Text {
                text: "continue fork after session-meta removal".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_response_message(RequestId::Integer(turn_id)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let source_requests = source_mock.requests();
    assert_eq!(source_requests.len(), 2);
    wait_for_responses_request_count(&current_server, /*expected_count*/ 0).await?;
    let forked_turn_request = source_requests
        .last()
        .ok_or_else(|| anyhow::anyhow!("forked turn request"))?;
    assert_source_config_request(forked_turn_request);

    Ok(())
}

#[tokio::test]
async fn thread_fork_by_path_persists_base_instructions_override_for_child_fallback() -> Result<()>
{
    let source_server = responses::start_mock_server().await;
    let current_server = responses::start_mock_server().await;
    let source_mock = responses::mount_sse_sequence(
        &source_server,
        vec![
            source_config_sse("source-seed-resp", "source-seed-msg"),
            source_config_sse("source-fork-resp", "source-fork-msg"),
        ],
    )
    .await;
    mount_current_provider_responder(&current_server).await;
    let codex_home = TempDir::new()?;
    write_dual_provider_config_toml(
        codex_home.path(),
        DualProviderConfig {
            default_provider: "source_provider",
            model: "gpt-5.2-codex",
            model_reasoning_effort: "high",
            service_tier: "flex",
            approval_policy: "on-request",
            sandbox_mode: "danger-full-access",
            instructions: "source base instructions",
            developer_instructions: "source developer instructions",
            source_server_uri: &source_server.uri(),
            current_server_uri: &current_server.uri(),
        },
    )?;

    let mut primary = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, primary.initialize()).await??;
    let start_id = primary
        .send_thread_start_request(ThreadStartParams {
            persist_extended_history: false,
            ..Default::default()
        })
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(start_resp)?;
    let materialize_id = primary
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "seed source config".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_response_message(RequestId::Integer(materialize_id)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_notification_message("turn/completed"),
    )
    .await??;
    let source_thread_path = thread
        .path
        .clone()
        .ok_or_else(|| anyhow::anyhow!("source thread path"))?;
    drop(primary);

    write_dual_provider_config_toml(
        codex_home.path(),
        DualProviderConfig {
            default_provider: "current_provider",
            model: "gpt-5.1-codex-max",
            model_reasoning_effort: "low",
            service_tier: "fast",
            approval_policy: "never",
            sandbox_mode: "read-only",
            instructions: "current base instructions",
            developer_instructions: "current developer instructions",
            source_server_uri: &source_server.uri(),
            current_server_uri: &current_server.uri(),
        },
    )?;

    let mut secondary = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, secondary.initialize()).await??;
    let fork_id = secondary
        .send_thread_fork_request(ThreadForkParams {
            thread_id: "ignored-when-path-is-present".to_string(),
            path: Some(source_thread_path),
            base_instructions: Some("forked base instructions".to_string()),
            ..Default::default()
        })
        .await?;
    let fork_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_response_message(RequestId::Integer(fork_id)),
    )
    .await??;
    let ThreadForkResponse { thread: forked, .. } = to_response::<ThreadForkResponse>(fork_resp)?;
    let forked_path = forked
        .path
        .clone()
        .ok_or_else(|| anyhow::anyhow!("forked thread path"))?;
    drop(secondary);

    strip_rollout_context_for_fork_fallback(forked_path.as_path())?;

    let mut tertiary = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, tertiary.initialize()).await??;
    let resume_id = tertiary
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: "ignored-when-path-is-present".to_string(),
            path: Some(forked_path),
            ..Default::default()
        })
        .await?;
    let resume_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        tertiary.read_stream_until_response_message(RequestId::Integer(resume_id)),
    )
    .await??;
    let ThreadResumeResponse {
        thread: resumed_fork,
        ..
    } = to_response::<ThreadResumeResponse>(resume_resp)?;

    let turn_id = tertiary
        .send_turn_start_request(TurnStartParams {
            thread_id: resumed_fork.id,
            input: vec![UserInput::Text {
                text: "continue fork with persisted override".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        tertiary.read_stream_until_response_message(RequestId::Integer(turn_id)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        tertiary.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let source_requests = source_mock.requests();
    assert_eq!(source_requests.len(), 2);
    assert_no_responses_requests(&current_server).await?;
    let forked_turn_request = source_requests
        .last()
        .ok_or_else(|| anyhow::anyhow!("forked turn request"))?;
    assert_persisted_fork_base_instructions_override_request(forked_turn_request);

    Ok(())
}

#[tokio::test]
async fn thread_fork_by_path_persists_base_instructions_override_without_rollout_rewrite()
-> Result<()> {
    let source_server = responses::start_mock_server().await;
    let current_server = responses::start_mock_server().await;
    let source_mock = responses::mount_sse_sequence(
        &source_server,
        vec![
            source_config_sse("source-seed-resp", "source-seed-msg"),
            source_config_sse("source-fork-resp", "source-fork-msg"),
        ],
    )
    .await;
    mount_current_provider_responder(&current_server).await;
    let codex_home = TempDir::new()?;
    write_dual_provider_config_toml(
        codex_home.path(),
        DualProviderConfig {
            default_provider: "source_provider",
            model: "gpt-5.2-codex",
            model_reasoning_effort: "high",
            service_tier: "flex",
            approval_policy: "on-request",
            sandbox_mode: "danger-full-access",
            instructions: "source base instructions",
            developer_instructions: "source developer instructions",
            source_server_uri: &source_server.uri(),
            current_server_uri: &current_server.uri(),
        },
    )?;

    let mut primary = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, primary.initialize()).await??;
    let start_id = primary
        .send_thread_start_request(ThreadStartParams {
            persist_extended_history: false,
            ..Default::default()
        })
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(start_resp)?;
    let materialize_id = primary
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "seed source config".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_response_message(RequestId::Integer(materialize_id)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        primary.read_stream_until_notification_message("turn/completed"),
    )
    .await??;
    let source_thread_path = thread
        .path
        .clone()
        .ok_or_else(|| anyhow::anyhow!("source thread path"))?;
    drop(primary);

    write_dual_provider_config_toml(
        codex_home.path(),
        DualProviderConfig {
            default_provider: "current_provider",
            model: "gpt-5.1-codex-max",
            model_reasoning_effort: "low",
            service_tier: "fast",
            approval_policy: "never",
            sandbox_mode: "read-only",
            instructions: "current base instructions",
            developer_instructions: "current developer instructions",
            source_server_uri: &source_server.uri(),
            current_server_uri: &current_server.uri(),
        },
    )?;

    let mut secondary = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, secondary.initialize()).await??;
    let fork_id = secondary
        .send_thread_fork_request(ThreadForkParams {
            thread_id: "ignored-when-path-is-present".to_string(),
            path: Some(source_thread_path),
            base_instructions: Some("forked base instructions".to_string()),
            ..Default::default()
        })
        .await?;
    let fork_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        secondary.read_stream_until_response_message(RequestId::Integer(fork_id)),
    )
    .await??;
    let ThreadForkResponse { thread: forked, .. } = to_response::<ThreadForkResponse>(fork_resp)?;
    let forked_path = forked
        .path
        .clone()
        .ok_or_else(|| anyhow::anyhow!("forked thread path"))?;
    drop(secondary);

    let mut tertiary = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, tertiary.initialize()).await??;
    let resume_child_id = tertiary
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: "ignored-when-path-is-present".to_string(),
            path: Some(forked_path),
            ..Default::default()
        })
        .await?;
    let resume_child_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        tertiary.read_stream_until_response_message(RequestId::Integer(resume_child_id)),
    )
    .await??;
    let ThreadResumeResponse {
        thread: resumed_child,
        ..
    } = to_response::<ThreadResumeResponse>(resume_child_resp)?;
    assert_eq!(resumed_child.id, forked.id);

    let turn_id = tertiary
        .send_turn_start_request(TurnStartParams {
            thread_id: resumed_child.id,
            input: vec![UserInput::Text {
                text: "continue child with persisted override".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        tertiary.read_stream_until_response_message(RequestId::Integer(turn_id)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        tertiary.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let source_requests = source_mock.requests();
    assert_eq!(source_requests.len(), 2);
    assert_no_responses_requests(&current_server).await?;
    let forked_turn_request = source_requests
        .last()
        .ok_or_else(|| anyhow::anyhow!("forked turn request"))?;
    assert_persisted_fork_base_instructions_override_request(forked_turn_request);

    Ok(())
}

struct DualProviderConfig<'a> {
    default_provider: &'a str,
    model: &'a str,
    model_reasoning_effort: &'a str,
    service_tier: &'a str,
    approval_policy: &'a str,
    sandbox_mode: &'a str,
    instructions: &'a str,
    developer_instructions: &'a str,
    source_server_uri: &'a str,
    current_server_uri: &'a str,
}

fn source_config_sse(response_id: &str, message_id: &str) -> String {
    responses::sse(vec![
        responses::ev_response_created(response_id),
        responses::ev_assistant_message(message_id, "Done"),
        responses::ev_completed(response_id),
    ])
}

async fn mount_current_provider_responder(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(responses::sse_response(source_config_sse(
            "current-resp",
            "current-msg",
        )))
        .mount(server)
        .await;
}

async fn assert_no_responses_requests(server: &MockServer) -> Result<()> {
    let requests = server
        .received_requests()
        .await
        .ok_or_else(|| anyhow::anyhow!("wiremock did not record requests"))?;
    let responses_request_count = requests
        .iter()
        .filter(|request| request.method == "POST" && request.url.path().ends_with("/responses"))
        .count();
    assert_eq!(
        responses_request_count, 0,
        "source fork should not use the current default provider"
    );
    Ok(())
}

fn write_dual_provider_config_toml(
    codex_home: &Path,
    config: DualProviderConfig<'_>,
) -> std::io::Result<()> {
    let config_toml = codex_home.join("config.toml");
    let model_instructions_file = codex_home.join("model_instructions.md");
    std::fs::write(&model_instructions_file, config.instructions)?;
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "{model}"
model_reasoning_effort = "{model_reasoning_effort}"
service_tier = "{service_tier}"
approval_policy = "{approval_policy}"
sandbox_mode = "{sandbox_mode}"
model_instructions_file = "{model_instructions_file}"
developer_instructions = "{developer_instructions}"

model_provider = "{default_provider}"

[features]
personality = true

[model_providers.source_provider]
name = "Source provider for fork baseline test"
base_url = "{source_server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0

[model_providers.current_provider]
name = "Current provider for fork baseline test"
base_url = "{current_server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#,
            model = config.model,
            model_reasoning_effort = config.model_reasoning_effort,
            service_tier = config.service_tier,
            approval_policy = config.approval_policy,
            sandbox_mode = config.sandbox_mode,
            model_instructions_file = model_instructions_file.display(),
            developer_instructions = config.developer_instructions,
            default_provider = config.default_provider,
            source_server_uri = config.source_server_uri,
            current_server_uri = config.current_server_uri,
        ),
    )
}

fn assert_source_config_request(request: &responses::ResponsesRequest) {
    let body = request.body_json();
    assert_eq!(body["model"], json!("gpt-5.2-codex"));
    assert_eq!(body["service_tier"], json!("flex"));
    assert_eq!(body["reasoning"]["effort"], json!("high"));

    let instructions_text = request.instructions_text();
    assert!(
        instructions_text.contains("source base instructions"),
        "expected source base instructions, got {instructions_text:?}"
    );
    assert!(
        !instructions_text.contains("current base instructions"),
        "forked turn should not use current base instructions: {instructions_text:?}"
    );

    let developer_texts = request.message_input_texts("developer");
    assert!(
        developer_texts
            .iter()
            .any(|text| text.contains("source developer instructions")),
        "expected source developer instructions, got {developer_texts:?}"
    );
    assert!(
        developer_texts
            .iter()
            .all(|text| !text.contains("current developer instructions")),
        "forked turn should not use current developer instructions: {developer_texts:?}"
    );
}

fn assert_zero_turn_start_override_request(request: &responses::ResponsesRequest) {
    let body = request.body_json();
    assert_eq!(body["model"], json!("gpt-5.2-codex"));
    assert_eq!(body["service_tier"], json!("flex"));
    assert_eq!(body["reasoning"]["effort"], json!("high"));

    let instructions_text = request.instructions_text();
    assert!(
        instructions_text.contains("zero-turn baseline base instructions"),
        "expected zero-turn baseline base instructions, got {instructions_text:?}"
    );
    assert!(
        !instructions_text.contains("source base instructions"),
        "forked turn should not fall back to stale config-file base instructions: {instructions_text:?}"
    );
    assert!(
        !instructions_text.contains("current base instructions"),
        "forked turn should not use current base instructions: {instructions_text:?}"
    );

    let developer_texts = request.message_input_texts("developer");
    assert!(
        developer_texts
            .iter()
            .any(|text| text.contains("zero-turn baseline developer instructions")),
        "expected zero-turn baseline developer instructions, got {developer_texts:?}"
    );
    assert!(
        developer_texts
            .iter()
            .all(|text| !text.contains("source developer instructions")),
        "forked turn should not fall back to stale config-file developer instructions: {developer_texts:?}"
    );
    assert!(
        developer_texts
            .iter()
            .all(|text| !text.contains("current developer instructions")),
        "forked turn should not use current developer instructions: {developer_texts:?}"
    );
    assert!(
        developer_texts
            .iter()
            .any(|text| text.contains("<personality_spec>")),
        "expected start-only personality in developer input, got {developer_texts:?}"
    );
}

fn assert_turn_context_override_request(request: &responses::ResponsesRequest) {
    let body = request.body_json();
    assert_eq!(body["model"], json!("gpt-5-turn-context-fork"));
    assert_eq!(body["service_tier"], json!("priority"));
    assert_eq!(body["reasoning"]["effort"], json!("low"));

    let instructions_text = request.instructions_text();
    assert!(
        instructions_text.contains("source base instructions"),
        "expected source base instructions, got {instructions_text:?}"
    );
    assert!(
        !instructions_text.contains("current base instructions"),
        "forked turn should not use current base instructions: {instructions_text:?}"
    );

    let developer_texts = request.message_input_texts("developer");
    assert!(
        developer_texts
            .iter()
            .any(|text| text.contains("turn context developer instructions")),
        "expected turn-context developer instructions, got {developer_texts:?}"
    );
    assert!(
        developer_texts
            .iter()
            .all(|text| !text.contains("source developer instructions")),
        "forked turn should not fall back to source developer instructions: {developer_texts:?}"
    );
    assert!(
        developer_texts
            .iter()
            .all(|text| !text.contains("current developer instructions")),
        "forked turn should not use current developer instructions: {developer_texts:?}"
    );
}

fn assert_persisted_fork_base_instructions_override_request(request: &responses::ResponsesRequest) {
    let body = request.body_json();
    assert_eq!(body["model"], json!("gpt-5.2-codex"));
    assert_eq!(body["service_tier"], json!("flex"));
    assert_eq!(body["reasoning"]["effort"], json!("high"));

    let instructions_text = request.instructions_text();
    assert!(
        instructions_text.contains("forked base instructions"),
        "expected persisted fork override instructions, got {instructions_text:?}"
    );
    assert!(
        !instructions_text.contains("source base instructions"),
        "forked turn should not fall back to stale source instructions: {instructions_text:?}"
    );
    assert!(
        !instructions_text.contains("current base instructions"),
        "forked turn should not use current base instructions: {instructions_text:?}"
    );

    let developer_texts = request.message_input_texts("developer");
    assert!(
        developer_texts
            .iter()
            .any(|text| text.contains("source developer instructions")),
        "expected source developer instructions, got {developer_texts:?}"
    );
    assert!(
        developer_texts
            .iter()
            .all(|text| !text.contains("current developer instructions")),
        "forked turn should not use current developer instructions: {developer_texts:?}"
    );
}

fn rewrite_latest_turn_context_for_fork_override(rollout_path: &Path) -> Result<()> {
    let contents = std::fs::read_to_string(rollout_path)?;
    let mut lines: Vec<String> = contents.lines().map(str::to_string).collect();
    let Some(line) = lines.iter_mut().rev().find(|line| {
        serde_json::from_str::<RolloutLine>(line)
            .is_ok_and(|rollout_line| matches!(rollout_line.item, RolloutItem::TurnContext(_)))
    }) else {
        anyhow::bail!(
            "rollout at {} is missing a turn context",
            rollout_path.display()
        );
    };
    let mut rollout_line: RolloutLine = serde_json::from_str(line)?;
    let RolloutItem::TurnContext(turn_context) = &mut rollout_line.item else {
        anyhow::bail!(
            "rollout at {} latest turn-context line decoded as a different item",
            rollout_path.display()
        );
    };
    apply_fork_turn_context_override(turn_context);
    *line = serde_json::to_string(&rollout_line)?;
    std::fs::write(rollout_path, lines.join("\n") + "\n")?;
    Ok(())
}

fn apply_fork_turn_context_override(turn_context: &mut TurnContextItem) {
    turn_context.model = "gpt-5-turn-context-fork".to_string();
    turn_context.cwd = PathBuf::from("/tmp/turn-context-fork");
    turn_context.approval_policy = RolloutAskForApproval::Never;
    turn_context.sandbox_policy = RolloutSandboxPolicy::new_read_only_policy();
    turn_context.service_tier = Some(ServiceTier::Fast);
    turn_context.effort = Some(ReasoningEffort::Low);
    turn_context.developer_instructions = Some("turn context developer instructions".to_string());
}

fn strip_rollout_context_for_fork_fallback(rollout_path: &Path) -> Result<()> {
    let contents = std::fs::read_to_string(rollout_path)?;
    let mut rewritten = Vec::new();
    for line in contents.lines() {
        let mut rollout_line: RolloutLine = serde_json::from_str(line)?;
        match &mut rollout_line.item {
            RolloutItem::SessionMeta(meta_line) => {
                meta_line.meta.base_instructions = None;
                rewritten.push(serde_json::to_string(&rollout_line)?);
            }
            RolloutItem::TurnContext(_) | RolloutItem::EventMsg(EventMsg::SessionConfigured(_)) => {
            }
            RolloutItem::EventMsg(_) | RolloutItem::ResponseItem(_) | RolloutItem::Compacted(_) => {
                rewritten.push(serde_json::to_string(&rollout_line)?)
            }
        }
    }
    std::fs::write(rollout_path, rewritten.join("\n") + "\n")?;
    Ok(())
}

fn strip_source_resolution_metadata_for_fork_path(rollout_path: &Path) -> Result<()> {
    let contents = std::fs::read_to_string(rollout_path)?;
    let mut rewritten = Vec::new();
    for line in contents.lines() {
        let rollout_line: RolloutLine = serde_json::from_str(line)?;
        match rollout_line.item {
            RolloutItem::SessionMeta(_)
            | RolloutItem::TurnContext(_)
            | RolloutItem::EventMsg(EventMsg::SessionConfigured(_)) => {}
            RolloutItem::EventMsg(_) | RolloutItem::ResponseItem(_) | RolloutItem::Compacted(_) => {
                rewritten.push(serde_json::to_string(&rollout_line)?)
            }
        }
    }
    std::fs::write(rollout_path, rewritten.join("\n") + "\n")?;
    Ok(())
}

fn prepend_mismatched_fork_session_meta(rollout_path: &Path) -> Result<()> {
    let existing_contents = std::fs::read_to_string(rollout_path)?;
    let mismatched_line = RolloutLine {
        timestamp: "2026-04-24T00:00:00Z".to_string(),
        item: RolloutItem::SessionMeta(SessionMetaLine {
            meta: SessionMeta {
                id: ThreadId::new(),
                forked_from_id: None,
                timestamp: "2026-04-24T00:00:00Z".to_string(),
                cwd: PathBuf::from("/tmp/polluted-session-cwd"),
                originator: "codex".to_string(),
                cli_version: "9.9.9".to_string(),
                source: RolloutSessionSource::Cli,
                agent_path: None,
                agent_nickname: None,
                agent_role: None,
                model_provider: Some("current_provider".to_string()),
                base_instructions: Some(BaseInstructions {
                    text: "polluted base instructions".to_string(),
                }),
                dynamic_tools: None,
                memory_mode: None,
            },
            git: None,
        }),
    };
    let mut rewritten = vec![serde_json::to_string(&mismatched_line)?];
    rewritten.extend(existing_contents.lines().map(ToString::to_string));
    std::fs::write(rollout_path, rewritten.join("\n") + "\n")?;
    Ok(())
}

// Helper to create a config.toml pointing at the mock model server.
fn create_config_toml(codex_home: &Path, server_uri: &str) -> std::io::Result<()> {
    let config_toml = codex_home.join("config.toml");
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "read-only"

model_provider = "mock_provider"

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#
        ),
    )
}

fn create_config_toml_with_chatgpt_base_url(
    codex_home: &Path,
    server_uri: &str,
    chatgpt_base_url: &str,
    general_analytics_enabled: bool,
) -> std::io::Result<()> {
    let general_analytics_toml = if general_analytics_enabled {
        "\ngeneral_analytics = true".to_string()
    } else {
        "\ngeneral_analytics = false".to_string()
    };
    let config_toml = codex_home.join("config.toml");
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "read-only"
chatgpt_base_url = "{chatgpt_base_url}"

model_provider = "mock_provider"

[features]
{general_analytics_toml}

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#
        ),
    )
}
