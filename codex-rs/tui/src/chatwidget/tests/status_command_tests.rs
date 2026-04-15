use super::*;
use crate::status::StatusAccountLeaseDisplay;
use assert_matches::assert_matches;

#[tokio::test]
async fn status_command_renders_immediately_and_refreshes_rate_limits_for_chatgpt_auth() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_chatgpt_auth(&mut chat);

    chat.dispatch_command(SlashCommand::Status);

    let rendered = match rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(cell)) => {
            lines_to_single_string(&cell.display_lines(/*width*/ 80))
        }
        other => panic!("expected status output before refresh request, got {other:?}"),
    };
    assert!(
        !rendered.contains("refreshing limits"),
        "expected /status to avoid transient refresh text in terminal history, got: {rendered}"
    );
    let request_id = match rx.try_recv() {
        Ok(AppEvent::RefreshRateLimits {
            origin: RateLimitRefreshOrigin::StatusCommand { request_id },
        }) => request_id,
        other => panic!("expected rate-limit refresh request, got {other:?}"),
    };
    pretty_assertions::assert_eq!(request_id, 0);
}

#[tokio::test]
async fn status_command_refresh_updates_cached_limits_for_future_status_outputs() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_chatgpt_auth(&mut chat);

    chat.dispatch_command(SlashCommand::Status);

    match rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(_)) => {}
        other => panic!("expected status output before refresh request, got {other:?}"),
    }
    let first_request_id = match rx.try_recv() {
        Ok(AppEvent::RefreshRateLimits {
            origin: RateLimitRefreshOrigin::StatusCommand { request_id },
        }) => request_id,
        other => panic!("expected rate-limit refresh request, got {other:?}"),
    };

    chat.on_rate_limit_snapshot(Some(snapshot(/*percent*/ 92.0)));
    chat.finish_status_rate_limit_refresh(first_request_id);
    drain_insert_history(&mut rx);

    chat.dispatch_command(SlashCommand::Status);
    let refreshed = match rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(cell)) => {
            lines_to_single_string(&cell.display_lines(/*width*/ 80))
        }
        other => panic!("expected refreshed status output, got {other:?}"),
    };
    assert!(
        refreshed.contains("8% left"),
        "expected a future /status output to use refreshed cached limits, got: {refreshed}"
    );
}

#[tokio::test]
async fn status_command_renders_pooled_lease_details() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.status_account_lease_display = Some(StatusAccountLeaseDisplay {
        pool_id: Some("legacy-default".to_string()),
        account_id: Some("acct-2".to_string()),
        status: "Cooling down · Busy".to_string(),
        note: Some("Automatic selection in use".to_string()),
        next_eligible_at: Some("03:24 on 11 Apr".to_string()),
        remote_reset: Some("gen 2 after turn turn-17".to_string()),
    });

    chat.dispatch_command(SlashCommand::Status);

    let rendered = match rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(cell)) => {
            lines_to_single_string(&cell.display_lines(/*width*/ 80))
        }
        other => panic!("expected status output, got {other:?}"),
    };
    assert!(
        rendered.contains("Pooled lease:"),
        "expected pooled lease line, got: {rendered}"
    );
    assert!(
        rendered.contains("legacy-default / acct-2"),
        "expected pool/account assignment, got: {rendered}"
    );
    assert!(
        rendered.contains("Remote reset:"),
        "expected remote reset line, got: {rendered}"
    );
}

#[tokio::test]
async fn account_lease_updated_adds_automatic_switch_notice_when_account_changes() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.update_account_lease_state(Some(StatusAccountLeaseDisplay {
        pool_id: Some("legacy-default".to_string()),
        account_id: Some("acct-1".to_string()),
        status: "Active · Healthy".to_string(),
        note: Some("Automatic selection in use".to_string()),
        next_eligible_at: None,
        remote_reset: None,
    }));
    assert!(drain_insert_history(&mut rx).is_empty());

    chat.update_account_lease_state(Some(StatusAccountLeaseDisplay {
        pool_id: Some("legacy-default".to_string()),
        account_id: Some("acct-2".to_string()),
        status: "Active · Healthy".to_string(),
        note: Some("Automatic selection in use".to_string()),
        next_eligible_at: None,
        remote_reset: Some("gen 1 after turn turn-2".to_string()),
    }));

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one switch notice");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("acct-2"),
        "expected switched account id in notice, got: {rendered}"
    );
    assert!(
        rendered.contains("Automatic selection in use"),
        "expected switch reason in notice, got: {rendered}"
    );
}

