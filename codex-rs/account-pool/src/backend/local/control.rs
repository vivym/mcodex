use super::LocalAccountPoolBackend;
use crate::backend::AccountPoolControlPlane;
use crate::backend::RegisteredAccountRegistration;
use age::secrecy::ExposeSecret;
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
use std::future::Future;

#[async_trait]
impl AccountPoolControlPlane for LocalAccountPoolBackend {
    async fn register_account(
        &self,
        request: RegisteredAccountRegistration,
    ) -> anyhow::Result<RegisteredAccountRecord> {
        self.register_account_with_upsert(request, |request| async move {
            self.runtime.upsert_registered_account(request).await
        })
        .await
    }

    async fn delete_registered_account(&self, account_id: &str) -> anyhow::Result<bool> {
        self.runtime.remove_account_registry_entry(account_id).await
    }
}

impl LocalAccountPoolBackend {
    pub(crate) async fn register_account_with_upsert<F, Fut>(
        &self,
        request: RegisteredAccountRegistration,
        upsert_registered_account: F,
    ) -> anyhow::Result<RegisteredAccountRecord>
    where
        F: FnOnce(RegisteredAccountUpsert) -> Fut,
        Fut: Future<Output = anyhow::Result<String>>,
    {
        let RegisteredAccountRegistration {
            request,
            pooled_registration_tokens,
        } = request;
        let request_for_record = request.clone();
        if let Some(tokens) = pooled_registration_tokens.as_ref() {
            self.persist_pooled_registration_tokens(&request.backend_account_handle, tokens)
                .await?;
        }

        let account_id = match upsert_registered_account(request).await {
            Ok(account_id) => account_id,
            Err(err) => {
                if pooled_registration_tokens.is_some()
                    && let Err(cleanup_err) = self
                        .clear_backend_private_auth_namespace(
                            &request_for_record.backend_account_handle,
                        )
                        .await
                {
                    eprintln!(
                        "failed to clean up backend-private pooled auth after registration failure for {}: {cleanup_err}",
                        request_for_record.backend_account_handle
                    );
                }
                return Err(err);
            }
        };

        Ok(RegisteredAccountRecord {
            account_id,
            backend_id: request_for_record.backend_id,
            backend_family: request_for_record.backend_family,
            workspace_id: request_for_record.workspace_id,
            backend_account_handle: request_for_record.backend_account_handle,
            account_kind: request_for_record.account_kind,
            provider_fingerprint: request_for_record.provider_fingerprint,
            display_name: request_for_record.display_name,
            source: request_for_record.source,
            enabled: request_for_record.enabled,
            healthy: request_for_record.healthy,
        })
    }

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
