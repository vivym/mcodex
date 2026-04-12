use anyhow::Context;
use codex_state::AccountSource;
use codex_state::StateRuntime;

pub(super) async fn list_accounts(runtime: &StateRuntime) -> anyhow::Result<()> {
    let memberships = runtime
        .list_account_pool_memberships(None)
        .await
        .context("list registered accounts")?;

    if memberships.is_empty() {
        println!("No accounts registered.");
        return Ok(());
    }

    for membership in memberships {
        println!(
            "{} pool={} enabled={} healthy={}{}",
            membership.account_id,
            membership.pool_id,
            membership.enabled,
            membership.healthy,
            source_suffix(membership.source)
        );
    }

    Ok(())
}

pub(super) async fn set_account_enabled(
    runtime: &StateRuntime,
    account_id: &str,
    enabled: bool,
) -> anyhow::Result<()> {
    let updated = runtime
        .set_account_enabled(account_id, enabled)
        .await
        .with_context(|| format!("update account `{account_id}` enabled state"))?;
    if !updated {
        anyhow::bail!("account `{account_id}` is not registered");
    }

    let status = if enabled { "enabled" } else { "disabled" };
    println!("account {account_id}: {status}");
    Ok(())
}

pub(super) async fn remove_account(runtime: &StateRuntime, account_id: &str) -> anyhow::Result<()> {
    let removed = runtime
        .remove_account_registry_entry(account_id)
        .await
        .with_context(|| format!("remove account `{account_id}`"))?;
    if !removed {
        anyhow::bail!("account `{account_id}` is not registered");
    }

    println!("account removed: {account_id}");
    Ok(())
}

pub(super) async fn list_account_pools(runtime: &StateRuntime) -> anyhow::Result<()> {
    let memberships = runtime
        .list_account_pool_memberships(None)
        .await
        .context("list account pool memberships")?;
    let mut pool_ids = memberships
        .into_iter()
        .map(|membership| membership.pool_id)
        .collect::<Vec<_>>();
    pool_ids.sort();
    pool_ids.dedup();

    if pool_ids.is_empty() {
        println!("No account pools registered.");
        return Ok(());
    }

    for pool_id in pool_ids {
        println!("{pool_id}");
    }

    Ok(())
}

pub(super) async fn assign_account_pool(
    runtime: &StateRuntime,
    account_id: &str,
    pool_id: &str,
) -> anyhow::Result<()> {
    let assigned = runtime
        .assign_account_pool(account_id, pool_id)
        .await
        .with_context(|| format!("assign account `{account_id}` to pool `{pool_id}`"))?;
    if !assigned {
        anyhow::bail!("account `{account_id}` is not registered");
    }

    println!("account {account_id}: pool {pool_id}");
    Ok(())
}

fn source_suffix(source: Option<AccountSource>) -> String {
    source.map_or_else(String::new, |source| format!(" source={}", source.as_str()))
}
