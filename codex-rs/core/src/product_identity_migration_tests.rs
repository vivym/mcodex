use super::*;
use codex_config::config_toml::ConfigToml;
use codex_config::types::AccountAllocationModeToml;
use pretty_assertions::assert_eq;
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use tempfile::TempDir;

#[derive(Debug, Default)]
struct StubUi {
    should_migrate: bool,
    prompt_count: usize,
}

impl StubUi {
    fn accepting() -> Self {
        Self {
            should_migrate: true,
            prompt_count: 0,
        }
    }

    fn declining() -> Self {
        Self {
            should_migrate: false,
            prompt_count: 0,
        }
    }
}

impl ProductIdentityMigrationUi for StubUi {
    fn should_migrate_product_identity(
        &mut self,
        _legacy_home: &Path,
        _mcodex_home: &Path,
    ) -> io::Result<bool> {
        self.prompt_count += 1;
        Ok(self.should_migrate)
    }
}

fn write_legacy_config(legacy_home: &Path) -> io::Result<()> {
    fs::write(
        legacy_home.join("config.toml"),
        r#"
[accounts]
default_pool = "legacy-default"
allocation_mode = "exclusive"

[accounts.pools.legacy-default]
allow_context_reuse = true
"#,
    )
}

fn write_legacy_auth(legacy_home: &Path) -> io::Result<()> {
    fs::write(
        legacy_home.join("auth.json"),
        r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":"sk-legacy"}"#,
    )
}

fn read_config(path: &Path) -> io::Result<ConfigToml> {
    let contents = fs::read_to_string(path.join("config.toml"))?;
    toml::from_str(&contents).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn some_legacy_home(legacy_home: &TempDir) -> io::Result<Option<PathBuf>> {
    Ok(Some(legacy_home.path().to_path_buf()))
}

#[tokio::test]
async fn config_transform_drops_startup_selection_fields() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_config(legacy_home.path())?;

    let mut ui = StubUi::accepting();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        some_legacy_home(&legacy_home),
        &mut ui,
    )
    .await?;

    assert_eq!(outcome.status, ProductIdentityMigrationStatus::Imported);
    assert_eq!(outcome.config_import, MigrationImportOutcome::Imported);
    let migrated = read_config(active_home.path())?;
    let accounts = migrated.accounts.expect("accounts should be preserved");
    assert_eq!(accounts.default_pool, None);
    assert_eq!(
        accounts.allocation_mode,
        Some(AccountAllocationModeToml::Exclusive)
    );
    Ok(())
}

#[tokio::test]
async fn auth_import_failure_records_warning_without_blocking_migration() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_config(legacy_home.path())?;
    write_legacy_auth(legacy_home.path())?;
    fs::create_dir_all(active_home.path().join("auth.json"))?;

    let mut ui = StubUi::accepting();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        some_legacy_home(&legacy_home),
        &mut ui,
    )
    .await?;

    assert_eq!(
        outcome.status,
        ProductIdentityMigrationStatus::ImportedWithWarnings
    );
    assert_eq!(outcome.config_import, MigrationImportOutcome::Imported);
    match outcome.auth_import {
        MigrationImportOutcome::Failed { warning } => {
            assert!(
                warning.contains("auth.json"),
                "unexpected warning: {warning}"
            );
        }
        other => panic!("unexpected auth import outcome: {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn does_not_import_legacy_skills_or_plugin_cache_by_default() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_config(legacy_home.path())?;
    write_legacy_auth(legacy_home.path())?;
    fs::create_dir_all(legacy_home.path().join("skills/sample"))?;
    fs::create_dir_all(legacy_home.path().join("plugins/cache"))?;
    fs::create_dir_all(legacy_home.path().join("marketplace/cache"))?;
    fs::create_dir_all(legacy_home.path().join("logs"))?;
    fs::create_dir_all(legacy_home.path().join("sessions/2025/01/01"))?;
    fs::write(legacy_home.path().join("state.db"), "sqlite")?;

    let mut ui = StubUi::accepting();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        some_legacy_home(&legacy_home),
        &mut ui,
    )
    .await?;

    assert_eq!(outcome.status, ProductIdentityMigrationStatus::Imported);
    assert!(active_home.path().join("config.toml").exists());
    assert!(active_home.path().join("auth.json").exists());
    assert!(!active_home.path().join("skills").exists());
    assert!(!active_home.path().join("plugins").exists());
    assert!(!active_home.path().join("marketplace").exists());
    assert!(!active_home.path().join("logs").exists());
    assert!(!active_home.path().join("sessions").exists());
    assert!(!active_home.path().join("state.db").exists());
    Ok(())
}

