use crate::personality_migration::PERSONALITY_MIGRATION_FILENAME;
use crate::personality_migration::PersonalityMigrationStatus;
use crate::personality_migration::maybe_migrate_personality;
use codex_config::CONFIG_TOML_FILE;
use codex_config::config_toml::ConfigToml;
use codex_product_identity::MCODEX;
use codex_utils_home_dir::find_legacy_codex_home_for_migration;
use std::ffi::OsStr;
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
pub const PRODUCT_IDENTITY_MIGRATION_PENDING_FILENAME: &str = ".product_identity_migration.pending";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationImportOutcome {
    NotAttempted,
    Imported,
    AlreadyPresent,
    Failed { warning: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExistingTargetBehavior {
    Warn,
    Reuse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductIdentityMigrationStatus {
    SkippedMarker,
    SkippedInitializedHome,
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
    let pending_marker_path = mcodex_home.join(PRODUCT_IDENTITY_MIGRATION_PENDING_FILENAME);
    if fs::try_exists(&marker_path).await? {
        return Ok(ProductIdentityMigrationOutcome {
            status: ProductIdentityMigrationStatus::SkippedMarker,
            config_import: MigrationImportOutcome::NotAttempted,
            auth_import: MigrationImportOutcome::NotAttempted,
            marker_warning: None,
        });
    }

    if fs::try_exists(&pending_marker_path).await? {
        let legacy_home =
            resolve_pending_legacy_home_for_import(legacy_home_result, mcodex_home)?;
        return Ok(import_product_identity_with_pending_marker(
            &legacy_home,
            mcodex_home,
            &pending_marker_path,
            &marker_path,
            ExistingTargetBehavior::Reuse,
            None,
        )
        .await);
    }

    if active_home_is_initialized(mcodex_home).await? {
        return Ok(ProductIdentityMigrationOutcome {
            status: ProductIdentityMigrationStatus::SkippedInitializedHome,
            config_import: MigrationImportOutcome::NotAttempted,
            auth_import: MigrationImportOutcome::NotAttempted,
            marker_warning: write_marker_warning(mcodex_home, &marker_path).await,
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

    let pending_marker_warning = write_marker_warning(mcodex_home, &pending_marker_path).await;
    Ok(import_product_identity_with_pending_marker(
        &legacy_home,
        mcodex_home,
        &pending_marker_path,
        &marker_path,
        ExistingTargetBehavior::Warn,
        pending_marker_warning,
    )
    .await)
}

fn resolve_pending_legacy_home_for_import(
    legacy_home_result: io::Result<Option<PathBuf>>,
    mcodex_home: &Path,
) -> io::Result<PathBuf> {
    let legacy_home = match legacy_home_result {
        Ok(Some(legacy_home)) => legacy_home,
        Ok(None) => {
            return Err(io::Error::other(format!(
                "cannot resume pending legacy Codex migration into {} because no readable legacy home is available via {} or {}",
                mcodex_home.display(),
                MCODEX.legacy_home_env_var,
                MCODEX.legacy_home_dir_name
            )));
        }
        Err(err) => {
            return Err(io::Error::other(format!(
                "cannot resume pending legacy Codex migration into {} because the legacy home could not be read: {err}",
                mcodex_home.display()
            )));
        }
    };

    if legacy_home == mcodex_home {
        return Err(io::Error::other(format!(
            "cannot resume pending legacy Codex migration because legacy home {} resolves to the active mcodex home",
            legacy_home.display()
        )));
    }

    Ok(legacy_home)
}

async fn import_product_identity_with_pending_marker(
    legacy_home: &Path,
    mcodex_home: &Path,
    pending_marker_path: &Path,
    marker_path: &Path,
    existing_target_behavior: ExistingTargetBehavior,
    pending_marker_warning: Option<String>,
) -> ProductIdentityMigrationOutcome {
    let config_import = import_config_with_existing_target_behavior(
        legacy_home,
        mcodex_home,
        existing_target_behavior,
    )
    .await;
    let auth_import = import_auth_with_existing_target_behavior(
        legacy_home,
        mcodex_home,
        existing_target_behavior,
    )
    .await;
    let marker_warning = merge_warnings(
        pending_marker_warning,
        finalize_migration_marker(mcodex_home, pending_marker_path, marker_path).await,
    );
    let has_warnings = matches!(config_import, MigrationImportOutcome::Failed { .. })
        || matches!(auth_import, MigrationImportOutcome::Failed { .. })
        || marker_warning.is_some();

    ProductIdentityMigrationOutcome {
        status: if has_warnings {
            ProductIdentityMigrationStatus::ImportedWithWarnings
        } else {
            ProductIdentityMigrationStatus::Imported
        },
        config_import,
        auth_import,
        marker_warning,
    }
}

#[cfg(test)]
async fn import_config(legacy_home: &Path, mcodex_home: &Path) -> MigrationImportOutcome {
    import_config_with_existing_target_behavior(
        legacy_home,
        mcodex_home,
        ExistingTargetBehavior::Warn,
    )
    .await
}

async fn import_config_with_existing_target_behavior(
    legacy_home: &Path,
    mcodex_home: &Path,
    existing_target_behavior: ExistingTargetBehavior,
) -> MigrationImportOutcome {
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
    match write_new_file(&active_config_path, transformed.as_bytes()).await {
        Ok(()) => MigrationImportOutcome::Imported,
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => match existing_target_behavior {
            ExistingTargetBehavior::Warn => MigrationImportOutcome::Failed {
                warning: format!(
                    "refusing to overwrite existing mcodex config.toml at {} during legacy import",
                    active_config_path.display()
                ),
            },
            ExistingTargetBehavior::Reuse => MigrationImportOutcome::AlreadyPresent,
        },
        Err(err) => MigrationImportOutcome::Failed {
            warning: format!(
                "failed to import legacy config.toml from {} to {}: {err}",
                legacy_config_path.display(),
                active_config_path.display()
            ),
        },
    }
}

#[cfg(test)]
async fn import_auth(legacy_home: &Path, mcodex_home: &Path) -> MigrationImportOutcome {
    import_auth_with_existing_target_behavior(
        legacy_home,
        mcodex_home,
        ExistingTargetBehavior::Warn,
    )
    .await
}

async fn import_auth_with_existing_target_behavior(
    legacy_home: &Path,
    mcodex_home: &Path,
    existing_target_behavior: ExistingTargetBehavior,
) -> MigrationImportOutcome {
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
    match write_new_file(&active_auth_path, &auth_contents).await {
        Ok(()) => MigrationImportOutcome::Imported,
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => match existing_target_behavior {
            ExistingTargetBehavior::Warn => MigrationImportOutcome::Failed {
                warning: format!(
                    "refusing to overwrite existing mcodex auth.json at {} during legacy import",
                    active_auth_path.display()
                ),
            },
            ExistingTargetBehavior::Reuse => MigrationImportOutcome::AlreadyPresent,
        },
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

async fn active_home_is_initialized(mcodex_home: &Path) -> io::Result<bool> {
    for relative_path in [
        CONFIG_TOML_FILE,
        "auth.json",
        PRODUCT_IDENTITY_MIGRATION_FILENAME,
        PERSONALITY_MIGRATION_FILENAME,
        "skills",
        "sessions",
        "plugins",
        "marketplace",
        "log",
        "logs",
        "themes",
    ] {
        if fs::try_exists(&mcodex_home.join(relative_path)).await? {
            return Ok(true);
        }
    }

    let mut entries = match fs::read_dir(mcodex_home).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };

    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        if file_type.is_file() && entry.path().extension() == Some(OsStr::new("sqlite")) {
            return Ok(true);
        }
    }

    Ok(false)
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

async fn finalize_migration_marker(
    mcodex_home: &Path,
    pending_marker_path: &Path,
    marker_path: &Path,
) -> Option<String> {
    let marker_warning = write_marker_warning(mcodex_home, marker_path).await;
    if marker_warning.is_some() {
        return marker_warning;
    }

    clear_pending_marker_warning(pending_marker_path).await
}

async fn clear_pending_marker_warning(pending_marker_path: &Path) -> Option<String> {
    match fs::remove_file(pending_marker_path).await {
        Ok(()) => None,
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => Some(format!(
            "failed to clear pending product identity migration marker at {}: {err}",
            pending_marker_path.display()
        )),
    }
}

fn merge_warnings(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (None, None) => None,
        (Some(warning), None) | (None, Some(warning)) => Some(warning),
        (Some(first_warning), Some(second_warning)) => {
            Some(format!("{first_warning}; {second_warning}"))
        }
    }
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

async fn write_new_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .await?;
    file.write_all(contents).await
}

#[cfg(test)]
#[path = "product_identity_migration_tests.rs"]
mod tests;
