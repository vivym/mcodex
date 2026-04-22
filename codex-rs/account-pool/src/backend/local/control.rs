use super::LocalAccountPoolBackend;
use crate::backend::AccountPoolControlPlane;
use crate::backend::RegisteredAccountRegistration;
use age::secrecy::ExposeSecret;
use anyhow::Context;
use anyhow::bail;
use async_trait::async_trait;
use chrono::Utc;
use codex_app_server_protocol::AuthMode;
use codex_login::AuthCredentialsStoreMode;
use codex_login::AuthDotJson;
use codex_login::ChatgptManagedRegistrationTokens;
use codex_login::TokenData;
use codex_login::save_auth;
use codex_login::token_data::parse_chatgpt_jwt_claims;
use codex_state::RegisteredAccountRecord;
use codex_state::RegisteredAccountUpsert;
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

struct StagedPooledRegistrationAuth {
    final_home: PathBuf,
    staging_home: PathBuf,
    backup_home: PathBuf,
    had_existing_namespace: bool,
}

#[async_trait]
impl AccountPoolControlPlane for LocalAccountPoolBackend {
    async fn register_account(
        &self,
        request: RegisteredAccountRegistration,
    ) -> anyhow::Result<RegisteredAccountRecord> {
        let RegisteredAccountRegistration {
            request,
            pooled_registration_tokens,
        } = request;
        let mut raw_backend_account_handle_to_cleanup = None;
        let request = if let Some(tokens) = pooled_registration_tokens.as_ref() {
            let normalized_backend_account_handle =
                LocalAccountPoolBackend::normalized_chatgpt_backend_account_handle(
                    tokens.account_id.as_str(),
                );
            if request.backend_account_handle != normalized_backend_account_handle {
                raw_backend_account_handle_to_cleanup =
                    Some(request.backend_account_handle.clone());
            }
            RegisteredAccountUpsert {
                backend_account_handle: normalized_backend_account_handle,
                ..request
            }
        } else {
            request
        };
        let staged_auth = if let Some(tokens) = pooled_registration_tokens.as_ref() {
            Some(
                self.stage_pooled_registration_auth(
                    request.backend_account_handle.as_str(),
                    tokens,
                )
                .await?,
            )
        } else {
            None
        };

        let resolved_account_id = match self.runtime.upsert_registered_account(request).await {
            Ok(account_id) => account_id,
            Err(err) => {
                if let Some(staged_auth) = staged_auth.as_ref() {
                    let rollback_err = self
                        .restore_pooled_registration_auth_namespace(staged_auth)
                        .await
                        .err();
                    if let Some(rollback_err) = rollback_err {
                        return Err(anyhow::anyhow!(
                            "pooled registration failed: {err}; rollback failed: {rollback_err}"
                        ));
                    }
                }
                return Err(err);
            }
        };
        let registered_account = match self
            .runtime
            .read_registered_account(&resolved_account_id)
            .await
        {
            Ok(Some(registered_account)) => registered_account,
            Ok(None) => {
                if let Some(staged_auth) = staged_auth.as_ref() {
                    let rollback_err = self
                        .restore_pooled_registration_auth_namespace(staged_auth)
                        .await
                        .err();
                    if let Some(rollback_err) = rollback_err {
                        return Err(anyhow::anyhow!(
                            "registered account missing after upsert; rollback failed: {rollback_err}"
                        ));
                    }
                }
                return Err(anyhow::anyhow!("registered account missing after upsert"));
            }
            Err(err) => {
                if let Some(staged_auth) = staged_auth.as_ref() {
                    let rollback_err = self
                        .restore_pooled_registration_auth_namespace(staged_auth)
                        .await
                        .err();
                    if let Some(rollback_err) = rollback_err {
                        return Err(anyhow::anyhow!(
                            "failed to read registered account after upsert: {err}; rollback failed: {rollback_err}"
                        ));
                    }
                }
                return Err(err);
            }
        };

        if let Some(staged_auth) = staged_auth.as_ref()
            && staged_auth.had_existing_namespace
            && let Err(err) = self
                .remove_backend_private_auth_namespace_path(staged_auth.backup_home.as_path())
                .await
        {
            eprintln!(
                "failed to remove backup backend-private auth after successful registration for {}: {err}",
                registered_account.backend_account_handle
            );
        }
        if let Some(raw_backend_account_handle) = raw_backend_account_handle_to_cleanup.as_deref()
            && !raw_backend_account_handle.is_empty()
            && Path::new(raw_backend_account_handle)
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
            && let Err(err) = self
                .clear_backend_private_auth_namespace(raw_backend_account_handle)
                .await
        {
            eprintln!(
                "failed to remove legacy raw backend-private auth after successful registration for {} raw handle {}: {err}",
                registered_account.account_id, raw_backend_account_handle
            );
        }

        Ok(registered_account)
    }

