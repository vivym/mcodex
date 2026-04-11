use async_trait::async_trait;
use codex_state::LegacyAccountImport;

/// Provides access to a legacy single-account snapshot for one-time pool bootstrap.
#[async_trait]
pub trait LegacyAuthBootstrap: Send + Sync {
    /// Returns the currently available legacy account, if one exists.
    async fn current_legacy_auth(&self) -> anyhow::Result<Option<LegacyAccountImport>>;
}

/// Default bootstrap source used when legacy auth is unavailable.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoLegacyAuthBootstrap;

#[async_trait]
impl LegacyAuthBootstrap for NoLegacyAuthBootstrap {
    async fn current_legacy_auth(&self) -> anyhow::Result<Option<LegacyAccountImport>> {
        Ok(None)
    }
}
