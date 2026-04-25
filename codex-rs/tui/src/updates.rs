#![cfg(not(debug_assertions))]

use crate::legacy_core::config::Config;
use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use codex_login::default_client::create_client;
use codex_product_identity::MCODEX;
use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;

use crate::version::CODEX_CLI_VERSION;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CachedUpdateInfo {
    pub(crate) latest_version: String,
    pub(crate) latest_notes_url: Option<String>,
}

pub fn get_upgrade_version(config: &Config) -> Option<CachedUpdateInfo> {
    if !config.check_for_update_on_startup || is_source_build_version(CODEX_CLI_VERSION) {
        return None;
    }

    let version_file = version_filepath(config);
    let info = read_cached_version_info(&version_file);

    info.and_then(cached_update_info)
}

fn read_cached_version_info(version_file: &Path) -> Option<VersionInfo> {
    let info = read_version_info(version_file).ok();

    if match &info {
        None => true,
        Some(info) => info.last_checked_at < Utc::now() - Duration::hours(20),
    } {
        // Refresh the cached latest version in the background so TUI startup
        // isn’t blocked by a network call. The UI reads the previously cached
        // value (if any) for this run; the next run shows the banner if needed.
        let version_file = version_file.to_path_buf();
        tokio::spawn(async move {
            check_for_update(&version_file)
                .await
                .inspect_err(|e| tracing::error!("Failed to update version: {e}"))
        });
    }

    info
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct VersionInfo {
    latest_version: String,
    #[serde(default)]
    latest_notes_url: Option<String>,
    // ISO-8601 timestamp (RFC3339)
    last_checked_at: DateTime<Utc>,
    #[serde(default)]
    dismissed_version: Option<String>,
}

const VERSION_FILENAME: &str = "version.json";

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct LatestManifest {
    version: String,
    #[serde(default)]
    notes_url: String,
}

fn version_filepath(config: &Config) -> PathBuf {
    config.codex_home.join(VERSION_FILENAME).into_path_buf()
}

fn read_version_info(version_file: &Path) -> anyhow::Result<VersionInfo> {
    let contents = std::fs::read_to_string(version_file)?;
    Ok(serde_json::from_str(&contents)?)
}

async fn check_for_update(version_file: &Path) -> anyhow::Result<()> {
    let LatestManifest {
        version: latest_version,
        notes_url,
    } = create_client()
        .get(MCODEX.stable_latest_manifest_url)
        .send()
        .await?
        .error_for_status()?
        .json::<LatestManifest>()
        .await?;

    // Preserve any previously dismissed version if present.
    let prev_info = read_version_info(version_file).ok();
    let info = VersionInfo {
        latest_version,
        latest_notes_url: normalize_notes_url(Some(notes_url)),
        last_checked_at: Utc::now(),
        dismissed_version: prev_info.and_then(|p| p.dismissed_version),
    };

    let json_line = format!("{}\n", serde_json::to_string(&info)?);
    if let Some(parent) = version_file.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(version_file, json_line).await?;
    Ok(())
}

fn is_newer(latest: &str, current: &str) -> Option<bool> {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => Some(l > c),
        _ => None,
    }
}

/// Returns the latest version to show in a popup, if it should be shown.
/// This respects the user's dismissal choice for the current latest version.
pub fn get_upgrade_version_for_popup(config: &Config) -> Option<CachedUpdateInfo> {
    if !config.check_for_update_on_startup || is_source_build_version(CODEX_CLI_VERSION) {
        return None;
    }

    let version_file = version_filepath(config);
    let info = read_cached_version_info(&version_file)?;
    let latest = cached_update_info(info.clone())?;
    // If the user dismissed this exact version previously, do not show the popup.
    if info.dismissed_version.as_deref() == Some(latest.latest_version.as_str()) {
        return None;
    }
    Some(latest)
}

