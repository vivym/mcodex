use super::ConfigRequirementsToml;
use super::ConfigRequirementsWithSources;
use super::RequirementSource;
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use codex_product_identity::MCODEX;
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_foundation::string::CFStringRef;
use std::ffi::c_void;
use std::io;
use tokio::task;
use toml::Value as TomlValue;

const MANAGED_PREFERENCES_APPLICATION_ID: &str = MCODEX.macos_managed_config_domain;
const MANAGED_PREFERENCES_CONFIG_KEY: &str = "config_toml_base64";
const MANAGED_PREFERENCES_REQUIREMENTS_KEY: &str = "requirements_toml_base64";

#[derive(Debug, Clone)]
pub(super) struct ManagedAdminConfigLayer {
    pub config: TomlValue,
    pub raw_toml: String,
}

#[derive(Debug)]
struct ManagedAdminRequirementsLayer {
    domain: &'static str,
    requirements: ConfigRequirementsToml,
}

#[derive(Debug, PartialEq, Eq)]
struct ManagedPreferenceValue {
    domain: &'static str,
    value: String,
}

fn managed_preferences_requirements_source(domain: &'static str) -> RequirementSource {
    RequirementSource::MdmManagedPreferences {
        domain: domain.to_string(),
        key: MANAGED_PREFERENCES_REQUIREMENTS_KEY.to_string(),
    }
}

#[cfg(test)]
pub(super) fn managed_preferences_requirements_source_for_tests() -> RequirementSource {
    managed_preferences_requirements_source(MCODEX.macos_managed_config_domain)
}

#[cfg(test)]
pub(super) fn legacy_managed_preferences_requirements_source_for_tests() -> RequirementSource {
    managed_preferences_requirements_source(MCODEX.legacy_macos_managed_config_domain)
}

pub(crate) async fn load_managed_admin_config_layer(
    override_base64: Option<&str>,
) -> io::Result<Option<ManagedAdminConfigLayer>> {
    if let Some(encoded) = override_base64 {
        let trimmed = encoded.trim();
        return if trimmed.is_empty() {
            Ok(None)
        } else {
            parse_managed_config_base64(trimmed).map(Some)
        };
    }

    match task::spawn_blocking(load_managed_admin_config).await {
        Ok(result) => result,
        Err(join_err) => {
            if join_err.is_cancelled() {
                tracing::error!("Managed config load task was cancelled");
            } else {
                tracing::error!("Managed config load task failed: {join_err}");
            }
            Err(io::Error::other("Failed to load managed config"))
        }
    }
}

fn load_managed_admin_config() -> io::Result<Option<ManagedAdminConfigLayer>> {
    load_managed_preference(MANAGED_PREFERENCES_CONFIG_KEY)?
        .map(|managed| parse_managed_config_base64(managed.value.trim()))
        .transpose()
}

pub(crate) async fn load_managed_admin_requirements_toml(
    target: &mut ConfigRequirementsWithSources,
    override_base64: Option<&str>,
) -> io::Result<()> {
    if let Some(encoded) = override_base64 {
        let trimmed = encoded.trim();
        if trimmed.is_empty() {
            return Ok(());
        }

        target.merge_unset_fields(
            managed_preferences_requirements_source(MANAGED_PREFERENCES_APPLICATION_ID),
            parse_managed_requirements_base64(trimmed)?,
        );
        return Ok(());
    }

    match task::spawn_blocking(load_managed_admin_requirements).await {
        Ok(result) => {
            if let Some(requirements) = result? {
                target.merge_unset_fields(
                    managed_preferences_requirements_source(requirements.domain),
                    requirements.requirements,
                );
            }
            Ok(())
        }
        Err(join_err) => {
            if join_err.is_cancelled() {
                tracing::error!("Managed requirements load task was cancelled");
            } else {
                tracing::error!("Managed requirements load task failed: {join_err}");
            }
            Err(io::Error::other("Failed to load managed requirements"))
        }
    }
}

fn load_managed_admin_requirements() -> io::Result<Option<ManagedAdminRequirementsLayer>> {
    load_managed_preference(MANAGED_PREFERENCES_REQUIREMENTS_KEY)?
        .map(|managed| {
            parse_managed_requirements_base64(managed.value.trim()).map(|requirements| {
                ManagedAdminRequirementsLayer {
                    domain: managed.domain,
                    requirements,
                }
            })
        })
        .transpose()
}

