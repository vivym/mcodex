use std::path::Path;

use anyhow::Result;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
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
    cmd.arg("--version")
        .assert()
        .success()
        .stdout(contains("mcodex "))
        .stdout(predicates::str::is_match("codex-cli").unwrap().not());

    Ok(())
}

#[tokio::test]
async fn runtime_display_identity_help_uses_mcodex_identity() -> Result<()> {
    let codex_home = TempDir::new()?;

    let mut cmd = mcodex_command(codex_home.path())?;
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(contains("mcodex CLI"))
        .stdout(contains("Run Codex non-interactively").not());

    Ok(())
}
