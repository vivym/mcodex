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
        let requested_account_id = request.account_id.clone();
        let requested_account_existed = self
            .runtime
            .read_registered_account(requested_account_id.as_str())
            .await?
            .is_some();

        let resolved_account_id = self.runtime.upsert_registered_account(request).await?;
        let registered_account = self
            .runtime
            .read_registered_account(&resolved_account_id)
            .await?
            .context("registered account missing after upsert")?;

        let created_new_account_row =
            !requested_account_existed && resolved_account_id == requested_account_id;
        if let Some(tokens) = pooled_registration_tokens.as_ref()
            && let Err(err) = self
                .persist_pooled_registration_tokens(
                    registered_account.backend_account_handle.as_str(),
                    tokens,
                )
                .await
        {
            if created_new_account_row {
                if let Err(cleanup_err) = self
                    .runtime
                    .remove_account_registry_entry(registered_account.account_id.as_str())
                    .await
                {
                    eprintln!(
                        "failed to remove registry row after pooled registration failure for {}: {cleanup_err}",
                        registered_account.account_id
                    );
                }
                if let Err(cleanup_err) = self
                    .clear_backend_private_auth_namespace(
                        registered_account.backend_account_handle.as_str(),
                    )
                    .await
                {
                    eprintln!(
                        "failed to clear backend-private auth after pooled registration failure for {}: {cleanup_err}",
                        registered_account.backend_account_handle
                    );
                }
            }
            return Err(err);
        }

        Ok(registered_account)
    }

    async fn delete_registered_account(&self, account_id: &str) -> anyhow::Result<bool> {
        self.runtime.remove_account_registry_entry(account_id).await
    }
}

impl LocalAccountPoolBackend {
    pub(crate) async fn persist_pooled_registration_tokens(
        &self,
        backend_account_handle: &str,
        tokens: &ChatgptManagedRegistrationTokens,
    ) -> anyhow::Result<()> {
        let auth_home = self.backend_private_auth_home(backend_account_handle);
        let id_token = parse_chatgpt_jwt_claims(&tokens.id_token)?;
        let Some(token_account_id) = id_token.chatgpt_account_id.clone() else {
            bail!("pooled registration id token is missing a chatgpt_account_id");
        };
        if token_account_id != tokens.account_id {
            bail!(
                "pooled registration account id mismatch between token claims and extracted account id"
            );
        }
        let auth = AuthDotJson {
            auth_mode: Some(AuthMode::Chatgpt),
            openai_api_key: None,
            tokens: Some(TokenData {
                id_token,
                access_token: tokens.access_token.expose_secret().to_string(),
                refresh_token: tokens.refresh_token.expose_secret().to_string(),
                account_id: Some(tokens.account_id.clone()),
            }),
            last_refresh: Some(Utc::now()),
        };
        save_auth(&auth_home, &auth, AuthCredentialsStoreMode::File)?;
        Ok(())
    }
}