fn load_managed_preference(key_name: &str) -> io::Result<Option<ManagedPreferenceValue>> {
    load_managed_preference_with_lookup(key_name, load_managed_preference_for_domain)
}

fn load_managed_preference_with_lookup(
    key_name: &str,
    mut load: impl FnMut(&str, &str) -> io::Result<Option<String>>,
) -> io::Result<Option<ManagedPreferenceValue>> {
    for domain in [
        MCODEX.macos_managed_config_domain,
        MCODEX.legacy_macos_managed_config_domain,
    ] {
        if let Some(value) = load(domain, key_name)? {
            return Ok(Some(ManagedPreferenceValue { domain, value }));
        }
    }

    Ok(None)
}

fn load_managed_preference_for_domain(domain: &str, key_name: &str) -> io::Result<Option<String>> {
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFPreferencesCopyAppValue(key: CFStringRef, application_id: CFStringRef) -> *mut c_void;
    }

    let value_ref = unsafe {
        CFPreferencesCopyAppValue(
            CFString::new(key_name).as_concrete_TypeRef(),
            CFString::new(domain).as_concrete_TypeRef(),
        )
    };

    if value_ref.is_null() {
        tracing::debug!("Managed preferences for {domain} key {key_name} not found",);
        return Ok(None);
    }

    let value = unsafe { CFString::wrap_under_create_rule(value_ref as _) }.to_string();
    Ok(Some(value))
}

fn parse_managed_config_base64(encoded: &str) -> io::Result<ManagedAdminConfigLayer> {
    let raw_toml = decode_managed_preferences_base64(encoded)?;
    match toml::from_str::<TomlValue>(&raw_toml) {
        Ok(TomlValue::Table(parsed)) => Ok(ManagedAdminConfigLayer {
            config: TomlValue::Table(parsed),
            raw_toml,
        }),
        Ok(other) => {
            tracing::error!("Managed config TOML must have a table at the root, found {other:?}",);
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed config root must be a table",
            ))
        }
        Err(err) => {
            tracing::error!("Failed to parse managed config TOML: {err}");
            Err(io::Error::new(io::ErrorKind::InvalidData, err))
        }
    }
}

fn parse_managed_requirements_base64(encoded: &str) -> io::Result<ConfigRequirementsToml> {
    toml::from_str::<ConfigRequirementsToml>(&decode_managed_preferences_base64(encoded)?).map_err(
        |err| {
            tracing::error!("Failed to parse managed requirements TOML: {err}");
            io::Error::new(io::ErrorKind::InvalidData, err)
        },
    )
}

fn decode_managed_preferences_base64(encoded: &str) -> io::Result<String> {
    String::from_utf8(BASE64_STANDARD.decode(encoded.as_bytes()).map_err(|err| {
        tracing::error!("Failed to decode managed value as base64: {err}",);
        io::Error::new(io::ErrorKind::InvalidData, err)
    })?)
    .map_err(|err| {
        tracing::error!("Managed value base64 contents were not valid UTF-8: {err}",);
        io::Error::new(io::ErrorKind::InvalidData, err)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn managed_preference_lookup_prefers_active_domain() {
        let selected = load_managed_preference_with_lookup(
            MANAGED_PREFERENCES_CONFIG_KEY,
            |domain, _key_name| match domain {
                domain if domain == MCODEX.macos_managed_config_domain => {
                    Ok(Some("active".to_string()))
                }
                domain if domain == MCODEX.legacy_macos_managed_config_domain => {
                    Ok(Some("legacy".to_string()))
                }
                _ => Ok(None),
            },
        )
        .expect("load managed preference")
        .expect("selected domain");

        assert_eq!(selected.domain, MCODEX.macos_managed_config_domain);
        assert_eq!(selected.value, "active");
    }

    #[test]
    fn managed_preference_lookup_falls_back_to_legacy_domain() {
        let selected = load_managed_preference_with_lookup(
            MANAGED_PREFERENCES_CONFIG_KEY,
            |domain, _key_name| match domain {
                domain if domain == MCODEX.macos_managed_config_domain => Ok(None),
                domain if domain == MCODEX.legacy_macos_managed_config_domain => {
                    Ok(Some("legacy".to_string()))
                }
                _ => Ok(None),
            },
        )
        .expect("load managed preference")
        .expect("selected domain");

        assert_eq!(selected.domain, MCODEX.legacy_macos_managed_config_domain);
        assert_eq!(selected.value, "legacy");
    }
}
