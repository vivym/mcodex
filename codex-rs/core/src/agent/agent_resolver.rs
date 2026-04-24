use crate::agent::control::ResolvedAgentReference;
use crate::agent::registry::AgentMetadata;
use crate::codex::Session;
use crate::codex::TurnContext;
use crate::function_tool::FunctionCallError;
use codex_protocol::ThreadId;
use std::sync::Arc;

/// Resolves a single tool-facing agent target with metadata from the same namespace lookup.
pub(crate) async fn resolve_agent_target_with_metadata(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    target: &str,
) -> Result<ResolvedAgentReference, FunctionCallError> {
    register_session_root(session, turn);
    if let Ok(thread_id) = ThreadId::from_string(target) {
        let metadata = session
            .services
            .agent_control
            .get_agent_metadata(thread_id)
            .unwrap_or(AgentMetadata {
                agent_id: Some(thread_id),
                ..Default::default()
            });
        return Ok(ResolvedAgentReference {
            thread_id,
            metadata,
        });
    }

    session
        .services
        .agent_control
        .resolve_agent_reference_with_metadata(
            session.conversation_id,
            &turn.session_source,
            target,
        )
        .await
        .map_err(|err| match err {
            codex_protocol::error::CodexErr::UnsupportedOperation(message) => {
                FunctionCallError::RespondToModel(message)
            }
            other => FunctionCallError::RespondToModel(other.to_string()),
        })
}

fn register_session_root(session: &Arc<Session>, turn: &Arc<TurnContext>) {
    session
        .services
        .agent_control
        .register_session_root(session.conversation_id, &turn.session_source);
}
