use std::path::Path;

use anyhow::Result;
use clap::Parser;
use codex_cli::AccountsCommand;
use codex_state::StateRuntime;
use codex_state::state_db_path;
use pretty_assertions::assert_eq;
use sqlx::SqlitePool;
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
async fn accounts_pool_show_json_uses_effective_pool_and_preserves_nullable_fields() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(&codex_home, &["accounts", "pool", "show", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["poolId"], "team-main");
    assert_eq!(json["summary"]["totalAccounts"], 2);
    assert!(json["refreshedAt"].is_string() || json["refreshedAt"].is_null());
    assert_eq!(json["data"].as_array().expect("data").len(), 2);
    assert!(json["data"][0]["quota"].is_null());
    assert!(json["nextCursor"].is_null());
    Ok(())
}

#[tokio::test]
async fn accounts_pool_show_command_pool_overrides_current_effective_pool() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(
        &codex_home,
        &["accounts", "pool", "show", "--pool", "team-other", "--json"],
    )
    .await?;

    assert!(output.success, "stderr: {}", output.stderr);
    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["poolId"], "team-other");
    assert_eq!(json["data"].as_array().expect("data").len(), 1);
    Ok(())
}

#[tokio::test]
async fn accounts_pool_show_rejects_conflicting_pool_flags_before_runtime_init() -> Result<()> {
    let codex_home = prepared_empty_home()?;
    let output = run_codex(
        &codex_home,
        &[
            "-c",
            "sqlite_home=\"/dev/null/nope\"",
            "accounts",
            "--account-pool",
            "team-main",
            "pool",
            "show",
            "--pool",
            "team-other",
        ],
    )
    .await?;

    assert!(!output.success, "stdout: {}", output.stdout);
    assert!(output.stderr.contains("conflicts with --account-pool"));
    assert!(
        !output
            .stderr
            .contains("initialize account startup selection state")
    );
    Ok(())
}

#[tokio::test]
async fn accounts_pool_show_text_reports_accounts_none_for_empty_pool() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(
        &codex_home,
        &["accounts", "pool", "show", "--pool", "team-empty"],
    )
    .await?;

    assert!(output.success, "stderr: {}", output.stderr);
    assert!(output.stdout.contains("accounts: none"));
    Ok(())
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

#[test]
fn accounts_events_parse_flags() {
    let events = AccountsCommand::try_parse_from([
        "codex",
        "events",
        "--pool",
        "team-main",
        "--limit",
        "25",
        "--cursor",
        "cursor-1",
        "--json",
    ])
    .expect("events parses");
    assert_eq!(
        format!("{:?}", events.subcommand),
        "Events(AccountsEventsCommand { pool: Some(\"team-main\"), limit: Some(25), cursor: Some(\"cursor-1\"), json: true })"
    );
}

#[tokio::test]
async fn accounts_events_rejects_conflicting_pool_flags() -> Result<()> {
    let codex_home = TempDir::new()?;
    let output = run_codex(
        &codex_home,
        &[
            "accounts",
            "--account-pool",
            "team-main",
            "events",
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

#[tokio::test]
async fn accounts_diagnostics_placeholder_does_not_require_runtime_init() -> Result<()> {
    let codex_home = prepared_empty_home()?;
    let output = run_codex(
        &codex_home,
        &[
            "-c",
            "sqlite_home=\"/dev/null/nope\"",
            "accounts",
            "diagnostics",
        ],
    )
    .await?;

    assert!(!output.success, "stdout: {}", output.stdout);
    assert!(
        output
            .stderr
            .contains("Error managing accounts: accounts diagnostics is not implemented yet")
    );
    assert!(
        !output
            .stderr
            .contains("initialize account startup selection state")
    );
    Ok(())
}

async fn prepared_home() -> Result<TempDir> {
    let codex_home = TempDir::new()?;
    seed_accounts_config(codex_home.path())?;
    seed_state(codex_home.path()).await?;
    Ok(codex_home)
}

fn prepared_empty_home() -> Result<TempDir> {
    Ok(TempDir::new()?)
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

fn seed_accounts_config(codex_home: &Path) -> Result<()> {
    std::fs::write(
        codex_home.join("config.toml"),
        r#"
[accounts]
default_pool = "team-main"

[accounts.pools.team-main]
allow_context_reuse = false

[accounts.pools.team-empty]
allow_context_reuse = true
"#,
    )?;
    Ok(())
}

async fn seed_state(codex_home: &Path) -> Result<()> {
    StateRuntime::init(codex_home.to_path_buf(), "test-provider".to_string()).await?;
    let pool =
        SqlitePool::connect(&format!("sqlite://{}", state_db_path(codex_home).display())).await?;
    seed_account(&pool, "acct-1", "team-main", 0).await?;
    seed_account(&pool, "acct-2", "team-main", 1).await?;
    seed_account(&pool, "acct-other", "team-other", 0).await?;
    Ok(())
}

async fn seed_account(
    pool: &SqlitePool,
    account_id: &str,
    pool_id: &str,
    position: i64,
) -> Result<()> {
    sqlx::query(
        r#"
INSERT INTO account_registry (
    account_id,
    pool_id,
    position,
    account_kind,
    backend_family,
    workspace_id,
    backend_id,
    backend_account_handle,
    provider_fingerprint,
    healthy,
    created_at,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(account_id)
    .bind(pool_id)
    .bind(position)
    .bind("chatgpt")
    .bind("chatgpt")
    .bind("workspace-main")
    .bind("local")
    .bind(account_id)
    .bind(format!("test:{account_id}"))
    .bind(1_i64)
    .bind(1_i64)
    .bind(1_i64)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
INSERT INTO account_pool_membership (
    account_id,
    pool_id,
    position,
    assigned_at,
    updated_at
) VALUES (?, ?, ?, ?, ?)
        "#,
    )
    .bind(account_id)
    .bind(pool_id)
    .bind(position)
    .bind(1_i64)
    .bind(1_i64)
    .execute(pool)
    .await?;
    Ok(())
}
