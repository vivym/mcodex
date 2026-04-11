use crate::types::ContextReuseDecision;
use crate::types::ContextReuseRequest;

/// Decide whether remote context can be reused across account rotation.
pub fn evaluate_context_reuse(request: ContextReuseRequest) -> ContextReuseDecision {
    if request.allow_context_reuse
        && request.explicit_context_reuse_consent
        && request.same_workspace
        && request.same_backend_family
        && request.transport_portable
    {
        ContextReuseDecision::ReuseRemoteContext
    } else {
        ContextReuseDecision::ResetRemoteContext
    }
}
