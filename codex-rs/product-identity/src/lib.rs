/// Static product-level metadata for mcodex and its legacy Codex migration
/// sources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProductIdentity {
    pub product_name: &'static str,
    pub binary_name: &'static str,
    pub default_home_dir_name: &'static str,
    pub home_env_var: &'static str,
    pub legacy_binary_name: &'static str,
    pub legacy_home_dir_name: &'static str,
    pub legacy_home_env_var: &'static str,
    pub unix_system_config_root: &'static str,
    pub legacy_unix_system_config_root: &'static str,
    pub windows_admin_config_components: &'static [&'static str],
    pub legacy_windows_admin_config_components: &'static [&'static str],
    pub github_repo_owner: &'static str,
    pub github_repo_name: &'static str,
    pub release_api_url: &'static str,
    pub release_notes_url: &'static str,
    pub installer_dir_name: &'static str,
    pub macos_managed_config_domain: &'static str,
}

pub const MCODEX: ProductIdentity = ProductIdentity {
    product_name: "mcodex",
    binary_name: "mcodex",
    default_home_dir_name: ".mcodex",
    home_env_var: "MCODEX_HOME",
    legacy_binary_name: "codex",
    legacy_home_dir_name: ".codex",
    legacy_home_env_var: "CODEX_HOME",
    unix_system_config_root: "/etc/mcodex",
    legacy_unix_system_config_root: "/etc/codex",
    windows_admin_config_components: &["Mcodex"],
    legacy_windows_admin_config_components: &["OpenAI", "Codex"],
    github_repo_owner: "vivym",
    github_repo_name: "mcodex",
    release_api_url: "https://api.github.com/repos/vivym/mcodex/releases/latest",
    release_notes_url: "https://github.com/vivym/mcodex/releases/latest",
    installer_dir_name: "mcodex",
    macos_managed_config_domain: "com.vivym.mcodex",
};

#[cfg(test)]
mod tests {
    use super::MCODEX;
    use pretty_assertions::assert_eq;

    #[test]
    fn mcodex_identity_defines_active_and_legacy_roots() {
        assert_eq!(MCODEX.product_name, "mcodex");
        assert_eq!(MCODEX.binary_name, "mcodex");
        assert_eq!(MCODEX.default_home_dir_name, ".mcodex");
        assert_eq!(MCODEX.home_env_var, "MCODEX_HOME");
        assert_eq!(MCODEX.legacy_home_env_var, "CODEX_HOME");
        assert_eq!(MCODEX.unix_system_config_root, "/etc/mcodex");
        assert_eq!(MCODEX.legacy_unix_system_config_root, "/etc/codex");
        assert!(MCODEX.windows_admin_config_components.contains(&"Mcodex"));
        assert!(
            MCODEX
                .legacy_windows_admin_config_components
                .contains(&"Codex")
        );
        assert_eq!(MCODEX.macos_managed_config_domain, "com.vivym.mcodex");
    }
}
