use anyhow::Context;
use codex_core::config::Config;
use codex_login::AuthManager;
use codex_login::LegacyAuthView;
use codex_state::AccountStartupSelectionUpdate;
use codex_state::LegacyAccountImport;
use codex_state::NewPendingAccountRegistration;
use codex_state::StateRuntime;
use std::borrow::ToOwned;

const LEGACY_DEFAULT_POOL_ID: &str = "legacy-default";

pub(crate) struct ImportedLegacyAccount {
    pub account_id: String,
    pub pool_id: String,
}

pub(crate) async fn import_legacy_account(
    runtime: &StateRuntime,
    config: &Config,
    pool: Option<&str>,
    account_pool_override: Option<&str>,
) -> anyhow::Result<ImportedLegacyAccount> {
    let auth_manager =
        AuthManager::shared_from_config(config, /*enable_codex_api_key_env*/ true);
    let legacy_auth = LegacyAuthView::new(&auth_manager);
    let Some(account_id) = legacy_auth
        .current()
        .await
        .and_then(|auth| auth.get_account_id())
    else {
        anyhow::bail!("no legacy compatibility account is available");
    };

    let target_pool_id = pool
        .or(account_pool_override)
        .map(ToOwned::to_owned)
        .or_else(|| {
            config
                .accounts
                .as_ref()
                .and_then(|accounts| accounts.default_pool.clone())
        })
        .unwrap_or_else(|| LEGACY_DEFAULT_POOL_ID.to_string());
    let idempotency_key = format!("legacy-import:{account_id}:{target_pool_id}");

    runtime
        .create_pending_account_registration(NewPendingAccountRegistration {
            idempotency_key: idempotency_key.clone(),
            backend_id: "local".to_string(),
            provider_kind: "chatgpt".to_string(),
            target_pool_id: Some(target_pool_id.clone()),
            backend_account_handle: Some(account_id.clone()),
            account_id: Some(account_id.clone()),
        })
        .await
        .context("create pending legacy account import")?;

    let mut imported = false;
    let result = async {
        runtime
            .import_legacy_default_account(LegacyAccountImport {
                account_id: account_id.clone(),
            })
            .await
            .context("import legacy account into pooled state")?;
        imported = true;

        if target_pool_id != LEGACY_DEFAULT_POOL_ID {
            runtime
                .assign_account_pool(&account_id, &target_pool_id)
                .await
                .context("assign legacy account to target pool")?;
            runtime
                .write_account_startup_selection(AccountStartupSelectionUpdate {
                    default_pool_id: Some(target_pool_id.clone()),
                    preferred_account_id: Some(account_id.clone()),
                    suppressed: false,
                })
                .await
                .context("record target pool selection")?;
        }

        runtime
            .write_account_compat_migration_state(true)
            .await
            .context("mark legacy import completed")?;
        runtime
            .finalize_pending_account_registration(&idempotency_key, &account_id, &account_id)
            .await
            .context("finalize pending legacy account import")?;
        Ok::<(), anyhow::Error>(())
    }
    .await;

    if let Err(err) = result {
        if !imported {
            let _ = runtime
                .clear_pending_account_registration(&idempotency_key)
                .await;
        }
        return Err(err);
    }

    Ok(ImportedLegacyAccount {
        account_id,
        pool_id: target_pool_id,
    })
}
