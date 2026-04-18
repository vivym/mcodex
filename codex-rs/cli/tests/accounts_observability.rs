use std::path::Path;

use anyhow::Result;
use clap::CommandFactory;
use codex_cli::AccountsCommand;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

struct CodexOutput {
    stdout: String,
    stderr: String,
    success: bool,
}

#[test]
fn accounts_pool_show_and_existing_pool_subcommands_parse() {
    let list = AccountsCommand::command()
        .try_get_matches_from(["codex", "pool", "list"])
        .expect("pool list parses");
    let Some(("pool", pool_matches)) = list.subcommand() else {
        panic!("expected pool subcommand");
    };
    let Some(("list", list_matches)) = pool_matches.subcommand() else {
        panic!("expected pool list subcommand");
    };
    assert_eq!(list_matches.ids().count(), 0);

    let show = AccountsCommand::command()
        .try_get_matches_from(["codex", "pool", "show", "--json"])
        .expect("pool show parses");
    let Some(("pool", pool_matches)) = show.subcommand() else {
        panic!("expected pool subcommand");
    };
    let Some(("show", show_matches)) = pool_matches.subcommand() else {
        panic!("expected pool show subcommand");
    };
    assert_eq!(show_matches.get_one::<String>("pool"), None);
    assert_eq!(show_matches.get_one::<u32>("limit"), None);
    assert_eq!(show_matches.get_one::<String>("cursor"), None);
    assert!(show_matches.get_flag("json"));
}

#[tokio::test]
async fn accounts_diagnostics_rejects_conflicting_pool_flags() -> Result<()> {
    let codex_home = TempDir::new()?;
    let output = run_codex(
        &codex_home,
        &[
            "accounts",
            "--account-pool",
            "team-main",
            "diagnostics",
            "--pool",
            "team-other",
        ],
    )
    .await?;

    assert!(!output.success, "stdout: {}", output.stdout);
    assert!(output.stderr.contains("conflicts with --account-pool"));
    Ok(())
}

#[tokio::test]
async fn accounts_diagnostics_uses_effective_pool_before_not_implemented_error() -> Result<()> {
    let codex_home = prepared_effective_pool_home()?;
    let output = run_codex(&codex_home, &["accounts", "diagnostics"]).await?;

    assert!(!output.success, "stdout: {}", output.stdout);
    assert!(
        output
            .stderr
            .contains("Error managing accounts: accounts diagnostics is not implemented yet")
    );
    assert!(!output.stderr.contains("no account pool is configured"));
    Ok(())
}

#[tokio::test]
async fn accounts_diagnostics_requires_pool_when_no_effective_pool_resolves() -> Result<()> {
    let codex_home = prepared_no_pool_home()?;
    let output = run_codex(&codex_home, &["accounts", "diagnostics"]).await?;

    assert!(!output.success, "stdout: {}", output.stdout);
    assert!(
        output
            .stderr
            .contains("no account pool is configured; pass --pool <POOL_ID> or configure a pool")
    );
    assert!(
        !output
            .stderr
            .contains("accounts diagnostics is not implemented yet")
    );
    Ok(())
}

async fn run_codex(codex_home: &TempDir, args: &[&str]) -> Result<CodexOutput> {
    let output = assert_cmd::Command::new(codex_utils_cargo_bin::cargo_bin("codex")?)
        .env("MCODEX_HOME", codex_home.path())
        .env_remove("CODEX_HOME")
        .args(args)
        .output()?;

    Ok(CodexOutput {
        stdout: String::from_utf8(output.stdout)?,
        stderr: String::from_utf8(output.stderr)?,
        success: output.status.success(),
    })
}

fn prepared_effective_pool_home() -> Result<TempDir> {
    let codex_home = TempDir::new()?;
    seed_accounts_config(codex_home.path())?;
    Ok(codex_home)
}

fn prepared_no_pool_home() -> Result<TempDir> {
    Ok(TempDir::new()?)
}

fn seed_accounts_config(codex_home: &Path) -> Result<()> {
    std::fs::write(
        codex_home.join("config.toml"),
        r#"
[accounts]
default_pool = "team-main"

[accounts.pools.team-main]
allow_context_reuse = false
"#,
    )?;
    Ok(())
}
