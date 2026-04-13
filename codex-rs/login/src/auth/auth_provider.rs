use std::sync::Arc;

use async_trait::async_trait;

use super::AuthManager;
use super::CodexAuth;
use super::RefreshTokenError;
use super::UnauthorizedRecovery;

/// Result metadata produced by one unauthorized-recovery step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthRecoveryStepResult {
    auth_state_changed: Option<bool>,
}

impl AuthRecoveryStepResult {
    pub fn new(auth_state_changed: Option<bool>) -> Self {
        Self { auth_state_changed }
    }

    pub fn auth_state_changed(self) -> Option<bool> {
        self.auth_state_changed
    }
}

/// One-shot or multi-step auth recovery workflow used after unauthorized responses.
///
/// Implementations should encapsulate the recovery strategy for a specific auth source and expose
/// stable telemetry names through `mode_name()` and `step_name()`.
#[async_trait]
pub trait AuthRecovery: Send {
    fn has_next(&self) -> bool;
    fn unavailable_reason(&self) -> &'static str;
    fn mode_name(&self) -> &'static str;
    fn step_name(&self) -> &'static str;
    async fn next(&mut self) -> Result<AuthRecoveryStepResult, RefreshTokenError>;
}

/// Async auth snapshot source for code that needs the current auth state outside the request path.
///
/// Callers should treat the returned [`CodexAuth`] as an immutable snapshot and request a fresh
/// snapshot for each independent operation.
#[async_trait]
pub trait AuthProvider: Send + Sync {
    async fn auth(&self) -> Option<CodexAuth>;
}

/// Auth snapshot source that can also produce unauthorized-recovery workflows.
///
/// Use this when the caller may need to retry after a 401/403-style auth failure instead of only
/// reading the current auth snapshot.
pub trait RefreshingAuthProvider: AuthProvider {
    fn unauthorized_recovery(&self) -> Option<Box<dyn AuthRecovery>>;
}

#[derive(Clone, Debug)]
pub struct SharedAuthProvider {
    auth_manager: Arc<AuthManager>,
}

impl SharedAuthProvider {
    pub fn new(auth_manager: Arc<AuthManager>) -> Self {
        Self { auth_manager }
    }
}

struct SharedAuthRecovery {
    recovery: UnauthorizedRecovery,
}

#[async_trait]
impl AuthRecovery for SharedAuthRecovery {
    fn has_next(&self) -> bool {
        self.recovery.has_next()
    }

    fn unavailable_reason(&self) -> &'static str {
        self.recovery.unavailable_reason()
    }

    fn mode_name(&self) -> &'static str {
        self.recovery.mode_name()
    }

    fn step_name(&self) -> &'static str {
        self.recovery.step_name()
    }

    async fn next(&mut self) -> Result<AuthRecoveryStepResult, RefreshTokenError> {
        self.recovery
            .next()
            .await
            .map(|step_result| AuthRecoveryStepResult::new(step_result.auth_state_changed()))
    }
}

#[async_trait]
impl AuthProvider for SharedAuthProvider {
    async fn auth(&self) -> Option<CodexAuth> {
        self.auth_manager.auth().await
    }
}

impl RefreshingAuthProvider for SharedAuthProvider {
    fn unauthorized_recovery(&self) -> Option<Box<dyn AuthRecovery>> {
        Some(Box::new(SharedAuthRecovery {
            recovery: self.auth_manager.unauthorized_recovery(),
        }))
    }
}
