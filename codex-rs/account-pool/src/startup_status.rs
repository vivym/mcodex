use crate::AccountPoolExecutionBackend;
use codex_state::AccountStartupStatus;
use codex_state::EffectivePoolResolutionSource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedStartupStatus {
    pub startup: AccountStartupStatus,
    pub pooled_applicable: bool,
}

pub async fn read_shared_startup_status<B: AccountPoolExecutionBackend>(
    backend: &B,
    configured_default_pool_id: Option<&str>,
    explicit_override_pool_id: Option<&str>,
) -> anyhow::Result<SharedStartupStatus> {
    let mut startup = backend
        .read_account_startup_status(explicit_override_pool_id.or(configured_default_pool_id))
        .await?;
    if explicit_override_pool_id.is_some() {
        startup.configured_default_pool_id = configured_default_pool_id.map(ToOwned::to_owned);
        startup.effective_pool_resolution_source = EffectivePoolResolutionSource::Override;
    }

    Ok(SharedStartupStatus {
        pooled_applicable: explicit_override_pool_id.is_some()
            || matches!(
                startup.effective_pool_resolution_source,
                EffectivePoolResolutionSource::Override
                    | EffectivePoolResolutionSource::PersistedSelection
            ),
        startup,
    })
}
