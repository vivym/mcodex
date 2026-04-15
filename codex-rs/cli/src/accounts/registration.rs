use super::diagnostics::read_current_diagnostic;
use anyhow::Context;
use codex_account_pool::AccountPoolConfig;
use codex_account_pool::AccountPoolControlPlane;
use codex_account_pool::LocalAccountPoolBackend;
use codex_account_pool::RegisteredAccountRegistration;
use codex_core::config::Config;
use codex_login::AuthManager;
use codex_login::CLIENT_ID;
use codex_login::ChatgptManagedRegistrationTokens;
use codex_login::LegacyAuthView;
use codex_login::ServerOptions;
use codex_login::run_pooled_browser_registration;
use codex_login::run_pooled_device_code_registration;
use codex_state::AccountStartupSelectionUpdate;
use codex_state::LegacyAccountImport;
use codex_state::NewPendingAccountRegistration;
use codex_state::RegisteredAccountMembership;
use codex_state::RegisteredAccountRecord;
use codex_state::RegisteredAccountUpsert;
use codex_state::StateRuntime;
use std::borrow::ToOwned;
use std::sync::Arc;

const LEGACY_DEFAULT_POOL_ID: &str = "legacy-default";

#[derive(Debug)]
pub(crate) struct RegisteredAddAccount {
    pub account_id: String,
    pub provider_account_id: String,
    pub pool_id: String,
}

pub(crate) struct ImportedLegacyAccount {
    pub account_id: String,
    pub pool_id: String,
}

trait ChatgptRegistrationRunner {
    async fn run_browser(
        &self,
        config: &Config,
    ) -> std::io::Result<ChatgptManagedRegistrationTokens>;

    async fn run_device_auth(
        &self,
        config: &Config,
    ) -> std::io::Result<ChatgptManagedRegistrationTokens>;
}

struct LiveChatgptRegistrationRunner;

impl ChatgptRegistrationRunner for LiveChatgptRegistrationRunner {
    async fn run_browser(
        &self,
        config: &Config,
    ) -> std::io::Result<ChatgptManagedRegistrationTokens> {
        run_pooled_browser_registration(chatgpt_registration_server_options(config)).await
    }

    async fn run_device_auth(
        &self,
        config: &Config,
    ) -> std::io::Result<ChatgptManagedRegistrationTokens> {
        run_pooled_device_code_registration(chatgpt_registration_server_options(config)).await
    }
}

trait AddRegistrationControlPlane: Send + Sync {
    async fn register_account(
        &self,
        request: RegisteredAccountRegistration,
    ) -> anyhow::Result<RegisteredAccountRecord>;

    async fn delete_registered_account(&self, account_id: &str) -> anyhow::Result<bool>;
}

impl<T> AddRegistrationControlPlane for T
where
    T: AccountPoolControlPlane + Send + Sync,
{
    async fn register_account(
        &self,
        request: RegisteredAccountRegistration,
    ) -> anyhow::Result<RegisteredAccountRecord> {
        AccountPoolControlPlane::register_account(self, request).await
    }

    async fn delete_registered_account(&self, account_id: &str) -> anyhow::Result<bool> {
        AccountPoolControlPlane::delete_registered_account(self, account_id).await
    }
}

trait PendingRegistrationFinalizer: Send + Sync {
    async fn finalize_pending_account_registration(
        &self,
        runtime: &StateRuntime,
        idempotency_key: &str,
        backend_account_handle: &str,
        account_id: &str,
    ) -> anyhow::Result<()>;
}

pub(crate) struct RuntimePendingRegistrationFinalizer;

impl PendingRegistrationFinalizer for RuntimePendingRegistrationFinalizer {
    async fn finalize_pending_account_registration(
        &self,
        runtime: &StateRuntime,
        idempotency_key: &str,
        backend_account_handle: &str,
        account_id: &str,
    ) -> anyhow::Result<()> {
        let finalized = runtime
            .finalize_pending_account_registration(
                idempotency_key,
                backend_account_handle,
                account_id,
            )
            .await?;
        if !finalized {
            anyhow::bail!("pending registration `{idempotency_key}` is not active");
        }
        Ok(())
    }
}

pub(crate) async fn add_chatgpt_account(
    runtime: &Arc<StateRuntime>,
    config: &Config,
    account_pool_override: Option<&str>,
    device_auth: bool,
) -> anyhow::Result<RegisteredAddAccount> {
    add_chatgpt_account_with_runner(
        runtime,
        config,
        account_pool_override,
        device_auth,
        &LiveChatgptRegistrationRunner,
    )
    .await
}

async fn add_chatgpt_account_with_runner(
    runtime: &Arc<StateRuntime>,
    config: &Config,
    account_pool_override: Option<&str>,
    device_auth: bool,
    runner: &impl ChatgptRegistrationRunner,
) -> anyhow::Result<RegisteredAddAccount> {
    let backend = LocalAccountPoolBackend::new(
        Arc::clone(runtime),
        AccountPoolConfig::default().lease_ttl_duration(),
    );
    add_chatgpt_account_with_dependencies(
        runtime,
        config,
        account_pool_override,
        device_auth,
        runner,
        &backend,
        &RuntimePendingRegistrationFinalizer,
    )
    .await
}