#[tokio::test]
async fn skips_when_marker_exists() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    fs::write(
        active_home.path().join(PRODUCT_IDENTITY_MIGRATION_FILENAME),
        "v1\n",
    )?;
    write_legacy_config(legacy_home.path())?;
    write_legacy_auth(legacy_home.path())?;

    let mut ui = StubUi::accepting();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        some_legacy_home(&legacy_home),
        &mut ui,
    )
    .await?;

    assert_eq!(ui.prompt_count, 0);
    assert_eq!(
        outcome.status,
        ProductIdentityMigrationStatus::SkippedMarker
    );
    assert_eq!(outcome.config_import, MigrationImportOutcome::NotAttempted);
    assert_eq!(outcome.auth_import, MigrationImportOutcome::NotAttempted);
    Ok(())
}

#[tokio::test]
async fn skips_when_no_legacy_home() -> io::Result<()> {
    let active_home = TempDir::new()?;

    let mut ui = StubUi::accepting();
    let outcome =
        maybe_migrate_product_identity_with_legacy_home(active_home.path(), Ok(None), &mut ui)
            .await?;

    assert_eq!(ui.prompt_count, 0);
    assert_eq!(
        outcome.status,
        ProductIdentityMigrationStatus::SkippedNoLegacyHome
    );
    assert_eq!(outcome.config_import, MigrationImportOutcome::NotAttempted);
    assert_eq!(outcome.auth_import, MigrationImportOutcome::NotAttempted);
    Ok(())
}

#[tokio::test]
async fn skips_unreadable_legacy_home_without_prompting() -> io::Result<()> {
    let active_home = TempDir::new()?;

    let mut ui = StubUi::accepting();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        Err(io::Error::other("legacy home is unreadable")),
        &mut ui,
    )
    .await?;

    assert_eq!(ui.prompt_count, 0);
    assert_eq!(
        outcome.status,
        ProductIdentityMigrationStatus::SkippedUnreadableLegacyHome
    );
    assert_eq!(outcome.config_import, MigrationImportOutcome::NotAttempted);
    assert_eq!(outcome.auth_import, MigrationImportOutcome::NotAttempted);
    Ok(())
}

#[tokio::test]
async fn skips_when_user_declines_migration() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_config(legacy_home.path())?;
    write_legacy_auth(legacy_home.path())?;

    let mut ui = StubUi::declining();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        some_legacy_home(&legacy_home),
        &mut ui,
    )
    .await?;

    assert_eq!(ui.prompt_count, 1);
    assert_eq!(
        outcome.status,
        ProductIdentityMigrationStatus::SkippedByUser
    );
    assert_eq!(outcome.config_import, MigrationImportOutcome::NotAttempted);
    assert_eq!(outcome.auth_import, MigrationImportOutcome::NotAttempted);
    assert!(
        active_home
            .path()
            .join(PRODUCT_IDENTITY_MIGRATION_FILENAME)
            .exists()
    );
    assert!(!active_home.path().join("config.toml").exists());
    assert!(!active_home.path().join("auth.json").exists());
    Ok(())
}

#[tokio::test]
async fn marker_write_failure_is_reported_as_warning() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_config(legacy_home.path())?;
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(active_home.path())?.permissions();
        permissions.set_mode(0o555);
        fs::set_permissions(active_home.path(), permissions)?;
    }

    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(active_home.path())?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(active_home.path(), permissions)?;
    }

    let mut ui = StubUi::declining();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        some_legacy_home(&legacy_home),
        &mut ui,
    )
    .await?;

    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(active_home.path())?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(active_home.path(), permissions)?;
    }

    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(active_home.path())?.permissions();
        permissions.set_readonly(false);
        fs::set_permissions(active_home.path(), permissions)?;
    }

    assert_eq!(
        outcome.status,
        ProductIdentityMigrationStatus::SkippedByUser
    );
    assert!(outcome.marker_warning.is_some());
    Ok(())
}

#[tokio::test]
async fn imports_config_and_auth_when_user_accepts() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_config(legacy_home.path())?;
    write_legacy_auth(legacy_home.path())?;

    let mut ui = StubUi::accepting();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        some_legacy_home(&legacy_home),
        &mut ui,
    )
    .await?;

    assert_eq!(ui.prompt_count, 1);
    assert_eq!(outcome.status, ProductIdentityMigrationStatus::Imported);
    assert_eq!(outcome.config_import, MigrationImportOutcome::Imported);
    assert_eq!(outcome.auth_import, MigrationImportOutcome::Imported);
    assert!(outcome.marker_warning.is_none());
    assert!(active_home.path().join("config.toml").exists());
    assert!(active_home.path().join("auth.json").exists());
    assert!(
        active_home
            .path()
            .join(PRODUCT_IDENTITY_MIGRATION_FILENAME)
            .exists()
    );
    Ok(())
}
