use codex_product_identity::MCODEX;

/// Update action the CLI should perform after the TUI exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateAction {
    /// Update via the script-managed installer.
    ScriptManagedLatest,
}

#[cfg_attr(test, derive(Debug, Clone, Copy, PartialEq, Eq))]
pub(crate) enum UpdatePlatform {
    Unix,
    Windows,
}

impl UpdatePlatform {
    fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

impl UpdateAction {
    /// Returns the user-facing update command for display.
    pub fn command_str(self) -> String {
        self.display_command_for_platform(UpdatePlatform::current())
            .to_string()
    }

    /// Returns the shell command and arguments for invoking the update.
    pub fn shell_invocation(self) -> (&'static str, &'static [&'static str]) {
        self.shell_invocation_for_platform(UpdatePlatform::current())
    }

    pub(crate) fn display_command_for_platform(self, platform: UpdatePlatform) -> &'static str {
        match (self, platform) {
            (Self::ScriptManagedLatest, UpdatePlatform::Unix) => MCODEX.unix_install_command,
            (Self::ScriptManagedLatest, UpdatePlatform::Windows) => MCODEX.windows_install_command,
        }
    }

    pub(crate) fn shell_invocation_for_platform(
        self,
        platform: UpdatePlatform,
    ) -> (&'static str, &'static [&'static str]) {
        match (self, platform) {
            (Self::ScriptManagedLatest, UpdatePlatform::Unix) => {
                ("sh", &["-c", MCODEX.unix_install_command])
            }
            (Self::ScriptManagedLatest, UpdatePlatform::Windows) => (
                "powershell",
                &[
                    "-NoProfile",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    MCODEX.windows_install_runner_command,
                ],
            ),
        }
    }
}

#[cfg(not(debug_assertions))]
pub(crate) fn get_update_action() -> Option<UpdateAction> {
    let managed = std::env::var("MCODEX_INSTALL_MANAGED").as_deref() == Ok("1");
    let method = std::env::var("MCODEX_INSTALL_METHOD").ok();

    detect_update_action(managed, method.as_deref())
}

#[cfg(any(not(debug_assertions), test))]
fn detect_update_action(managed: bool, method: Option<&str>) -> Option<UpdateAction> {
    (managed && method == Some("script")).then_some(UpdateAction::ScriptManagedLatest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn detects_script_managed_update_action_only() {
        assert_eq!(
            detect_update_action(/*managed*/ false, /*method*/ None),
            None
        );
        assert_eq!(
            detect_update_action(/*managed*/ false, /*method*/ Some("script")),
            None
        );
        assert_eq!(
            detect_update_action(/*managed*/ true, /*method*/ None),
            None
        );
        assert_eq!(
            detect_update_action(/*managed*/ true, /*method*/ Some("npm")),
            None
        );
        assert_eq!(
            detect_update_action(/*managed*/ true, /*method*/ Some("script")),
            Some(UpdateAction::ScriptManagedLatest)
        );
    }

    #[test]
    fn script_update_commands_use_runtime_installers() {
        assert_eq!(
            UpdateAction::ScriptManagedLatest.display_command_for_platform(UpdatePlatform::Unix),
            MCODEX.unix_install_command
        );
        assert_eq!(
            UpdateAction::ScriptManagedLatest.display_command_for_platform(UpdatePlatform::Windows),
            MCODEX.windows_install_command
        );
    }

    #[test]
    fn script_update_runner_uses_unix_shell() {
        assert_eq!(
            UpdateAction::ScriptManagedLatest.shell_invocation_for_platform(UpdatePlatform::Unix),
            ("sh", &["-c", MCODEX.unix_install_command] as &[&str])
        );
    }

    #[test]
    fn script_update_runner_uses_single_powershell_invocation() {
        let args: &[&str] = &[
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            MCODEX.windows_install_runner_command,
        ];

        assert_eq!(
            UpdateAction::ScriptManagedLatest
                .shell_invocation_for_platform(UpdatePlatform::Windows),
            ("powershell", args)
        );
    }
}