/// Persist a dismissal for the current latest version so we don't show
/// the update popup again for this version.
pub async fn dismiss_version(config: &Config, version: &str) -> anyhow::Result<()> {
    let version_file = version_filepath(config);
    let mut info = match read_version_info(&version_file) {
        Ok(info) => info,
        Err(_) => return Ok(()),
    };
    info.dismissed_version = Some(version.to_string());
    let json_line = format!("{}\n", serde_json::to_string(&info)?);
    if let Some(parent) = version_file.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(version_file, json_line).await?;
    Ok(())
}

fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut iter = v.trim().split('.');
    let maj = iter.next()?.parse::<u64>().ok()?;
    let min = iter.next()?.parse::<u64>().ok()?;
    let pat = iter.next()?.parse::<u64>().ok()?;
    Some((maj, min, pat))
}

fn is_source_build_version(version: &str) -> bool {
    parse_version(version) == Some((0, 0, 0))
}

fn cached_update_info(info: VersionInfo) -> Option<CachedUpdateInfo> {
    if is_newer(&info.latest_version, CODEX_CLI_VERSION).unwrap_or(false) {
        Some(CachedUpdateInfo {
            latest_version: info.latest_version,
            latest_notes_url: normalize_notes_url(info.latest_notes_url),
        })
    } else {
        None
    }
}

fn normalize_notes_url(notes_url: Option<String>) -> Option<String> {
    notes_url.and_then(|notes_url| {
        let notes_url = notes_url.trim();
        (!notes_url.is_empty()).then(|| notes_url.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn latest_manifest_parses_version_and_notes_url() {
        let manifest: LatestManifest = serde_json::from_str(
            r#"{
                "version": "1.5.0",
                "notesUrl": "https://example.com/releases/1.5.0"
            }"#,
        )
        .expect("manifest should parse");

        assert_eq!(
            manifest,
            LatestManifest {
                version: "1.5.0".to_string(),
                notes_url: "https://example.com/releases/1.5.0".to_string(),
            }
        );
    }

    #[test]
    fn latest_manifest_allows_missing_notes_url() {
        let manifest: LatestManifest = serde_json::from_str(
            r#"{
                "version": "1.5.0"
            }"#,
        )
        .expect("manifest should parse");

        assert_eq!(
            manifest,
            LatestManifest {
                version: "1.5.0".to_string(),
                notes_url: String::new(),
            }
        );
    }

    #[test]
    fn old_cache_without_notes_url_still_deserializes_and_surfaces_update() {
        let info: VersionInfo = serde_json::from_str(&format!(
            r#"{{
                    "latest_version": "999.999.999",
                    "last_checked_at": "{}"
                }}"#,
            Utc::now().to_rfc3339()
        ))
        .expect("old cache payload should parse");

        assert_eq!(
            cached_update_info(info),
            Some(CachedUpdateInfo {
                latest_version: "999.999.999".to_string(),
                latest_notes_url: None,
            })
        );
    }

    #[test]
    fn blank_notes_url_is_normalized_to_none_in_cached_handoff() {
        let info = VersionInfo {
            latest_version: "999.999.999".to_string(),
            latest_notes_url: Some("   ".to_string()),
            last_checked_at: Utc::now(),
            dismissed_version: None,
        };

        assert_eq!(
            cached_update_info(info),
            Some(CachedUpdateInfo {
                latest_version: "999.999.999".to_string(),
                latest_notes_url: None,
            })
        );
    }

    #[test]
    fn prerelease_version_is_not_considered_newer() {
        assert_eq!(is_newer("0.11.0-beta.1", "0.11.0"), None);
        assert_eq!(is_newer("1.0.0-rc.1", "1.0.0"), None);
    }

    #[test]
    fn plain_semver_comparisons_work() {
        assert_eq!(is_newer("0.11.1", "0.11.0"), Some(true));
        assert_eq!(is_newer("0.11.0", "0.11.1"), Some(false));
        assert_eq!(is_newer("1.0.0", "0.9.9"), Some(true));
        assert_eq!(is_newer("0.9.9", "1.0.0"), Some(false));
    }

    #[test]
    fn whitespace_is_ignored() {
        assert_eq!(parse_version(" 1.2.3 \n"), Some((1, 2, 3)));
        assert_eq!(is_newer(" 1.2.3 ", "1.2.2"), Some(true));
    }
}