    async fn delete_registered_account(&self, account_id: &str) -> anyhow::Result<bool> {
        let Some(registered_account) = self.runtime.read_registered_account(account_id).await?
        else {
            return Ok(false);
        };
        self.clear_backend_private_auth_namespace(&registered_account.backend_account_handle)
            .await
            .with_context(|| {
                format!(
                    "failed to delete backend-private auth for account {} handle {}",
                    registered_account.account_id, registered_account.backend_account_handle
                )
            })?;
        self.runtime
            .remove_account_registry_entry(account_id)
            .await
            .with_context(|| {
                format!(
                    "backend-private auth was deleted for account {} handle {}; local registry cleanup failed",
                    registered_account.account_id, registered_account.backend_account_handle
                )
            })
    }
}

impl LocalAccountPoolBackend {
    async fn stage_pooled_registration_auth(
        &self,
        backend_account_handle: &str,
        tokens: &ChatgptManagedRegistrationTokens,
    ) -> anyhow::Result<StagedPooledRegistrationAuth> {
        let auth = self.build_pooled_registration_auth(tokens)?;
        let registration_suffix = format!(
            "{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let staged_auth = StagedPooledRegistrationAuth {
            final_home: self.backend_private_auth_home(backend_account_handle),
            staging_home: self
                .backend_private_auth_staging_home(backend_account_handle, &registration_suffix),
            backup_home: self
                .backend_private_auth_backup_home(backend_account_handle, &registration_suffix),
            had_existing_namespace: false,
        };

        self.remove_backend_private_auth_namespace_path(staged_auth.staging_home.as_path())
            .await?;
        self.remove_backend_private_auth_namespace_path(staged_auth.backup_home.as_path())
            .await?;
        save_auth(
            &staged_auth.staging_home,
            &auth,
            AuthCredentialsStoreMode::File,
        )?;

        let had_existing_namespace = match fs::symlink_metadata(&staged_auth.final_home) {
            Ok(_) => true,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(err) => return Err(err.into()),
        };
        let staged_auth = StagedPooledRegistrationAuth {
            had_existing_namespace,
            ..staged_auth
        };
        if staged_auth.had_existing_namespace {
            self.move_backend_private_auth_namespace(
                staged_auth.final_home.as_path(),
                staged_auth.backup_home.as_path(),
            )
            .await
            .with_context(|| {
                format!(
                    "failed to back up existing backend-private auth for {backend_account_handle}"
                )
            })?;
        }

        if let Err(err) = self
            .move_backend_private_auth_namespace(
                staged_auth.staging_home.as_path(),
                staged_auth.final_home.as_path(),
            )
            .await
        {
            let rollback_err = self
                .restore_pooled_registration_auth_namespace(&staged_auth)
                .await
                .err();
            if let Some(rollback_err) = rollback_err {
                return Err(anyhow::anyhow!(
                    "failed to install staged backend-private auth for {backend_account_handle}: {err}; rollback failed: {rollback_err}"
                ));
            }
            return Err(err).with_context(|| {
                format!(
                    "failed to install staged backend-private auth for {backend_account_handle}"
                )
            });
        }

        Ok(staged_auth)
    }

    async fn restore_pooled_registration_auth_namespace(
        &self,
        staged: &StagedPooledRegistrationAuth,
    ) -> anyhow::Result<()> {
        let mut errors = Vec::new();
        if staged.had_existing_namespace {
            if let Err(err) = self
                .remove_backend_private_auth_namespace_path(staged.final_home.as_path())
                .await
            {
                errors.push(err);
            }
            if let Err(err) = self
                .move_backend_private_auth_namespace(
                    staged.backup_home.as_path(),
                    staged.final_home.as_path(),
                )
                .await
            {
                errors.push(err);
            }
        } else if let Err(err) = self
            .remove_backend_private_auth_namespace_path(staged.final_home.as_path())
            .await
        {
            errors.push(err);
        }

        if let Err(err) = self
            .remove_backend_private_auth_namespace_path(staged.staging_home.as_path())
            .await
        {
            errors.push(err);
        }

        match errors.len() {
            0 => Ok(()),
            1 => Err(errors.remove(0)),
            _ => Err(anyhow::anyhow!(
                "pooled registration rollback failed: {}",
                errors
                    .into_iter()
                    .map(|err| err.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )),
        }
    }

    fn build_pooled_registration_auth(
        &self,
        tokens: &ChatgptManagedRegistrationTokens,
    ) -> anyhow::Result<AuthDotJson> {
        let id_token = parse_chatgpt_jwt_claims(&tokens.id_token)?;
        let Some(token_account_id) = id_token.chatgpt_account_id.clone() else {
            bail!("pooled registration id token is missing a chatgpt_account_id");
        };
        if token_account_id != tokens.account_id {
            bail!(
                "pooled registration account id mismatch between token claims and extracted account id"
            );
        }

        Ok(AuthDotJson {
            auth_mode: Some(AuthMode::Chatgpt),
            openai_api_key: None,
            tokens: Some(TokenData {
                id_token,
                access_token: tokens.access_token.expose_secret().to_string(),
                refresh_token: tokens.refresh_token.expose_secret().to_string(),
                account_id: Some(tokens.account_id.clone()),
            }),
            last_refresh: Some(Utc::now()),
            agent_identity: None,
        })
    }
}
