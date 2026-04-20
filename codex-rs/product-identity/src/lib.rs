/// Static product-level metadata for mcodex and its legacy Codex migration
/// sources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProductIdentity {
    pub product_name: &'static str,
    pub display_name: &'static str,
    pub runtime_tagline: &'static str,
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
    pub repository_url: &'static str,
    pub release_api_url: &'static str,
    pub release_notes_url: &'static str,
    pub installer_dir_name: &'static str,
    pub npm_package_name: &'static str,
    pub homebrew_cask_token: &'static str,
    pub download_base_url: &'static str,
    pub stable_latest_manifest_url: &'static str,
    pub unix_install_command: &'static str,
    pub windows_install_command: &'static str,
    pub windows_install_runner_command: &'static str,
    pub macos_managed_config_domain: &'static str,
    pub legacy_macos_managed_config_domain: &'static str,
}

pub const MCODEX: ProductIdentity = ProductIdentity {
    product_name: "mcodex",
    display_name: "mcodex",
    runtime_tagline: "an OpenAI Codex-derived command-line coding agent",
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
    repository_url: "https://github.com/vivym/mcodex",
    release_api_url: "https://api.github.com/repos/vivym/mcodex/releases/latest",
    release_notes_url: "https://github.com/vivym/mcodex/releases/latest",
    installer_dir_name: "mcodex",
    npm_package_name: "@vivym/mcodex",
    homebrew_cask_token: "mcodex",
    download_base_url: "https://downloads.mcodex.sota.wiki",
    stable_latest_manifest_url: "https://downloads.mcodex.sota.wiki/repositories/mcodex/channels/stable/latest.json",
    unix_install_command: "curl -fsSL https://downloads.mcodex.sota.wiki/install.sh | sh",
    windows_install_command: "powershell -NoProfile -ExecutionPolicy Bypass -Command \"iwr -UseBasicParsing https://downloads.mcodex.sota.wiki/install.ps1 -OutFile $env:TEMP\\mcodex-install.ps1; & $env:TEMP\\mcodex-install.ps1\"",
    windows_install_runner_command: "iwr -UseBasicParsing https://downloads.mcodex.sota.wiki/install.ps1 -OutFile $env:TEMP\\mcodex-install.ps1; & $env:TEMP\\mcodex-install.ps1",
    macos_managed_config_domain: "com.vivym.mcodex",
    legacy_macos_managed_config_domain: "com.openai.codex",
};

#[cfg(test)]
mod tests {
    use super::MCODEX;
    use pretty_assertions::assert_eq;

    #[test]
    fn mcodex_identity_defines_active_and_legacy_roots() {
        assert_eq!(MCODEX.product_name, "mcodex");
        assert_eq!(MCODEX.display_name, "mcodex");
        assert_eq!(
            MCODEX.runtime_tagline,
            "an OpenAI Codex-derived command-line coding agent"
        );
        assert_eq!(MCODEX.binary_name, "mcodex");
        assert_eq!(MCODEX.default_home_dir_name, ".mcodex");
        assert_eq!(MCODEX.home_env_var, "MCODEX_HOME");
        assert_eq!(MCODEX.legacy_home_env_var, "CODEX_HOME");
        assert_eq!(MCODEX.unix_system_config_root, "/etc/mcodex");
        assert_eq!(MCODEX.legacy_unix_system_config_root, "/etc/codex");
        assert_eq!(MCODEX.windows_admin_config_components, &["Mcodex"]);
        assert_eq!(
            MCODEX.legacy_windows_admin_config_components,
            &["OpenAI", "Codex"]
        );
        assert_eq!(MCODEX.macos_managed_config_domain, "com.vivym.mcodex");
        assert_eq!(
            MCODEX.legacy_macos_managed_config_domain,
            "com.openai.codex"
        );
        assert_eq!(MCODEX.repository_url, "https://github.com/vivym/mcodex");
        assert_eq!(MCODEX.npm_package_name, "@vivym/mcodex");
        assert_eq!(MCODEX.homebrew_cask_token, "mcodex");
        assert_eq!(
            MCODEX.download_base_url,
            "https://downloads.mcodex.sota.wiki"
        );
        assert_eq!(
            MCODEX.stable_latest_manifest_url,
            "https://downloads.mcodex.sota.wiki/repositories/mcodex/channels/stable/latest.json"
        );
        assert_eq!(
            MCODEX.unix_install_command,
            "curl -fsSL https://downloads.mcodex.sota.wiki/install.sh | sh"
        );
        assert_eq!(
            MCODEX.windows_install_command,
            "powershell -NoProfile -ExecutionPolicy Bypass -Command \"iwr -UseBasicParsing https://downloads.mcodex.sota.wiki/install.ps1 -OutFile $env:TEMP\\mcodex-install.ps1; & $env:TEMP\\mcodex-install.ps1\""
        );
        assert_eq!(
            MCODEX.windows_install_runner_command,
            "iwr -UseBasicParsing https://downloads.mcodex.sota.wiki/install.ps1 -OutFile $env:TEMP\\mcodex-install.ps1; & $env:TEMP\\mcodex-install.ps1"
        );
        assert!(!MCODEX.windows_install_runner_command.contains("powershell"));
    }
}
