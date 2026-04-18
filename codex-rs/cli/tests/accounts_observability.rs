use anyhow::Result;
use clap::Parser;
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
    let list =
        AccountsCommand::try_parse_from(["codex", "pool", "list"]).expect("pool list parses");
    assert_eq!(
        format!("{:?}", list.subcommand),
        "Pool(PoolCommand { subcommand: List })"
    );

    let show = AccountsCommand::try_parse_from(["codex", "pool", "show", "--json"])
        .expect("pool show parses");
    assert_eq!(
        format!("{:?}", show.subcommand),
        "Pool(PoolCommand { subcommand: Show(PoolShowCommand { pool: None, limit: None, cursor: None, json: true }) })"
    );
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
async fn accounts_diagnostics_without_explicit_pool_keeps_placeholder_behavior() -> Result<()> {
    let codex_home = prepared_empty_home()?;
    let output = run_codex(&codex_home, &["accounts", "diagnostics"]).await?;

    assert!(!output.success, "stdout: {}", output.stdout);
    assert!(
        output
            .stderr
            .contains("Error managing accounts: accounts diagnostics is not implemented yet")
    );
    assert!(
        !output
            .stderr
            .contains("no account pool is configured; pass --pool <POOL_ID> or configure a pool")
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

fn prepared_empty_home() -> Result<TempDir> {
    Ok(TempDir::new()?)
}
