use super::LoaderOverrides;
#[cfg(target_os = "macos")]
use super::macos::ManagedAdminConfigLayer;
#[cfg(target_os = "macos")]
use super::macos::load_managed_admin_config_layer;
use codex_config::config_error_from_toml;
use codex_config::io_error_from_config_error;
use codex_product_identity::MCODEX;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use tokio::fs;
use toml::Value as TomlValue;

#[derive(Debug, Clone)]
pub(super) struct MangedConfigFromFile {
    pub managed_config: TomlValue,
    pub file: AbsolutePathBuf,
}

#[derive(Debug, Clone)]
pub(super) struct ManagedConfigFromMdm {
    pub managed_config: TomlValue,
    pub raw_toml: String,
}

#[derive(Debug, Clone)]
pub(super) struct LoadedConfigLayers {
    /// If present, data read from a file such as `/etc/mcodex/managed_config.toml`.
    pub managed_config: Option<MangedConfigFromFile>,
    /// If present, data read from managed preferences (macOS only).
    pub managed_config_from_mdm: Option<ManagedConfigFromMdm>,
}

pub(super) async fn load_config_layers_internal(
    codex_home: &Path,
    overrides: LoaderOverrides,
) -> io::Result<LoadedConfigLayers> {
    #[cfg(target_os = "macos")]
    let LoaderOverrides {
        managed_config_path,
        managed_preferences_base64,
        ..
    } = overrides;

    #[cfg(not(target_os = "macos"))]
    let LoaderOverrides {
        managed_config_path,
        ..
    } = overrides;

    let managed_config_path = AbsolutePathBuf::from_absolute_path(
        managed_config_path.unwrap_or_else(|| managed_config_path_for_load(codex_home)),
    )?;

    let managed_config =
        read_config_from_path(&managed_config_path, /*log_missing_as_info*/ false)
            .await?
            .map(|managed_config| MangedConfigFromFile {
                managed_config,
                file: managed_config_path.clone(),
            });

    #[cfg(target_os = "macos")]
    let managed_preferences =
        load_managed_admin_config_layer(managed_preferences_base64.as_deref())
            .await?
            .map(map_managed_admin_layer);

    #[cfg(not(target_os = "macos"))]
    let managed_preferences = None;

    Ok(LoadedConfigLayers {
        managed_config,
        managed_config_from_mdm: managed_preferences,
    })
}

#[cfg(target_os = "macos")]
fn map_managed_admin_layer(layer: ManagedAdminConfigLayer) -> ManagedConfigFromMdm {
    let ManagedAdminConfigLayer {
        config, raw_toml, ..
    } = layer;
    ManagedConfigFromMdm {
        managed_config: config,
        raw_toml,
    }
}

pub(super) async fn read_config_from_path(
    path: impl AsRef<Path>,
    log_missing_as_info: bool,
) -> io::Result<Option<TomlValue>> {
    match fs::read_to_string(path.as_ref()).await {
        Ok(contents) => match toml::from_str::<TomlValue>(&contents) {
            Ok(value) => Ok(Some(value)),
            Err(err) => {
                tracing::error!("Failed to parse {}: {err}", path.as_ref().display());
                let config_error = config_error_from_toml(path.as_ref(), &contents, err.clone());
                Err(io_error_from_config_error(
                    io::ErrorKind::InvalidData,
                    config_error,
                    Some(err),
                ))
            }
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            if log_missing_as_info {
                tracing::info!("{} not found, using defaults", path.as_ref().display());
            } else {
                tracing::debug!("{} not found", path.as_ref().display());
            }
            Ok(None)
        }
        Err(err) => {
            tracing::error!("Failed to read {}: {err}", path.as_ref().display());
            Err(err)
        }
    }
}

/// Return the default managed config path.
pub(super) fn managed_config_default_path(codex_home: &Path) -> PathBuf {
    #[cfg(unix)]
    {
        let _ = codex_home;
        Path::new(MCODEX.unix_system_config_root).join("managed_config.toml")
    }

    #[cfg(not(unix))]
    {
        codex_home.join("managed_config.toml")
    }
}

fn managed_config_path_for_load(codex_home: &Path) -> PathBuf {
    #[cfg(unix)]
    {
        let active_path = managed_config_default_path(codex_home);
        let Some(active_root) = active_path.parent() else {
            return active_path;
        };
        managed_config_path_with_legacy_fallback(
            active_root,
            Path::new(MCODEX.legacy_unix_system_config_root),
        )
    }

    #[cfg(not(unix))]
    {
        managed_config_default_path(codex_home)
    }
}

#[cfg(unix)]
fn managed_config_path_with_legacy_fallback(active_root: &Path, legacy_root: &Path) -> PathBuf {
    let active_path = active_root.join("managed_config.toml");
    let legacy_path = legacy_root.join("managed_config.toml");
    if active_path.exists() || !legacy_path.exists() {
        active_path
    } else {
        legacy_path
    }
}

#[cfg(test)]
pub(super) fn managed_config_default_path_for_tests() -> PathBuf {
    managed_config_default_path(Path::new("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    #[cfg(unix)]
    #[test]
    fn managed_config_path_for_load_prefers_active_root_when_present() {
        let temp_dir = tempdir().expect("tempdir");
        let active_root = temp_dir.path().join("mcodex");
        let legacy_root = temp_dir.path().join("codex");
        std::fs::create_dir_all(&active_root).expect("create active root");
        std::fs::create_dir_all(&legacy_root).expect("create legacy root");
        std::fs::write(
            active_root.join("managed_config.toml"),
            "model = \"active\"",
        )
        .expect("write active managed config");
        std::fs::write(
            legacy_root.join("managed_config.toml"),
            "model = \"legacy\"",
        )
        .expect("write legacy managed config");

        let selected = managed_config_path_with_legacy_fallback(&active_root, &legacy_root);

        assert_eq!(selected, active_root.join("managed_config.toml"),);
    }

    #[cfg(unix)]
    #[test]
    fn managed_config_path_for_load_falls_back_to_legacy_root() {
        let temp_dir = tempdir().expect("tempdir");
        let active_root = temp_dir.path().join("mcodex");
        let legacy_root = temp_dir.path().join("codex");
        std::fs::create_dir_all(&legacy_root).expect("create legacy root");
        std::fs::write(
            legacy_root.join("managed_config.toml"),
            "model = \"legacy\"",
        )
        .expect("write legacy managed config");

        let selected = managed_config_path_with_legacy_fallback(&active_root, &legacy_root);

        assert_eq!(selected, legacy_root.join("managed_config.toml"),);
    }
}
