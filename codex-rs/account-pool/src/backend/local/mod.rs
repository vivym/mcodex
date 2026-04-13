mod control;
mod execution;

use chrono::Duration;
use codex_login::auth::LocalLeaseScopedAuthSession;
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
}
