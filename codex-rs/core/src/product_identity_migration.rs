use crate::personality_migration::PersonalityMigrationStatus;
use crate::personality_migration::maybe_migrate_personality;
use codex_config::CONFIG_TOML_FILE;
use codex_config::config_toml::ConfigToml;
use codex_product_identity::MCODEX;
use codex_utils_home_dir::find_legacy_codex_home_for_migration;
use std::future::Future;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use tokio::fs;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use toml_edit::DocumentMut;
use toml_edit::Item as TomlItem;

pub const PRODUCT_IDENTITY_MIGRATION_FILENAME: &str = ".product_identity_migration";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationImportOutcome {
    NotAttempted,
    Imported,
    Failed { warning: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductIdentityMigrationStatus {
    SkippedMarker,
    SkippedNoLegacyHome,
    SkippedUnreadableLegacyHome,
    SkippedByUser,
    Imported,
    ImportedWithWarnings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductIdentityMigrationOutcome {
    pub status: ProductIdentityMigrationStatus,
    pub config_import: MigrationImportOutcome,
    pub auth_import: MigrationImportOutcome,
    pub marker_warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupMigrationOutcome {
    pub product_identity: ProductIdentityMigrationStatus,
    pub personality: PersonalityMigrationStatus,
}

/// Prompts the caller about whether legacy Codex home data should be imported.
pub trait ProductIdentityMigrationUi {
    /// Returns `true` when supported legacy files should be imported.
    fn should_migrate_product_identity(
        &mut self,
        legacy_home: &Path,
        mcodex_home: &Path,
    ) -> io::Result<bool>;
}

pub async fn maybe_migrate_product_identity(
    mcodex_home: &Path,
    ui: &mut dyn ProductIdentityMigrationUi,
) -> io::Result<ProductIdentityMigrationOutcome> {
    maybe_migrate_product_identity_with_legacy_home(mcodex_home, legacy_home_from_environment(), ui)
        .await
}

pub async fn run_startup_migrations<LoadConfig, LoadConfigFuture>(
    codex_home: &Path,
    ui: &mut dyn ProductIdentityMigrationUi,
    load_config_toml: LoadConfig,
) -> io::Result<StartupMigrationOutcome>
where
    LoadConfig: FnOnce() -> LoadConfigFuture,
    LoadConfigFuture: Future<Output = io::Result<ConfigToml>>,
{
    run_startup_migrations_with_loader_and_legacy_home(
        codex_home,
        legacy_home_from_environment(),
        ui,
        load_config_toml,
    )
    .await
}

fn legacy_home_from_environment() -> io::Result<Option<PathBuf>> {
    let legacy_home_env = std::env::var(MCODEX.legacy_home_env_var).ok();
    find_legacy_codex_home_for_migration(legacy_home_env.as_deref()).map(|legacy_home| {
        legacy_home.map(codex_utils_absolute_path::AbsolutePathBuf::into_path_buf)
    })
}

#[cfg(test)]
async fn run_startup_migrations_with_legacy_home(
    codex_home: &Path,
    config_toml: &ConfigToml,
    legacy_home_result: io::Result<Option<PathBuf>>,
    ui: &mut dyn ProductIdentityMigrationUi,
) -> io::Result<StartupMigrationOutcome> {
    run_startup_migrations_with_loader_and_legacy_home(codex_home, legacy_home_result, ui, || {
        let config_toml = config_toml.clone();
        async move { Ok(config_toml) }
    })
    .await
}

async fn run_startup_migrations_with_loader_and_legacy_home<LoadConfig, LoadConfigFuture>(
    codex_home: &Path,
    legacy_home_result: io::Result<Option<PathBuf>>,
    ui: &mut dyn ProductIdentityMigrationUi,
    load_config_toml: LoadConfig,
) -> io::Result<StartupMigrationOutcome>
where
    LoadConfig: FnOnce() -> LoadConfigFuture,
    LoadConfigFuture: Future<Output = io::Result<ConfigToml>>,
{
    let product_identity =
        maybe_migrate_product_identity_with_legacy_home(codex_home, legacy_home_result, ui).await?;
    let config_toml = load_config_toml().await?;
    let personality = maybe_migrate_personality(codex_home, &config_toml).await?;

    Ok(StartupMigrationOutcome {
        product_identity: product_identity.status,
        personality,
    })
}

async fn maybe_migrate_product_identity_with_legacy_home(
    mcodex_home: &Path,
    legacy_home_result: io::Result<Option<PathBuf>>,
    ui: &mut dyn ProductIdentityMigrationUi,
) -> io::Result<ProductIdentityMigrationOutcome> {
    let marker_path = mcodex_home.join(PRODUCT_IDENTITY_MIGRATION_FILENAME);
    if fs::try_exists(&marker_path).await? {
        return Ok(ProductIdentityMigrationOutcome {
            status: ProductIdentityMigrationStatus::SkippedMarker,
            config_import: MigrationImportOutcome::NotAttempted,
            auth_import: MigrationImportOutcome::NotAttempted,
            marker_warning: None,
        });
    }

    let legacy_home = match legacy_home_result {
        Ok(Some(legacy_home)) => legacy_home,
        Ok(None) => {
            return Ok(ProductIdentityMigrationOutcome {
                status: ProductIdentityMigrationStatus::SkippedNoLegacyHome,
                config_import: MigrationImportOutcome::NotAttempted,
                auth_import: MigrationImportOutcome::NotAttempted,
                marker_warning: None,
            });
        }
        Err(_) => {
            return Ok(ProductIdentityMigrationOutcome {
                status: ProductIdentityMigrationStatus::SkippedUnreadableLegacyHome,
                config_import: MigrationImportOutcome::NotAttempted,
                auth_import: MigrationImportOutcome::NotAttempted,
                marker_warning: None,
            });
        }
    };

    if legacy_home == mcodex_home {
        return Ok(ProductIdentityMigrationOutcome {
            status: ProductIdentityMigrationStatus::SkippedNoLegacyHome,
            config_import: MigrationImportOutcome::NotAttempted,
            auth_import: MigrationImportOutcome::NotAttempted,
            marker_warning: None,
        });
    }

    if !ui.should_migrate_product_identity(&legacy_home, mcodex_home)? {
        return Ok(ProductIdentityMigrationOutcome {
            status: ProductIdentityMigrationStatus::SkippedByUser,
            config_import: MigrationImportOutcome::NotAttempted,
            auth_import: MigrationImportOutcome::NotAttempted,
            marker_warning: write_marker_warning(mcodex_home, &marker_path).await,
        });
    }

    let config_import = import_config(&legacy_home, mcodex_home).await;
    let auth_import = import_auth(&legacy_home, mcodex_home).await;
    let marker_warning = write_marker_warning(mcodex_home, &marker_path).await;
    let has_warnings = matches!(config_import, MigrationImportOutcome::Failed { .. })
        || matches!(auth_import, MigrationImportOutcome::Failed { .. })
        || marker_warning.is_some();

    Ok(ProductIdentityMigrationOutcome {
        status: if has_warnings {
            ProductIdentityMigrationStatus::ImportedWithWarnings
        } else {
            ProductIdentityMigrationStatus::Imported
        },
        config_import,
        auth_import,
        marker_warning,
    })
}

async fn import_config(legacy_home: &Path, mcodex_home: &Path) -> MigrationImportOutcome {
    let legacy_config_path = legacy_home.join(CONFIG_TOML_FILE);
    let config_contents = match fs::read_to_string(&legacy_config_path).await {
        Ok(config_contents) => config_contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return MigrationImportOutcome::NotAttempted;
        }
        Err(err) => {
            return MigrationImportOutcome::Failed {
                warning: format!(
                    "failed to read legacy config.toml at {}: {err}",
                    legacy_config_path.display()
                ),
            };
        }
    };

    let transformed = match transform_imported_config(&config_contents) {
        Ok(transformed) => transformed,
        Err(err) => {
            return MigrationImportOutcome::Failed {
                warning: format!(
                    "failed to transform legacy config.toml at {}: {err}",
                    legacy_config_path.display()
                ),
            };
        }
    };

    if let Err(err) = fs::create_dir_all(mcodex_home).await {
        return MigrationImportOutcome::Failed {
            warning: format!(
                "failed to create mcodex home {} for config import: {err}",
                mcodex_home.display()
            ),
        };
    }

    let active_config_path = mcodex_home.join(CONFIG_TOML_FILE);
    match fs::write(&active_config_path, transformed).await {
        Ok(()) => MigrationImportOutcome::Imported,
        Err(err) => MigrationImportOutcome::Failed {
            warning: format!(
                "failed to import legacy config.toml from {} to {}: {err}",
                legacy_config_path.display(),
                active_config_path.display()
            ),
        },
    }
}

async fn import_auth(legacy_home: &Path, mcodex_home: &Path) -> MigrationImportOutcome {
    let legacy_auth_path = legacy_home.join("auth.json");
    let auth_contents = match fs::read(&legacy_auth_path).await {
        Ok(auth_contents) => auth_contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return MigrationImportOutcome::NotAttempted;
        }
        Err(err) => {
            return MigrationImportOutcome::Failed {
                warning: format!(
                    "failed to read legacy auth.json at {}: {err}",
                    legacy_auth_path.display()
                ),
            };
        }
    };

    if let Err(err) = fs::create_dir_all(mcodex_home).await {
        return MigrationImportOutcome::Failed {
            warning: format!(
                "failed to create mcodex home {} for auth import: {err}",
                mcodex_home.display()
            ),
        };
    }

    let active_auth_path = mcodex_home.join("auth.json");
    match fs::write(&active_auth_path, auth_contents).await {
        Ok(()) => MigrationImportOutcome::Imported,
        Err(err) => MigrationImportOutcome::Failed {
            warning: format!(
                "failed to import legacy auth.json from {} to {}: {err}",
                legacy_auth_path.display(),
                active_auth_path.display()
            ),
        },
    }
}

fn transform_imported_config(config_contents: &str) -> io::Result<String> {
    let mut doc = config_contents
        .parse::<DocumentMut>()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    if let Some(accounts_item) = doc.as_table_mut().get_mut("accounts") {
        match accounts_item {
            TomlItem::Table(accounts_table) => {
                accounts_table.remove("default_pool");
            }
            item => {
                if let Some(inline_table) = item.as_inline_table_mut() {
                    inline_table.remove("default_pool");
                }
            }
        }
    }

    Ok(doc.to_string())
}

async fn write_marker_warning(mcodex_home: &Path, marker_path: &Path) -> Option<String> {
    if let Err(err) = fs::create_dir_all(mcodex_home).await {
        return Some(format!(
            "failed to prepare mcodex home {} for product identity migration marker: {err}",
            mcodex_home.display()
        ));
    }

    create_marker(marker_path).await.err().map(|err| {
        format!(
            "failed to write product identity migration marker at {}: {err}",
            marker_path.display()
        )
    })
}

async fn create_marker(marker_path: &Path) -> io::Result<()> {
    match OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(marker_path)
        .await
    {
        Ok(mut file) => file.write_all(b"v1\n").await,
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
#[path = "product_identity_migration_tests.rs"]
mod tests;