#[tokio::test]
async fn account_lease_updated_adds_non_replayable_turn_notice() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.update_account_lease_state(Some(StatusAccountLeaseDisplay {
        pool_id: Some("legacy-default".to_string()),
        account_id: Some("acct-1".to_string()),
        status: "Active · Healthy".to_string(),
        note: Some("Automatic selection in use".to_string()),
        next_eligible_at: None,
        remote_reset: None,
    }));
    assert!(drain_insert_history(&mut rx).is_empty());

    chat.update_account_lease_state(Some(StatusAccountLeaseDisplay {
        pool_id: Some("legacy-default".to_string()),
        account_id: Some("acct-1".to_string()),
        status: "Active · Healthy".to_string(),
        note: Some(
            "Current turn was not replayed; future turns will use the next eligible account"
                .to_string(),
        ),
        next_eligible_at: Some("03:24".to_string()),
        remote_reset: None,
    }));

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one non-replayable notice");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("Current turn was not replayed"),
        "expected non-replayable copy, got: {rendered}"
    );
    assert!(
        rendered.contains("03:24"),
        "expected next eligible hint, got: {rendered}"
    );
}

#[tokio::test]
async fn account_lease_updated_adds_no_eligible_account_error_notice() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.update_account_lease_state(Some(StatusAccountLeaseDisplay {
        pool_id: Some("legacy-default".to_string()),
        account_id: Some("acct-1".to_string()),
        status: "Active · Healthy".to_string(),
        note: Some("Automatic selection in use".to_string()),
        next_eligible_at: None,
        remote_reset: None,
    }));
    assert!(drain_insert_history(&mut rx).is_empty());

    chat.update_account_lease_state(Some(StatusAccountLeaseDisplay {
        pool_id: Some("legacy-default".to_string()),
        account_id: None,
        status: "Waiting · Unavailable".to_string(),
        note: Some("No eligible account is available".to_string()),
        next_eligible_at: Some("03:24".to_string()),
        remote_reset: None,
    }));

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one unavailable notice");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("No eligible pooled account is available"),
        "expected unavailable copy, got: {rendered}"
    );
    assert!(
        rendered.contains("03:24"),
        "expected next eligible hint, got: {rendered}"
    );
}

#[tokio::test]
async fn status_command_renders_immediately_without_rate_limit_refresh() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Status);

    assert_matches!(rx.try_recv(), Ok(AppEvent::InsertHistoryCell(_)));
    assert!(
        !std::iter::from_fn(|| rx.try_recv().ok())
            .any(|event| matches!(event, AppEvent::RefreshRateLimits { .. })),
        "non-ChatGPT sessions should not request a rate-limit refresh for /status"
    );
}

#[tokio::test]
async fn status_command_renders_instruction_sources_from_thread_session() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.instruction_source_paths = vec![chat.config.cwd.join("AGENTS.md").to_path_buf()];

    chat.dispatch_command(SlashCommand::Status);

    let rendered = match rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(cell)) => {
            lines_to_single_string(&cell.display_lines(/*width*/ 80))
        }
        other => panic!("expected status output, got {other:?}"),
    };
    assert!(
        rendered.contains("Agents.md"),
        "expected /status to render app-server instruction sources, got: {rendered}"
    );
    assert!(
        !rendered.contains("Agents.md  <none>"),
        "expected /status to avoid stale <none> when app-server provided instruction sources, got: {rendered}"
    );
}

#[tokio::test]
async fn status_command_overlapping_refreshes_update_matching_cells_only() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_chatgpt_auth(&mut chat);

    chat.dispatch_command(SlashCommand::Status);
    match rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(_)) => {}
        other => panic!("expected first status output, got {other:?}"),
    }
    let first_request_id = match rx.try_recv() {
        Ok(AppEvent::RefreshRateLimits {
            origin: RateLimitRefreshOrigin::StatusCommand { request_id },
        }) => request_id,
        other => panic!("expected first refresh request, got {other:?}"),
    };

    chat.dispatch_command(SlashCommand::Status);
    let second_rendered = match rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(cell)) => {
            lines_to_single_string(&cell.display_lines(/*width*/ 80))
        }
        other => panic!("expected second status output, got {other:?}"),
    };
    let second_request_id = match rx.try_recv() {
        Ok(AppEvent::RefreshRateLimits {
            origin: RateLimitRefreshOrigin::StatusCommand { request_id },
        }) => request_id,
        other => panic!("expected second refresh request, got {other:?}"),
    };

    assert_ne!(first_request_id, second_request_id);
    assert!(
        !second_rendered.contains("refreshing limits"),
        "expected /status to avoid transient refresh text in terminal history, got: {second_rendered}"
    );

    chat.finish_status_rate_limit_refresh(first_request_id);
    pretty_assertions::assert_eq!(chat.refreshing_status_outputs.len(), 1);

    chat.on_rate_limit_snapshot(Some(snapshot(/*percent*/ 92.0)));
    chat.finish_status_rate_limit_refresh(second_request_id);
    assert!(chat.refreshing_status_outputs.is_empty());
}