async fn add_chatgpt_account_with_dependencies(
    runtime: &Arc<StateRuntime>,
    config: &Config,
    account_pool_override: Option<&str>,
    device_auth: bool,
    runner: &impl ChatgptRegistrationRunner,
    control_plane: &impl AddRegistrationControlPlane,
    finalizer: &impl PendingRegistrationFinalizer,
) -> anyhow::Result<RegisteredAddAccount> {
    let diagnostic = read_current_diagnostic(runtime.as_ref(), config, account_pool_override)
        .await
        .context("resolve current account pool")?;
    let Some(pool_id) = diagnostic.preview.effective_pool_id else {
        anyhow::bail!(
            "no account pool is configured; pass `--account-pool <POOL_ID>` or configure a pool before running `codex accounts add chatgpt`"
        );
    };

    let tokens = if device_auth {
        runner
            .run_device_auth(config)
            .await
            .context("run ChatGPT device-code registration")?
    } else {
        runner
            .run_browser(config)
            .await
            .context("run ChatGPT browser registration")?
    };
    let provider_account_id = tokens.account_id.clone();
    let requested_account_id = chatgpt_local_account_id(&provider_account_id);
    let idempotency_key = format!("chatgpt-add:local:{provider_account_id}:{pool_id}");

    reconcile_pending_add_registration(runtime.as_ref(), &idempotency_key, control_plane).await?;
    if runtime
        .read_pending_account_registration(&idempotency_key)
        .await?
        .is_some_and(|pending| pending.completed_at.is_some())
    {
        runtime
            .clear_pending_account_registration(&idempotency_key)
            .await
            .context("clear completed ChatGPT registration before retry")?;
    }

    let provider_fingerprint = provider_fingerprint(&provider_account_id);
    let existing_requested_account = runtime
        .read_registered_account(&requested_account_id)
        .await?;
    let existing_memberships = runtime.list_account_pool_memberships(None).await?;
    let mut same_pool_existing_position = None;
    let mut same_pool_existing_account_id = None;
    for membership in existing_memberships {
        let Some(registered) = runtime
            .read_registered_account(&membership.account_id)
            .await?
        else {
            continue;
        };
        if registered.provider_fingerprint != provider_fingerprint {
            continue;
        }
        if membership.pool_id != pool_id {
            anyhow::bail!(
                "provider identity `{provider_account_id}` is already registered in pool `{}`; use `codex accounts pool assign` if you need to move it",
                membership.pool_id
            );
        }
        if same_pool_existing_account_id.is_none() {
            same_pool_existing_position = Some(
                runtime
                    .read_account_pool_position(&membership.account_id)
                    .await?
                    .unwrap_or(0),
            );
            same_pool_existing_account_id = Some(membership.account_id);
        }
    }

    runtime
        .create_pending_account_registration(NewPendingAccountRegistration {
            idempotency_key: idempotency_key.clone(),
            backend_id: "local".to_string(),
            provider_kind: "chatgpt".to_string(),
            target_pool_id: Some(pool_id.clone()),
            backend_account_handle: None,
            account_id: None,
        })
        .await
        .context("create pending ChatGPT account registration")?;

    // Membership scans only surface assigned accounts. Re-registering an already-assigned
    // same-pool identity should still replay the backend registration path so legacy/raw
    // backend-private auth can be normalized and repaired. If this provider identity exists
    // without a pool assignment, register_account will reuse the canonical row by provider
    // fingerprint and attach it to the resolved pool.
    let registration_result = control_plane
        .register_account(RegisteredAccountRegistration {
            request: RegisteredAccountUpsert {
                account_id: requested_account_id.clone(),
                backend_id: "local".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some(provider_account_id.clone()),
                backend_account_handle: provider_account_id.clone(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: provider_fingerprint.clone(),
                display_name: Some("Managed ChatGPT".to_string()),
                source: None,
                enabled: true,
                healthy: true,
                membership: Some(RegisteredAccountMembership {
                    pool_id: pool_id.clone(),
                    position: same_pool_existing_position.unwrap_or(0),
                }),
            },
            pooled_registration_tokens: Some(tokens),
        })
        .await;

    let registered = match registration_result {
        Ok(registered) => registered,
        Err(err) => {
            runtime
                .clear_pending_account_registration(&idempotency_key)
                .await
                .context("clear pending ChatGPT registration after backend failure")?;
            return Err(err);
        }
    };
    let reused_existing_account = existing_requested_account.as_ref().is_some_and(|account| {
        account.provider_fingerprint == provider_fingerprint
            || account.backend_account_handle == provider_account_id
    }) || same_pool_existing_account_id.is_some()
        || registered.account_id != requested_account_id;

    if let Err(err) = finalizer
        .finalize_pending_account_registration(
            runtime.as_ref(),
            &idempotency_key,
            registered.backend_account_handle.as_str(),
            registered.account_id.as_str(),
        )
        .await
    {
        if reused_existing_account {
            runtime
                .clear_pending_account_registration(&idempotency_key)
                .await
                .context(
                    "clear pending ChatGPT registration after finalize failure on existing account",
                )?;
            return Err(err);
        }
        match control_plane
            .delete_registered_account(registered.account_id.as_str())
            .await
        {
            Ok(true) => {
                runtime
                    .clear_pending_account_registration(&idempotency_key)
                    .await
                    .context("clear pending ChatGPT registration after finalize compensation")?;
            }
            Ok(false) => {
                return Err(err.context(
                    "failed to finalize pending ChatGPT registration and compensation did not remove the registered account; manual recovery is required",
                ));
            }
            Err(compensation_err) => {
                return Err(err.context(format!(
                    "failed to finalize pending ChatGPT registration and compensation failed: {compensation_err}"
                )));
            }
        }
        return Err(err);
    }

    Ok(RegisteredAddAccount {
        account_id: registered.account_id,
        provider_account_id,
        pool_id,
    })
}

async fn reconcile_pending_add_registration(
    runtime: &StateRuntime,
    idempotency_key: &str,
    control_plane: &impl AddRegistrationControlPlane,
) -> anyhow::Result<()> {
    let Some(pending) = runtime
        .read_pending_account_registration(idempotency_key)
        .await?
    else {
        return Ok(());
    };
    if pending.completed_at.is_some() {
        return Ok(());
    }

    match (
        pending.backend_account_handle.as_deref(),
        pending.account_id.as_deref(),
    ) {
        (Some(backend_account_handle), Some(account_id)) => {
            if runtime.read_registered_account(account_id).await?.is_some() {
                let finalized = runtime
                    .finalize_pending_account_registration(
                        idempotency_key,
                        backend_account_handle,
                        account_id,
                    )
                    .await?;
                if !finalized {
                    anyhow::bail!("pending registration `{idempotency_key}` is not active");
                }
                return Ok(());
            }

            match control_plane.delete_registered_account(account_id).await {
                Ok(true) => {
                    runtime
                        .clear_pending_account_registration(idempotency_key)
                        .await
                        .context("clear compensated pending ChatGPT registration")?;
                    Ok(())
                }
                Ok(false) => anyhow::bail!(
                    "phase 1 manual recovery required for pending ChatGPT registration `{idempotency_key}`: local account `{account_id}` is missing and compensation could not confirm deletion"
                ),
                Err(err) => Err(err).context(format!(
                    "phase 1 manual recovery required for pending ChatGPT registration `{idempotency_key}`"
                )),
            }
        }
        (None, Some(account_id)) => {
            let Some(registered) = runtime.read_registered_account(account_id).await? else {
                runtime
                    .clear_pending_account_registration(idempotency_key)
                    .await
                    .context("clear stale pending ChatGPT registration")?;
                return Ok(());
            };
            let finalized = runtime
                .finalize_pending_account_registration(
                    idempotency_key,
                    registered.backend_account_handle.as_str(),
                    registered.account_id.as_str(),
                )
                .await?;
            if !finalized {
                anyhow::bail!("pending registration `{idempotency_key}` is not active");
            }
            Ok(())
        }
        (Some(backend_account_handle), None) => {
            anyhow::bail!(
                "phase 1 manual recovery required for pending ChatGPT registration `{idempotency_key}`: backend handle `{backend_account_handle}` is recorded without a local account id"
            )
        }
        (None, None) => {
            runtime
                .clear_pending_account_registration(idempotency_key)
                .await
                .context("clear empty pending ChatGPT registration")?;
            Ok(())
        }
    }
}

pub(crate) fn api_key_add_is_unsupported() -> anyhow::Result<RegisteredAddAccount> {
    anyhow::bail!(
        "phase 1 only supports `codex accounts add chatgpt`; `codex accounts add api-key` is not supported yet"
    )
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

fn chatgpt_registration_server_options(config: &Config) -> ServerOptions {
    ServerOptions::new(
        config.codex_home.clone().to_path_buf(),
        CLIENT_ID.to_string(),
        config.forced_chatgpt_workspace_id.clone(),
        config.cli_auth_credentials_store_mode,
    )
}

fn provider_fingerprint(provider_account_id: &str) -> String {
    format!("chatgpt::{provider_account_id}")
}

fn chatgpt_local_account_id(provider_account_id: &str) -> String {
    let local_account_id_suffix = provider_account_id
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("acct-local-{local_account_id_suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    use anyhow::Result;
    use codex_account_pool::RegisteredAccountRegistration;
    use codex_core::config::ConfigBuilder;
    use codex_login::ChatgptManagedRegistrationTokens;
    use codex_state::AccountPoolMembership;
    use codex_state::AccountStartupSelectionState;
    use codex_state::AccountStartupSelectionUpdate;
    use codex_state::RegisteredAccountMembership;
    use codex_state::RegisteredAccountRecord;
    use codex_state::RegisteredAccountUpsert;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tempfile::TempDir;

    #[tokio::test]
    async fn add_chatgpt_registration_uses_override_pool_and_keeps_startup_defaults() -> Result<()>
    {
        let harness = RegistrationHarness::with_configured_pool("team-main").await?;
        let runner = FakeChatgptRegistrationRunner::browser_success("provider-acct-new");
        let control_plane = RuntimeBackedControlPlane::new(harness.runtime.clone());

        let result = add_chatgpt_account_with_dependencies(
            &harness.runtime,
            &harness.config,
            Some("team-other"),
            /*device_auth*/ false,
            &runner,
            &control_plane,
            &RuntimePendingRegistrationFinalizer,
        )
        .await?;

        assert_eq!(result.pool_id, "team-other");
        assert_eq!(result.provider_account_id, "provider-acct-new");
        assert_eq!(
            harness.runtime.read_account_startup_selection().await?,
            AccountStartupSelectionState {
                default_pool_id: Some("team-main".to_string()),
                preferred_account_id: None,
                suppressed: false,
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn add_chatgpt_registration_fails_without_resolved_pool_before_persisting_state()
    -> Result<()> {
        let harness = RegistrationHarness::without_configured_pool().await?;
        let runner = FakeChatgptRegistrationRunner::browser_success("provider-acct-new");
        let control_plane = RuntimeBackedControlPlane::new(harness.runtime.clone());

        let err = add_chatgpt_account_with_dependencies(
            &harness.runtime,
            &harness.config,
            None,
            /*device_auth*/ false,
            &runner,
            &control_plane,
            &RuntimePendingRegistrationFinalizer,
        )
        .await
        .expect_err("missing pool should fail before registration");

        assert!(err.to_string().contains("configure a pool"));
        assert!(
            harness
                .runtime
                .list_pending_account_registrations()
                .await?
                .is_empty()
        );
        assert_eq!(runner.browser_calls(), 0);
        assert_eq!(runner.device_auth_calls(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn add_chatgpt_registration_is_idempotent_for_same_identity_in_same_pool() -> Result<()> {
        let harness = RegistrationHarness::with_configured_pool("team-main").await?;
        let runner = FakeChatgptRegistrationRunner::device_success("provider-acct-new");
        let control_plane = RuntimeBackedControlPlane::new(harness.runtime.clone());

        let first = add_chatgpt_account_with_dependencies(
            &harness.runtime,
            &harness.config,
            None,
            /*device_auth*/ true,
            &runner,
            &control_plane,
            &RuntimePendingRegistrationFinalizer,
        )
        .await?;
        let second = add_chatgpt_account_with_dependencies(
            &harness.runtime,
            &harness.config,
            None,
            /*device_auth*/ true,
            &runner,
            &control_plane,
            &RuntimePendingRegistrationFinalizer,
        )
        .await?;

        assert_eq!(first.account_id, second.account_id);
        assert_eq!(first.provider_account_id, "provider-acct-new");
        assert_eq!(second.provider_account_id, "provider-acct-new");
        assert_eq!(
            harness.membership(first.account_id.as_str()).await?,
            Some(AccountPoolMembership {
                account_id: first.account_id.clone(),
                pool_id: "team-main".to_string(),
                source: None,
                enabled: true,
                healthy: true,
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn add_chatgpt_registration_rejects_existing_identity_in_other_pool() -> Result<()> {
        let harness = RegistrationHarness::with_registered_account(
            "acct-local-1",
            "provider-acct-new",
            Some("team-main"),
        )
        .await?;
        let runner = FakeChatgptRegistrationRunner::browser_success("provider-acct-new");
        let control_plane = RuntimeBackedControlPlane::new(harness.runtime.clone());

        let err = add_chatgpt_account_with_dependencies(
            &harness.runtime,
            &harness.config,
            Some("team-other"),
            /*device_auth*/ false,
            &runner,
            &control_plane,
            &RuntimePendingRegistrationFinalizer,
        )
        .await
        .expect_err("cross-pool reuse should be rejected");

        assert!(err.to_string().contains("accounts pool assign"));
        Ok(())
    }

    #[tokio::test]
    async fn add_chatgpt_registration_assigns_existing_unassigned_identity_to_resolved_pool()
    -> Result<()> {
        let harness =
            RegistrationHarness::with_registered_account("acct-local-1", "provider-acct-new", None)
                .await?;
        let runner = FakeChatgptRegistrationRunner::browser_success("provider-acct-new");
        let control_plane = RuntimeBackedControlPlane::new(harness.runtime.clone());

        let result = add_chatgpt_account_with_dependencies(
            &harness.runtime,
            &harness.config,
            Some("team-main"),
            /*device_auth*/ false,
            &runner,
            &control_plane,
            &RuntimePendingRegistrationFinalizer,
        )
        .await?;

        assert_eq!(result.account_id, "acct-local-1");
        assert_eq!(result.provider_account_id, "provider-acct-new");
        assert_eq!(result.pool_id, "team-main");
        assert_eq!(
            harness
                .runtime
                .read_registered_account(&chatgpt_local_account_id("provider-acct-new"))
                .await?,
            None
        );
        assert_eq!(
            harness.membership("acct-local-1").await?,
            Some(AccountPoolMembership {
                account_id: "acct-local-1".to_string(),
                pool_id: "team-main".to_string(),
                source: None,
                enabled: true,
                healthy: true,
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn add_chatgpt_registration_reruns_backend_registration_for_same_pool_identity()
    -> Result<()> {
        let harness = RegistrationHarness::with_configured_pool("team-main").await?;
        let provider_account_id = "provider-acct-new";
        harness
            .runtime
            .upsert_registered_account(RegisteredAccountUpsert {
                account_id: "acct-local-1".to_string(),
                backend_id: "local".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some(provider_account_id.to_string()),
                backend_account_handle: provider_account_id.to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: provider_fingerprint(provider_account_id),
                display_name: Some("Managed ChatGPT".to_string()),
                source: None,
                enabled: true,
                healthy: true,
                membership: Some(RegisteredAccountMembership {
                    pool_id: "team-main".to_string(),
                    position: 0,
                }),
            })
            .await?;
        let legacy_auth_home = harness
            .runtime
            .codex_home()
            .join(".pooled-auth/backends/local/accounts")
            .join(provider_account_id);
        std::fs::create_dir_all(&legacy_auth_home)?;
        std::fs::write(legacy_auth_home.join("auth.json"), "{}")?;

        let runner =
            FakeChatgptRegistrationRunner::browser_success_with_valid_id_token(provider_account_id);
        let backend = LocalAccountPoolBackend::new(
            harness.runtime.clone(),
            AccountPoolConfig::default().lease_ttl_duration(),
        );

        let result = add_chatgpt_account_with_dependencies(
            &harness.runtime,
            &harness.config,
            None,
            /*device_auth*/ false,
            &runner,
            &backend,
            &RuntimePendingRegistrationFinalizer,
        )
        .await?;

        assert_eq!(result.account_id, "acct-local-1");
        assert_eq!(result.provider_account_id, provider_account_id);
        assert_eq!(result.pool_id, "team-main");
        assert_eq!(
            harness
                .runtime
                .read_registered_account("acct-local-1")
                .await?
                .expect("registered account")
                .backend_account_handle,
            normalized_chatgpt_backend_account_handle_for_test(provider_account_id)
        );
        assert!(
            !legacy_auth_home.exists(),
            "legacy raw backend-private auth should be removed"
        );
        assert!(
            harness
                .runtime
                .codex_home()
                .join(".pooled-auth/backends/local/accounts")
                .join(normalized_chatgpt_backend_account_handle_for_test(
                    provider_account_id
                ))
                .join("auth.json")
                .exists(),
            "normalized backend-private auth should be present"
        );
        Ok(())
    }

    #[tokio::test]
    async fn add_chatgpt_registration_preserves_existing_same_pool_position_on_rerun() -> Result<()>
    {
        let harness = RegistrationHarness::with_configured_pool("team-main").await?;
        let provider_account_id = "provider-acct-new";
        harness
            .runtime
            .upsert_registered_account(RegisteredAccountUpsert {
                account_id: "acct-local-1".to_string(),
                backend_id: "local".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some(provider_account_id.to_string()),
                backend_account_handle: provider_account_id.to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: provider_fingerprint(provider_account_id),
                display_name: Some("Managed ChatGPT".to_string()),
                source: None,
                enabled: true,
                healthy: true,
                membership: Some(RegisteredAccountMembership {
                    pool_id: "team-main".to_string(),
                    position: 7,
                }),
            })
            .await?;
        let runner = FakeChatgptRegistrationRunner::browser_success(provider_account_id);
        let control_plane = RuntimeBackedControlPlane::new(harness.runtime.clone());

        let result = add_chatgpt_account_with_dependencies(
            &harness.runtime,
            &harness.config,
            None,
            /*device_auth*/ false,
            &runner,
            &control_plane,
            &RuntimePendingRegistrationFinalizer,
        )
        .await?;

        assert_eq!(result.account_id, "acct-local-1");
        let registration_requests = control_plane.registration_requests();
        assert_eq!(registration_requests.len(), 1);
        assert_eq!(
            registration_requests[0].request,
            RegisteredAccountUpsert {
                account_id: chatgpt_local_account_id(provider_account_id),
                backend_id: "local".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some(provider_account_id.to_string()),
                backend_account_handle: provider_account_id.to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: provider_fingerprint(provider_account_id),
                display_name: Some("Managed ChatGPT".to_string()),
                source: None,
                enabled: true,
                healthy: true,
                membership: Some(RegisteredAccountMembership {
                    pool_id: "team-main".to_string(),
                    position: 7,
                }),
            }
        );
        assert_eq!(
            registration_requests[0]
                .pooled_registration_tokens
                .as_ref()
                .map(|tokens| tokens.account_id.as_str()),
            Some(provider_account_id)
        );
        Ok(())
    }

    #[tokio::test]
    async fn reconcile_pending_add_registration_finalizes_existing_local_account() -> Result<()> {
        let harness = RegistrationHarness::with_registered_account(
            "acct-local-1",
            "provider-acct-new",
            Some("team-main"),
        )
        .await?;
        harness
            .runtime
            .create_pending_account_registration(NewPendingAccountRegistration {
                idempotency_key: "chatgpt-add:local:provider-acct-new:team-main".to_string(),
                backend_id: "local".to_string(),
                provider_kind: "chatgpt".to_string(),
                target_pool_id: Some("team-main".to_string()),
                backend_account_handle: Some("backend-handle-1".to_string()),
                account_id: Some("acct-local-1".to_string()),
            })
            .await?;

        reconcile_pending_add_registration(
            &harness.runtime,
            "chatgpt-add:local:provider-acct-new:team-main",
            &RuntimeBackedControlPlane::new(harness.runtime.clone()),
        )
        .await?;

        let pending = harness
            .runtime
            .read_pending_account_registration("chatgpt-add:local:provider-acct-new:team-main")
            .await?
            .expect("pending row should still exist after finalization");
        assert_eq!(
            pending.backend_account_handle.as_deref(),
            Some("backend-handle-1")
        );
        assert_eq!(pending.account_id.as_deref(), Some("acct-local-1"));
        assert!(pending.completed_at.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn reconcile_pending_add_registration_clears_abandoned_local_only_row() -> Result<()> {
        let harness = RegistrationHarness::with_configured_pool("team-main").await?;
        harness
            .runtime
            .create_pending_account_registration(NewPendingAccountRegistration {
                idempotency_key: "chatgpt-add:local:provider-acct-new:team-main".to_string(),
                backend_id: "local".to_string(),
                provider_kind: "chatgpt".to_string(),
                target_pool_id: Some("team-main".to_string()),
                backend_account_handle: None,
                account_id: Some("acct-local-missing".to_string()),
            })
            .await?;

        reconcile_pending_add_registration(
            &harness.runtime,
            "chatgpt-add:local:provider-acct-new:team-main",
            &RuntimeBackedControlPlane::new(harness.runtime.clone()),
        )
        .await?;

        assert!(
            harness
                .runtime
                .read_pending_account_registration("chatgpt-add:local:provider-acct-new:team-main")
                .await?
                .is_none()
        );
        Ok(())
    }

    #[tokio::test]
    async fn reconcile_pending_add_registration_fails_closed_for_backend_only_row() -> Result<()> {
        let harness = RegistrationHarness::with_configured_pool("team-main").await?;
        harness
            .runtime
            .create_pending_account_registration(NewPendingAccountRegistration {
                idempotency_key: "chatgpt-add:local:provider-acct-new:team-main".to_string(),
                backend_id: "local".to_string(),
                provider_kind: "chatgpt".to_string(),
                target_pool_id: Some("team-main".to_string()),
                backend_account_handle: Some("backend-handle-1".to_string()),
                account_id: None,
            })
            .await?;

        let err = reconcile_pending_add_registration(
            &harness.runtime,
            "chatgpt-add:local:provider-acct-new:team-main",
            &RuntimeBackedControlPlane::new(harness.runtime.clone()),
        )
        .await
        .expect_err("backend-only rows should fail closed in phase 1");

        assert!(err.to_string().contains("manual recovery"));
        Ok(())
    }

    #[tokio::test]
    async fn add_chatgpt_registration_compensates_backend_record_when_finalize_fails() -> Result<()>
    {
        let harness = RegistrationHarness::with_configured_pool("team-main").await?;
        let runner = FakeChatgptRegistrationRunner::browser_success("provider-acct-new");
        let control_plane = RuntimeBackedControlPlane::new(harness.runtime.clone());
        let finalizer = FailingPendingFinalizer::once("finalize failed");

        let err = add_chatgpt_account_with_dependencies(
            &harness.runtime,
            &harness.config,
            None,
            /*device_auth*/ false,
            &runner,
            &control_plane,
            &finalizer,
        )
        .await
        .expect_err("finalize failure should trigger compensation");

        assert!(err.to_string().contains("finalize failed"));
        assert_eq!(
            control_plane.deleted_account_ids(),
            vec!["acct-local-70726f76696465722d616363742d6e6577".to_string()]
        );
        Ok(())
    }

    #[tokio::test]
    async fn add_chatgpt_registration_finalize_failure_keeps_existing_same_pool_account()
    -> Result<()> {
        let harness = RegistrationHarness::with_configured_pool("team-main").await?;
        let provider_account_id = "provider-acct-new";
        harness
            .runtime
            .upsert_registered_account(RegisteredAccountUpsert {
                account_id: "acct-local-1".to_string(),
                backend_id: "local".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some(provider_account_id.to_string()),
                backend_account_handle: provider_account_id.to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: provider_fingerprint(provider_account_id),
                display_name: Some("Managed ChatGPT".to_string()),
                source: None,
                enabled: true,
                healthy: true,
                membership: Some(RegisteredAccountMembership {
                    pool_id: "team-main".to_string(),
                    position: 7,
                }),
            })
            .await?;
        let runner = FakeChatgptRegistrationRunner::browser_success(provider_account_id);
        let control_plane = RuntimeBackedControlPlane::new(harness.runtime.clone());
        let finalizer = FailingPendingFinalizer::once("finalize failed");

        let err = add_chatgpt_account_with_dependencies(
            &harness.runtime,
            &harness.config,
            None,
            /*device_auth*/ false,
            &runner,
            &control_plane,
            &finalizer,
        )
        .await
        .expect_err("finalize failure should preserve existing same-pool account");

        assert!(err.to_string().contains("finalize failed"));
        assert!(control_plane.deleted_account_ids().is_empty());
        assert_eq!(
            harness
                .runtime
                .read_registered_account("acct-local-1")
                .await?
                .expect("existing account should be preserved")
                .account_id,
            "acct-local-1"
        );
        Ok(())
    }

    #[tokio::test]
    async fn add_chatgpt_registration_finalize_failure_compensates_unrelated_requested_id_collision()
    -> Result<()> {
        let harness = RegistrationHarness::with_configured_pool("team-main").await?;
        let provider_account_id = "provider-acct-new";
        let requested_account_id = chatgpt_local_account_id(provider_account_id);
        harness
            .runtime
            .upsert_registered_account(RegisteredAccountUpsert {
                account_id: requested_account_id.clone(),
                backend_id: "local".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some("provider-acct-old".to_string()),
                backend_account_handle: "provider-acct-old".to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: provider_fingerprint("provider-acct-old"),
                display_name: Some("Managed ChatGPT".to_string()),
                source: None,
                enabled: true,
                healthy: true,
                membership: None,
            })
            .await?;
        let runner = FakeChatgptRegistrationRunner::browser_success(provider_account_id);
        let control_plane = RuntimeBackedControlPlane::new(harness.runtime.clone());
        let finalizer = FailingPendingFinalizer::once("finalize failed");

        let err = add_chatgpt_account_with_dependencies(
            &harness.runtime,
            &harness.config,
            None,
            /*device_auth*/ false,
            &runner,
            &control_plane,
            &finalizer,
        )
        .await
        .expect_err("unrelated requested-id collision should still compensate");

        assert!(err.to_string().contains("finalize failed"));
        assert_eq!(
            control_plane.deleted_account_ids(),
            vec![requested_account_id.clone()]
        );
        assert_eq!(
            harness
                .runtime
                .read_registered_account(&requested_account_id)
                .await?,
            None
        );
        Ok(())
    }

    #[test]
    fn add_api_key_reports_phase_one_unsupported() {
        let err = api_key_add_is_unsupported().expect_err("api-key should stay unsupported");
        assert!(err.to_string().contains("phase 1"));
        assert!(err.to_string().contains("chatgpt"));
    }

    struct RegistrationHarness {
        _tempdir: TempDir,
        config: Config,
        runtime: Arc<StateRuntime>,
    }

    impl RegistrationHarness {
        async fn with_configured_pool(default_pool_id: &str) -> Result<Self> {
            Self::new(Some(default_pool_id)).await
        }

        async fn without_configured_pool() -> Result<Self> {
            Self::new(None).await
        }

        async fn with_registered_account(
            account_id: &str,
            provider_account_id: &str,
            pool_id: Option<&str>,
        ) -> Result<Self> {
            let harness = Self::new(Some("team-main")).await?;
            harness
                .runtime
                .upsert_registered_account(RegisteredAccountUpsert {
                    account_id: account_id.to_string(),
                    backend_id: "local".to_string(),
                    backend_family: "chatgpt".to_string(),
                    workspace_id: Some(provider_account_id.to_string()),
                    backend_account_handle: "backend-handle-1".to_string(),
                    account_kind: "chatgpt".to_string(),
                    provider_fingerprint: provider_fingerprint(provider_account_id),
                    display_name: Some("Managed ChatGPT".to_string()),
                    source: None,
                    enabled: true,
                    healthy: true,
                    membership: pool_id.map(|pool_id| RegisteredAccountMembership {
                        pool_id: pool_id.to_string(),
                        position: 0,
                    }),
                })
                .await?;
            Ok(harness)
        }

        async fn new(default_pool_id: Option<&str>) -> Result<Self> {
            let tempdir = TempDir::new()?;
            let config_toml = default_pool_id.map_or_else(String::new, |default_pool_id| {
                format!(
                    r#"
[accounts]
default_pool = "{default_pool_id}"

[accounts.pools.{default_pool_id}]
allow_context_reuse = false
"#
                )
            });
            if !config_toml.is_empty() {
                std::fs::write(tempdir.path().join("config.toml"), config_toml)?;
            }

            let config = ConfigBuilder::default()
                .codex_home(tempdir.path().to_path_buf())
                .build()
                .await?;
            let runtime = StateRuntime::init(
                tempdir.path().to_path_buf(),
                config.model_provider_id.clone(),
            )
            .await?;

            if let Some(default_pool_id) = default_pool_id {
                runtime
                    .write_account_startup_selection(AccountStartupSelectionUpdate {
                        default_pool_id: Some(default_pool_id.to_string()),
                        preferred_account_id: None,
                        suppressed: false,
                    })
                    .await?;
            }

            Ok(Self {
                _tempdir: tempdir,
                config,
                runtime,
            })
        }

        async fn membership(&self, account_id: &str) -> Result<Option<AccountPoolMembership>> {
            self.runtime.read_account_pool_membership(account_id).await
        }
    }

    #[derive(Clone)]
    struct FakeChatgptRegistrationRunner {
        browser_result: ChatgptManagedRegistrationTokens,
        device_auth_result: ChatgptManagedRegistrationTokens,
        browser_calls: Arc<Mutex<u32>>,
        device_auth_calls: Arc<Mutex<u32>>,
    }

    impl FakeChatgptRegistrationRunner {
        fn browser_success(provider_account_id: &str) -> Self {
            Self {
                browser_result: fake_tokens(provider_account_id),
                device_auth_result: fake_tokens("unused-device-auth-account"),
                browser_calls: Arc::new(Mutex::new(0)),
                device_auth_calls: Arc::new(Mutex::new(0)),
            }
        }

        fn browser_success_with_valid_id_token(provider_account_id: &str) -> Self {
            Self {
                browser_result: valid_tokens_for_real_backend(provider_account_id),
                device_auth_result: fake_tokens("unused-device-auth-account"),
                browser_calls: Arc::new(Mutex::new(0)),
                device_auth_calls: Arc::new(Mutex::new(0)),
            }
        }

        fn device_success(provider_account_id: &str) -> Self {
            Self {
                browser_result: fake_tokens("unused-browser-account"),
                device_auth_result: fake_tokens(provider_account_id),
                browser_calls: Arc::new(Mutex::new(0)),
                device_auth_calls: Arc::new(Mutex::new(0)),
            }
        }

        fn browser_calls(&self) -> u32 {
            *self
                .browser_calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }

        fn device_auth_calls(&self) -> u32 {
            *self
                .device_auth_calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    }

    impl ChatgptRegistrationRunner for FakeChatgptRegistrationRunner {
        async fn run_browser(
            &self,
            _config: &Config,
        ) -> std::io::Result<ChatgptManagedRegistrationTokens> {
            let mut calls = self
                .browser_calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *calls += 1;
            Ok(self.browser_result.clone())
        }

        async fn run_device_auth(
            &self,
            _config: &Config,
        ) -> std::io::Result<ChatgptManagedRegistrationTokens> {
            let mut calls = self
                .device_auth_calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *calls += 1;
            Ok(self.device_auth_result.clone())
        }
    }

    struct RuntimeBackedControlPlane {
        runtime: Arc<StateRuntime>,
        next_account_id: Mutex<u32>,
        deleted_account_ids: Mutex<Vec<String>>,
        registration_requests: Mutex<Vec<RegisteredAccountRegistration>>,
    }

    impl RuntimeBackedControlPlane {
        fn new(runtime: Arc<StateRuntime>) -> Self {
            Self {
                runtime,
                next_account_id: Mutex::new(1),
                deleted_account_ids: Mutex::new(Vec::new()),
                registration_requests: Mutex::new(Vec::new()),
            }
        }

        fn deleted_account_ids(&self) -> Vec<String> {
            self.deleted_account_ids
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn registration_requests(&self) -> Vec<RegisteredAccountRegistration> {
            self.registration_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl AddRegistrationControlPlane for RuntimeBackedControlPlane {
        async fn register_account(
            &self,
            mut request: RegisteredAccountRegistration,
        ) -> anyhow::Result<RegisteredAccountRecord> {
            self.registration_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request.clone());
            if request.request.account_id.is_empty() {
                let mut next_account_id = self
                    .next_account_id
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                request.request.account_id = format!("acct-local-{next_account_id}");
                *next_account_id += 1;
            }
            let resolved_account_id = self
                .runtime
                .upsert_registered_account(request.request.clone())
                .await?;
            self.runtime
                .read_registered_account(&resolved_account_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("registered account missing after fake upsert"))
        }

        async fn delete_registered_account(&self, account_id: &str) -> anyhow::Result<bool> {
            self.deleted_account_ids
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(account_id.to_string());
            self.runtime.remove_account_registry_entry(account_id).await
        }
    }

    struct FailingPendingFinalizer {
        error_message: String,
    }

    impl FailingPendingFinalizer {
        fn once(error_message: &str) -> Self {
            Self {
                error_message: error_message.to_string(),
            }
        }
    }

    impl PendingRegistrationFinalizer for FailingPendingFinalizer {
        async fn finalize_pending_account_registration(
            &self,
            _runtime: &StateRuntime,
            _idempotency_key: &str,
            _backend_account_handle: &str,
            _account_id: &str,
        ) -> anyhow::Result<()> {
            anyhow::bail!("{}", self.error_message);
        }
    }

    fn fake_tokens(provider_account_id: &str) -> ChatgptManagedRegistrationTokens {
        ChatgptManagedRegistrationTokens {
            id_token: format!("fake-id-token-{provider_account_id}"),
            access_token: format!("fake-access-token-{provider_account_id}").into(),
            refresh_token: format!("fake-refresh-token-{provider_account_id}").into(),
            account_id: provider_account_id.to_string(),
        }
    }

    fn normalized_chatgpt_backend_account_handle_for_test(provider_account_id: &str) -> String {
        let encoded_provider_account_id = provider_account_id
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        format!("chatgpt-{encoded_provider_account_id}")
    }

    fn valid_tokens_for_real_backend(
        provider_account_id: &str,
    ) -> ChatgptManagedRegistrationTokens {
        let id_token = match provider_account_id {
            "provider-acct-new" => "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJlbWFpbCI6InVzZXJAZXhhbXBsZS5jb20iLCJlbWFpbF92ZXJpZmllZCI6dHJ1ZSwiaHR0cHM6Ly9hcGkub3BlbmFpLmNvbS9hdXRoIjp7ImNoYXRncHRfcGxhbl90eXBlIjoicHJvIiwiY2hhdGdwdF91c2VyX2lkIjoidXNlci0xMjM0NSIsImNoYXRncHRfYWNjb3VudF9pZCI6InByb3ZpZGVyLWFjY3QtbmV3In19.c2ln".to_string(),
            other => panic!("missing valid ChatGPT id token fixture for provider account {other}"),
        };
        ChatgptManagedRegistrationTokens {
            id_token,
            access_token: format!("real-access-token-{provider_account_id}").into(),
            refresh_token: format!("real-refresh-token-{provider_account_id}").into(),
            account_id: provider_account_id.to_string(),
        }
    }
}
