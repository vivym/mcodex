use anyhow::bail;
use codex_account_pool::LocalDefaultPoolClearRequest;
use codex_account_pool::LocalDefaultPoolSetRequest;
use codex_account_pool::clear_local_default_pool;
use codex_account_pool::set_local_default_pool;
use codex_core::config::Config;
use codex_product_identity::MCODEX;
use codex_state::StateRuntime;

pub(crate) async fn set_default_pool(
    runtime: &StateRuntime,
    config: &Config,
    account_pool_override_id: Option<&str>,
    pool_id: &str,
) -> anyhow::Result<()> {
    reject_process_local_override(account_pool_override_id)?;

    let configured_default_pool_id = configured_default_pool_id(config).map(ToOwned::to_owned);
    let outcome = set_local_default_pool(
        runtime,
        LocalDefaultPoolSetRequest {
            pool_id: pool_id.to_string(),
            configured_default_pool_id: configured_default_pool_id.clone(),
        },
    )
    .await?;

    if outcome.state_changed {
        println!("default pool set: {pool_id}");
    } else {
        println!("default pool unchanged: {pool_id}");
    }
    if outcome.preferred_account_cleared {
        println!("preferred startup selection reset");
    }
    if let Some(configured_default_pool_id) = configured_default_pool_id {
        println!(
            "configured default pool still controls pooled startup: {configured_default_pool_id}"
        );
    }
    if outcome.suppressed {
        println!(
            "pooled startup remains paused; run `{} accounts resume`",
            MCODEX.binary_name
        );
    }

    Ok(())
}

pub(crate) async fn clear_default_pool(
    runtime: &StateRuntime,
    config: &Config,
    account_pool_override_id: Option<&str>,
) -> anyhow::Result<()> {
    reject_process_local_override(account_pool_override_id)?;

    let configured_default_pool_id = configured_default_pool_id(config).map(ToOwned::to_owned);
    let outcome = clear_local_default_pool(
        runtime,
        LocalDefaultPoolClearRequest {
            configured_default_pool_id: configured_default_pool_id.clone(),
        },
    )
    .await?;

    if outcome.state_changed {
        println!("default pool cleared");
    } else {
        println!("default pool already cleared");
    }
    if outcome.preferred_account_cleared {
        println!("preferred startup selection reset");
    }
    if let Some(configured_default_pool_id) = configured_default_pool_id {
        println!(
            "configured default pool still controls pooled startup: {configured_default_pool_id}"
        );
    }
    if outcome.suppressed {
        println!(
            "pooled startup remains paused; run `{} accounts resume`",
            MCODEX.binary_name
        );
    }

    Ok(())
}

pub(crate) fn reject_process_local_override(
    account_pool_override_id: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(pool_id) = account_pool_override_id {
        bail!(
            "`--account-pool {pool_id}` cannot be combined with `{} accounts pool default`; persistent default mutation cannot be combined with a process-local override",
            MCODEX.binary_name
        );
    }

    Ok(())
}

fn configured_default_pool_id(config: &Config) -> Option<&str> {
    config
        .accounts
        .as_ref()
        .and_then(|accounts| accounts.default_pool.as_deref())
}
