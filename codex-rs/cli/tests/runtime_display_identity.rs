use std::path::Path;

use anyhow::Result;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

fn mcodex_command(codex_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(codex_utils_cargo_bin::cargo_bin("mcodex")?);
    cmd.env("MCODEX_HOME", codex_home);
    Ok(cmd)
}

#[tokio::test]
async fn runtime_display_identity_version_uses_mcodex_identity() -> Result<()> {
    let codex_home = TempDir::new()?;

    let mut cmd = mcodex_command(codex_home.path())?;
    let assert = cmd.arg("--version").assert().success();
    let output = assert.get_output();
    let stdout = String::from_utf8(output.stdout.clone())?;

    assert_eq!(stdout, format!("mcodex {}\n", env!("CARGO_PKG_VERSION")));
    assert!(output.stderr.is_empty());
    assert!(!stdout.contains("codex-cli"));

    Ok(())
}

#[tokio::test]
async fn runtime_display_identity_help_uses_mcodex_identity() -> Result<()> {
    let codex_home = TempDir::new()?;

    let mut cmd = mcodex_command(codex_home.path())?;
    let assert = cmd.arg("--help").assert().success();
    let output = assert.get_output();
    let stdout = String::from_utf8(output.stdout.clone())?;

    assert!(stdout.contains("mcodex CLI"));
    assert!(stdout.contains("Run mcodex non-interactively"));
    assert!(stdout.contains("Manage external MCP servers for mcodex"));
    assert!(stdout.contains("Manage mcodex plugins"));
    assert!(stdout.contains("Start mcodex as an MCP server (stdio)"));
    assert!(stdout.contains("Run commands within a mcodex-provided sandbox"));
    assert!(stdout.contains("Apply the latest diff produced by the mcodex agent"));
    assert!(!stdout.contains("codex-cli"));
    assert!(!stdout.contains("Run Codex non-interactively"));
    assert!(!stdout.contains("Manage external MCP servers for Codex"));
    assert!(!stdout.contains("Manage plugin marketplaces for Codex"));
    assert!(!stdout.contains("Start Codex as an MCP server"));
    assert!(!stdout.contains("Launch the Codex desktop app"));
    assert!(!stdout.contains("Codex-provided sandbox"));
    assert!(!stdout.contains("Codex agent"));
    if cfg!(target_os = "macos") {
        assert!(stdout.contains("Launch the mcodex desktop app"));
        assert!(!stdout.contains("Launch the Codex desktop app"));
    }

    Ok(())
}
