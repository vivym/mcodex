mod control;
mod execution;
mod observability;

use crate::build_selection_plan;
use crate::quota::QuotaFamilyView;
use crate::types::AccountKind;
use crate::types::AccountRecord;
use crate::types::SelectionRequest;
use chrono::Duration;
use codex_login::auth::LocalLeaseScopedAuthSession;
use codex_state::AccountHealthState;
use codex_state::AccountQuotaStateRecord;
use codex_state::RegisteredAccountRecord;
use codex_state::StateRuntime;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

/// Local backend backed by `codex-state` SQLite persistence.
#[derive(Clone)]
pub struct LocalAccountPoolBackend {
    runtime: Arc<StateRuntime>,
    lease_ttl: Duration,
}

impl LocalAccountPoolBackend {
    pub fn new(runtime: Arc<StateRuntime>, lease_ttl: Duration) -> Self {
        Self { runtime, lease_ttl }
    }

    pub(crate) async fn plan_runtime_selection(
        &self,
        request: &SelectionRequest,
        holder_instance_id: &str,
    ) -> anyhow::Result<(String, crate::SelectionPlan)> {
        let pool_id = request
            .pool_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("runtime selection requires pool_id"))?;
        let selection_family = self.resolve_selection_family(request).await?;
        let mut selector = build_selection_plan(SelectionRequest {
            selection_family: Some(selection_family.clone()),
            ..request.clone()
        });
        for candidate in self
            .load_runtime_selection_candidates(pool_id, holder_instance_id, &selection_family)
            .await?
        {
            selector = selector.with_candidate(candidate);
        }

        Ok((selection_family, selector.run()))
    }

    pub(crate) fn normalized_chatgpt_backend_account_handle(provider_account_id: &str) -> String {
        let encoded_provider_account_id = provider_account_id
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        format!("chatgpt-{encoded_provider_account_id}")
    }

    pub(crate) fn backend_private_auth_home(&self, backend_account_handle: &str) -> PathBuf {
        self.runtime
            .codex_home()
            .join(".pooled-auth/backends/local/accounts")
            .join(backend_account_handle)
    }

    pub(crate) fn backend_private_auth_staging_home(
        &self,
        backend_account_handle: &str,
        registration_suffix: &str,
    ) -> PathBuf {
        self.backend_private_auth_home(backend_account_handle)
            .with_file_name(format!(
                "{backend_account_handle}.staging-{registration_suffix}"
            ))
    }

    pub(crate) fn backend_private_auth_backup_home(
        &self,
        backend_account_handle: &str,
        registration_suffix: &str,
    ) -> PathBuf {
        self.backend_private_auth_home(backend_account_handle)
            .with_file_name(format!(
                "{backend_account_handle}.backup-{registration_suffix}"
            ))
    }

    pub(crate) async fn write_backend_private_lease_epoch(
        &self,
        backend_account_handle: &str,
        lease_epoch: u64,
    ) -> anyhow::Result<()> {
        let auth_home = self.backend_private_auth_home(backend_account_handle);
        LocalLeaseScopedAuthSession::write_lease_epoch_marker(auth_home.as_path(), lease_epoch)
    }

    pub(crate) async fn clear_backend_private_lease_epoch(
        &self,
        backend_account_handle: &str,
    ) -> anyhow::Result<()> {
        let auth_home = self.backend_private_auth_home(backend_account_handle);
        LocalLeaseScopedAuthSession::clear_lease_epoch_marker(auth_home.as_path())
    }

    pub(crate) async fn clear_backend_private_auth_namespace(
        &self,
        backend_account_handle: &str,
    ) -> anyhow::Result<()> {
        let auth_home = self.backend_private_auth_home(backend_account_handle);
        self.remove_backend_private_auth_namespace_path(auth_home.as_path())
            .await?;
        Ok(())
    }

    pub(crate) async fn remove_backend_private_auth_namespace_path(
        &self,
        auth_home: &Path,
    ) -> anyhow::Result<()> {
        match fs::symlink_metadata(auth_home) {
            Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(auth_home)?,
            Ok(_) => fs::remove_file(auth_home)?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        Ok(())
    }

    pub(crate) async fn move_backend_private_auth_namespace(
        &self,
        from: &Path,
        to: &Path,
    ) -> anyhow::Result<()> {
        fs::rename(from, to)?;
        Ok(())
    }

    async fn resolve_selection_family(&self, request: &SelectionRequest) -> anyhow::Result<String> {
        if let Some(selection_family) = request.selection_family.clone() {
            return Ok(selection_family);
        }
        if matches!(request.intent, crate::SelectionIntent::Startup) {
            return Ok("codex".to_string());
        }
        if let Some(current_account_id) = request.current_account_id.as_deref()
            && let Some(registered_account) = self
                .runtime
                .read_registered_account(current_account_id)
                .await?
        {
            return Ok(normalized_selection_family(
                registered_account.backend_family.as_str(),
            ));
        }

        Ok("codex".to_string())
    }

    async fn load_runtime_selection_candidates(
        &self,
        pool_id: &str,
        holder_instance_id: &str,
        selection_family: &str,
    ) -> anyhow::Result<Vec<AccountRecord>> {
        let rows = self
            .runtime
            .read_account_lease_selection_candidates(pool_id)
            .await?;
        let mut candidates = Vec::with_capacity(rows.len());
        for (registered_account, health_state, active_lease, position) in rows {
            let selection_quota = self
                .read_candidate_quota(registered_account.account_id.as_str(), selection_family)
                .await?;
            let codex_fallback = if selection_family == "codex" || selection_quota.is_some() {
                None
            } else {
                self.runtime
                    .read_account_quota_state(registered_account.account_id.as_str(), "codex")
                    .await?
            };
            candidates.push(AccountRecord {
                account_id: registered_account.account_id.clone(),
                healthy: registered_account.healthy,
                kind: account_kind(&registered_account),
                enabled: registered_account.enabled,
                selector_auth_eligible: !matches!(
                    health_state,
                    Some(AccountHealthState::Unauthorized)
                ),
                pool_position: usize::try_from(position).unwrap_or_default(),
                leased_to_other_holder: active_lease
                    .as_ref()
                    .is_some_and(|lease| lease.holder_instance_id != holder_instance_id),
                quota: QuotaFamilyView {
                    selection: selection_quota,
                    codex_fallback,
                },
            });
        }

        Ok(candidates)
    }

    async fn read_candidate_quota(
        &self,
        account_id: &str,
        selection_family: &str,
    ) -> anyhow::Result<Option<AccountQuotaStateRecord>> {
        self.runtime
            .read_account_quota_state(account_id, selection_family)
            .await
    }
}

fn account_kind(registered_account: &RegisteredAccountRecord) -> AccountKind {
    if registered_account.account_kind == "chatgpt" {
        AccountKind::ChatGpt
    } else {
        AccountKind::ManualOnly
    }
}

fn normalized_selection_family(selection_family: &str) -> String {
    if selection_family.is_empty() {
        "codex".to_string()
    } else {
        selection_family.to_string()
    }
}
