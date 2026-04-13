use super::LocalAccountPoolBackend;
use crate::backend::AccountPoolControlPlane;
use anyhow::Context;
use async_trait::async_trait;
use codex_state::RegisteredAccountRecord;
use codex_state::RegisteredAccountUpsert;

#[async_trait]
impl AccountPoolControlPlane for LocalAccountPoolBackend {
    async fn register_account(
        &self,
        request: RegisteredAccountUpsert,
    ) -> anyhow::Result<RegisteredAccountRecord> {
        let account_id = self.runtime.upsert_registered_account(request).await?;
        self.runtime
            .read_registered_account(&account_id)
            .await?
            .context("registered account missing after upsert")
    }

    async fn delete_registered_account(&self, account_id: &str) -> anyhow::Result<bool> {
        self.runtime.remove_account_registry_entry(account_id).await
    }
}
