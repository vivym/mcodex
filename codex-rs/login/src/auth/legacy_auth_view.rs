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

#[cfg(test)]
mod tests {
    use super::LegacyAuthView;
    use crate::auth::AuthManager;
    use crate::auth::CodexAuth;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn legacy_auth_view_reads_auth_manager_snapshot() {
        let manager =
            AuthManager::from_auth_for_testing(CodexAuth::create_dummy_chatgpt_auth_for_testing());

        let legacy = LegacyAuthView::new(&manager);
        let current = legacy.current().await.expect("expected auth snapshot");
        assert_eq!(current.get_account_id(), Some("account_id".to_string()));
    }
}
