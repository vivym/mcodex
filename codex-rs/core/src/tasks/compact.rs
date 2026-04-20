use std::sync::Arc;

use super::SessionTask;
use super::SessionTaskContext;
use crate::codex::TurnContext;
use crate::state::TaskKind;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::ErrorEvent;
use codex_protocol::protocol::EventMsg;
use codex_protocol::user_input::UserInput;
use tokio_util::sync::CancellationToken;
use tracing::warn;

#[derive(Clone, Copy, Default)]
pub(crate) struct CompactTask;

impl SessionTask for CompactTask {
    fn kind(&self) -> TaskKind {
        TaskKind::Compact
    }

    fn span_name(&self) -> &'static str {
        "session_task.compact"
    }

    async fn run(
        self: Arc<Self>,
        session: Arc<SessionTaskContext>,
        ctx: Arc<TurnContext>,
        input: Vec<UserInput>,
        cancellation_token: CancellationToken,
    ) -> Option<String> {
        let session = session.clone_session();
        let use_remote_compact = crate::compact::should_use_remote_compact_task(&ctx.provider);
        let pooled_mode_enabled = use_remote_compact && session.services.pooled_runtime_active();
        let turn_account_pool_manager = session.services.account_pool_manager_for_turn();
        let turn_account_selection = if use_remote_compact {
            if let Some(account_pool_manager) = turn_account_pool_manager.as_ref() {
                let mut account_pool_manager = account_pool_manager.lock().await;
                match account_pool_manager.prepare_turn().await {
                    Ok(selection) => {
                        session.services.lease_auth.replace_current(
                            selection
                                .as_ref()
                                .map(|selection| std::sync::Arc::clone(&selection.auth_session)),
                        );
                        selection
                    }
                    Err(err) => {
                        warn!("failed to prepare account-pool lease for compact task: {err:#}");
                        session.services.lease_auth.clear();
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };
        if pooled_mode_enabled && turn_account_selection.is_none() {
            session
                .send_event(
                    &ctx,
                    EventMsg::Error(ErrorEvent {
                        message: "No eligible pooled account is available for this turn."
                            .to_string(),
                        codex_error_info: Some(CodexErrorInfo::Other),
                    }),
                )
                .await;
            return None;
        }

        let turn_account_id_override = turn_account_selection
            .as_ref()
            .map(|selection| selection.account_id.clone());
        let _account_pool_lease_heartbeat = crate::codex::start_account_pool_lease_heartbeat(
            &session,
            turn_account_selection.is_some(),
            &cancellation_token,
        )
        .await;
        let _ = if use_remote_compact {
            let reset_remote_context = if let Some(selection) = turn_account_selection.as_ref() {
                session
                    .services
                    .reset_remote_context_for_selection(selection)
                    .await
            } else {
                false
            };
            if reset_remote_context {
                session
                    .services
                    .model_client
                    .reset_remote_session_identity();
            }
            session.services.session_telemetry.counter(
                "codex.task.compact",
                /*inc*/ 1,
                &[("type", "remote")],
            );
            crate::compact_remote::run_remote_compact_task(
                session.clone(),
                ctx,
                turn_account_id_override,
            )
            .await
        } else {
            session.services.session_telemetry.counter(
                "codex.task.compact",
                /*inc*/ 1,
                &[("type", "local")],
            );
            crate::compact::run_compact_task(session.clone(), ctx, input).await
        };
        None
    }
}
