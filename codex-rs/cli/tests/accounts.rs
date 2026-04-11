use std::path::Path;

use anyhow::Result;
use codex_state::AccountStartupSelectionUpdate;
use codex_state::StateRuntime;
use tempfile::TempDir;

const CHATGPT_AUTH_JWT: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJlbWFpbCI6InVzZXJAZXhhbXBsZS5jb20iLCJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9wbGFuX3R5cGUiOiJwcm8iLCJjaGF0Z3B0X3VzZXJfaWQiOiJ1c2VyLTEyMzQ1IiwiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdC0xIn19.c2ln";

struct CodexOutput {
    stdout: String,
    stderr: String,
}

async fn run_codex(args: &[&str]) -> Result<CodexOutput> {
    let codex_home = TempDir::new()?;
    seed_chatgpt_auth(codex_home.path())?;
    seed_accounts_config(codex_home.path())?;
    seed_startup_selection(codex_home.path()).await?;

    let output = assert_cmd::Command::new(codex_utils_cargo_bin::cargo_bin("codex")?)
        .env("CODEX_HOME", codex_home.path())
        .args(args)
        .output()?;
    assert!(
        output.status.success(),
        "codex {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(CodexOutput {
        stdout: String::from_utf8(output.stdout)?,
        stderr: String::from_utf8(output.stderr)?,
    })
}

fn seed_chatgpt_auth(codex_home: &Path) -> Result<()> {
    let auth_json = serde_json::json!({
        "tokens": {
            "id_token": CHATGPT_AUTH_JWT,
            "access_token": "test-access-token",
            "refresh_token": "test-refresh-token",
            "account_id": "acct-1"
        }
    });
    std::fs::write(
        codex_home.join("auth.json"),
        serde_json::to_string_pretty(&auth_json)?,
    )?;
    Ok(())
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

async fn seed_startup_selection(codex_home: &Path) -> Result<()> {
    let runtime = StateRuntime::init(codex_home.to_path_buf(), "test-provider".to_string()).await?;
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("team-main".to_string()),
            preferred_account_id: Some("acct-1".to_string()),
            suppressed: true,
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn login_status_reads_legacy_auth_view_only() -> Result<()> {
    let output = run_codex(&["login", "status"]).await?;
    assert!(output.stderr.contains("Logged in using ChatGPT"));
    Ok(())
}

#[tokio::test]
async fn accounts_current_reports_predicted_pool_selection() -> Result<()> {
    let output = run_codex(&["accounts", "current"]).await?;
    assert!(output.stdout.contains("effective pool"));
    Ok(())
}

#[tokio::test]
async fn accounts_status_reports_suppression_and_eligibility() -> Result<()> {
    let output = run_codex(&["accounts", "status"]).await?;
    assert!(output.stdout.contains("eligibility"));
    Ok(())
}

#[tokio::test]
async fn accounts_resume_clears_durable_suppression() -> Result<()> {
    let output = run_codex(&["accounts", "resume"]).await?;
    assert!(output.stdout.contains("automatic selection resumed"));
    Ok(())
}

#[tokio::test]
async fn accounts_switch_sets_preferred_account_override() -> Result<()> {
    let output = run_codex(&["accounts", "switch", "acct-2"]).await?;
    assert!(output.stdout.contains("preferred account"));
    Ok(())
}

#[tokio::test]
async fn logout_enables_durable_startup_suppression_for_future_runtimes() -> Result<()> {
    let output = run_codex(&["logout"]).await?;
    assert!(
        output
            .stderr
            .contains("automatic pooled selection suppressed")
    );
    Ok(())
}
