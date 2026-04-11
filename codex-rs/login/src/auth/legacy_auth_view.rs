use crate::auth::AuthManager;
use crate::auth::CodexAuth;

/// Compatibility view that reads auth from the shared manager snapshot.
pub struct LegacyAuthView<'a> {
    manager: &'a AuthManager,
}

impl<'a> LegacyAuthView<'a> {
    pub fn new(manager: &'a AuthManager) -> Self {
        Self { manager }
    }

    pub async fn current(&self) -> Option<CodexAuth> {
        self.manager.auth_cached()
    }
}
