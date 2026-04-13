use super::LocalAccountPoolBackend;
use crate::backend::AccountPoolControlPlane;
use age::secrecy::ExposeSecret;
use anyhow::Context;
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

#[async_trait]
impl AccountPoolControlPlane for LocalAccountPoolBackend {
    async fn register_account(
        &self,
        request: RegisteredAccountUpsert,
        pooled_registration_tokens: Option<ChatgptManagedRegistrationTokens>,
    ) -> anyhow::Result<RegisteredAccountRecord> {
        let account_id = self.runtime.upsert_registered_account(request).await?;
        let registered_account = self
            .runtime
            .read_registered_account(&account_id)
            .await?
            .context("registered account missing after upsert")?;

        if let Some(tokens) = pooled_registration_tokens.as_ref() {
            self.persist_pooled_registration_tokens(
                registered_account.backend_account_handle.as_str(),
                tokens,
            )
            .await?;
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
        let auth = AuthDotJson {
            auth_mode: Some(AuthMode::Chatgpt),
            openai_api_key: None,
            tokens: Some(TokenData {
                id_token: parse_chatgpt_jwt_claims(&tokens.id_token)?,
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
