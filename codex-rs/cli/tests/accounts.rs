use std::path::Path;

use anyhow::Result;
use codex_state::AccountStartupSelectionState;
use codex_state::AccountStartupSelectionUpdate;
use codex_state::LegacyAccountImport;
use codex_state::StateRuntime;
use codex_state::state_db_path;
use pretty_assertions::assert_eq;
use sqlx::SqlitePool;
use tempfile::TempDir;

const CHATGPT_AUTH_JWT: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJlbWFpbCI6InVzZXJAZXhhbXBsZS5jb20iLCJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9wbGFuX3R5cGUiOiJwcm8iLCJjaGF0Z3B0X3VzZXJfaWQiOiJ1c2VyLTEyMzQ1IiwiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdC0xIn19.c2ln";

struct CodexOutput {
    stdout: String,
    stderr: String,
    success: bool,
}

async fn prepared_home() -> Result<TempDir> {
    let codex_home = TempDir::new()?;
    seed_chatgpt_auth(codex_home.path())?;
    seed_accounts_config(codex_home.path())?;
    seed_state(codex_home.path()).await?;
    Ok(codex_home)
}

async fn prepared_migrated_home() -> Result<TempDir> {
    let codex_home = TempDir::new()?;
    seed_chatgpt_auth(codex_home.path())?;
    seed_migrated_state(codex_home.path()).await?;
    Ok(codex_home)
}

async fn prepared_legacy_auth_only_home() -> Result<TempDir> {
    let codex_home = TempDir::new()?;
    seed_chatgpt_auth(codex_home.path())?;
    Ok(codex_home)
}

async fn run_codex(codex_home: &TempDir, args: &[&str]) -> Result<CodexOutput> {
    let output = assert_cmd::Command::new(codex_utils_cargo_bin::cargo_bin("codex")?)
        .env("CODEX_HOME", codex_home.path())
        .args(args)
        .output()?;

    Ok(CodexOutput {
        stdout: String::from_utf8(output.stdout)?,
        stderr: String::from_utf8(output.stderr)?,
        success: output.status.success(),
    })
}

async fn read_startup_selection(codex_home: &TempDir) -> Result<AccountStartupSelectionState> {
    let runtime =
        StateRuntime::init(codex_home.path().to_path_buf(), "test-provider".to_string()).await?;
    runtime.read_account_startup_selection().await
}

async fn write_startup_selection(
    codex_home: &TempDir,
    update: AccountStartupSelectionUpdate,
) -> Result<()> {
    let runtime =
        StateRuntime::init(codex_home.path().to_path_buf(), "test-provider".to_string()).await?;
    runtime.write_account_startup_selection(update).await?;
    Ok(())
}

async fn read_pool_membership(
    codex_home: &TempDir,
    account_id: &str,
) -> Result<Option<codex_state::AccountPoolMembership>> {
    let runtime =
        StateRuntime::init(codex_home.path().to_path_buf(), "test-provider".to_string()).await?;
    runtime.read_account_pool_membership(account_id).await
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

async fn seed_state(codex_home: &Path) -> Result<()> {
    let runtime = StateRuntime::init(codex_home.to_path_buf(), "test-provider".to_string()).await?;
    let pool =
        SqlitePool::connect(&format!("sqlite://{}", state_db_path(codex_home).display())).await?;
    seed_account(&pool, "acct-1", "team-main", 0).await?;
    seed_account(&pool, "acct-2", "team-main", 1).await?;
    seed_account(&pool, "acct-other", "team-other", 0).await?;
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("team-main".to_string()),
            preferred_account_id: Some("acct-1".to_string()),
            suppressed: true,
        })
        .await?;
    Ok(())
}

async fn seed_migrated_state(codex_home: &Path) -> Result<()> {
    let runtime = StateRuntime::init(codex_home.to_path_buf(), "test-provider".to_string()).await?;
    runtime
        .import_legacy_default_account(LegacyAccountImport {
            account_id: "acct-legacy".to_string(),
        })
        .await?;
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
    healthy,
    created_at,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(account_id)
    .bind(pool_id)
    .bind(position)
    .bind("chatgpt")
    .bind("chatgpt")
    .bind("workspace-main")
    .bind(1_i64)
    .bind(1_i64)
    .bind(1_i64)
    .execute(pool)
    .await?;
    Ok(())
}

async fn seed_busy_and_unhealthy_pool_state(codex_home: &TempDir) -> Result<()> {
    let pool = SqlitePool::connect(&format!(
        "sqlite://{}",
        state_db_path(codex_home.path()).display()
    ))
    .await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    let expires_at = now + 300;

    sqlx::query(
        r#"
INSERT INTO account_leases (
    lease_id,
    pool_id,
    account_id,
    holder_instance_id,
    lease_epoch,
    acquired_at,
    renewed_at,
    expires_at,
    released_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind("lease-1")
    .bind("team-main")
    .bind("acct-1")
    .bind("holder-1")
    .bind(0_i64)
    .bind(now)
    .bind(now)
    .bind(expires_at)
    .bind(Option::<i64>::None)
    .execute(&pool)
    .await?;

    sqlx::query(
        r#"
INSERT INTO account_runtime_state (
    account_id,
    pool_id,
    health_state,
    last_health_event_sequence,
    last_health_event_at,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind("acct-2")
    .bind("team-main")
    .bind("rate_limited")
    .bind(1_i64)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await?;

    sqlx::query(
        r#"
UPDATE account_registry
SET healthy = 0,
    updated_at = ?
WHERE account_id = ?
        "#,
    )
    .bind(now)
    .bind("acct-2")
    .execute(&pool)
    .await?;

    Ok(())
}

#[tokio::test]
async fn login_status_reads_legacy_auth_view_only() -> Result<()> {
    let codex_home = prepared_home().await?;
    let output = run_codex(&codex_home, &["login", "status"]).await?;
    assert!(output.success);
    assert!(output.stderr.contains("Logged in using ChatGPT"));
    Ok(())
}

#[tokio::test]
async fn accounts_current_reports_predicted_pool_selection() -> Result<()> {
    let codex_home = prepared_home().await?;
    let output = run_codex(&codex_home, &["accounts", "current"]).await?;
    assert!(output.success);
    assert!(output.stdout.contains("effective pool"));
    assert!(output.stdout.contains("predicted account"));
    Ok(())
}

#[tokio::test]
async fn accounts_list_lists_registered_accounts_and_marks_migrated_source() -> Result<()> {
    let codex_home = prepared_migrated_home().await?;

    let output = run_codex(&codex_home, &["accounts", "list"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let lines = output
        .stdout
        .lines()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec!["acct-legacy pool=legacy-default enabled=true healthy=true source=migrated"]
    );

    Ok(())
}

#[tokio::test]
async fn accounts_list_bootstraps_legacy_auth_into_pooled_state() -> Result<()> {
    let codex_home = prepared_legacy_auth_only_home().await?;

    let output = run_codex(&codex_home, &["accounts", "list"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let lines = output
        .stdout
        .lines()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec!["acct-1 pool=legacy-default enabled=true healthy=true source=migrated"]
    );
    assert_eq!(
        read_startup_selection(&codex_home).await?,
        AccountStartupSelectionState {
            default_pool_id: Some("legacy-default".to_string()),
            preferred_account_id: Some("acct-1".to_string()),
            suppressed: false,
        }
    );
    assert_eq!(
        read_pool_membership(&codex_home, "acct-1")
            .await?
            .expect("membership"),
        codex_state::AccountPoolMembership {
            account_id: "acct-1".to_string(),
            pool_id: "legacy-default".to_string(),
            source: Some(codex_state::AccountSource::Migrated),
            enabled: true,
            healthy: true,
        }
    );

    Ok(())
}

#[tokio::test]
async fn accounts_add_failures_leave_legacy_auth_only_home_unbootstrapped() -> Result<()> {
    for args in [
        vec!["accounts", "add"],
        vec!["accounts", "add", "chatgpt"],
        vec!["accounts", "add", "chatgpt", "--device-auth"],
        vec!["accounts", "add", "api-key"],
    ] {
        let codex_home = prepared_legacy_auth_only_home().await?;

        let output = run_codex(&codex_home, &args).await?;
        assert!(!output.success, "stdout: {}", output.stdout);
        assert!(
            output.stderr.contains(
                "pooled credential storage keyed by `credential_ref` is not implemented yet"
            ),
            "stderr: {}",
            output.stderr
        );
        assert!(
            !state_db_path(codex_home.path()).exists(),
            "state db should not be created for {args:?}"
        );
    }

    Ok(())
}

#[tokio::test]
async fn accounts_current_json_reports_startup_preview() -> Result<()> {
    let codex_home = prepared_home().await?;
    let output = run_codex(&codex_home, &["accounts", "current", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["effectivePoolId"], "team-main");
    assert_eq!(json["preferredAccountId"], "acct-1");
    assert_eq!(json["predictedAccountId"], serde_json::Value::Null);
    assert_eq!(json["suppressed"], true);
    assert_eq!(json["eligibility"]["code"], "suppressed");

    Ok(())
}

#[tokio::test]
async fn accounts_status_reports_suppression_and_eligibility() -> Result<()> {
    let codex_home = prepared_home().await?;
    let output = run_codex(&codex_home, &["accounts", "status"]).await?;
    assert!(output.success);
    assert!(output.stdout.contains("health state: healthy"));
    assert!(output.stdout.contains("eligibility"));
    Ok(())
}

#[tokio::test]
async fn accounts_status_json_suppressed_normalizes_healthy_account_eligibility() -> Result<()> {
    let codex_home = prepared_home().await?;
    let output = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["suppressed"], true);
    assert_eq!(json["switchReason"]["code"], "suppressed");
    assert_eq!(json["healthState"], "healthy");

    let accounts = json["accounts"].as_array().expect("accounts array");
    assert_eq!(accounts.len(), 2);
    assert_eq!(accounts[0]["eligibility"]["code"], "suppressed");
    assert_eq!(accounts[1]["eligibility"]["code"], "suppressed");
    for account in accounts {
        let code = account["eligibility"]["code"]
            .as_str()
            .expect("eligibility code");
        assert_ne!(code, "preferredAccountSelected");
        assert_ne!(code, "automaticAccountSelected");
        assert_ne!(code, "eligible");
    }

    Ok(())
}

#[tokio::test]
async fn accounts_status_json_marks_migrated_effective_pool_and_account_source() -> Result<()> {
    let codex_home = prepared_migrated_home().await?;

    let output = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["effectivePoolId"], "legacy-default");
    assert_eq!(json["effectivePoolSource"], "migrated");
    assert_eq!(json["predictedAccountId"], "acct-legacy");

    let accounts = json["accounts"].as_array().expect("accounts array");
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0]["accountId"], "acct-legacy");
    assert_eq!(accounts[0]["poolId"], "legacy-default");
    assert_eq!(accounts[0]["source"], "migrated");

    Ok(())
}

#[tokio::test]
async fn accounts_list_keeps_migrated_source_after_reassignment() -> Result<()> {
    let codex_home = prepared_legacy_auth_only_home().await?;
    let runtime =
        StateRuntime::init(codex_home.path().to_path_buf(), "test-provider".to_string()).await?;
    runtime
        .import_legacy_default_account(LegacyAccountImport {
            account_id: "acct-1".to_string(),
        })
        .await?;
    runtime.assign_account_pool("acct-1", "team-main").await?;
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("team-main".to_string()),
            preferred_account_id: Some("acct-1".to_string()),
            suppressed: false,
        })
        .await?;

    let output = run_codex(&codex_home, &["accounts", "list"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);
    assert_eq!(
        output
            .stdout
            .lines()
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>(),
        vec!["acct-1 pool=team-main enabled=true healthy=true source=migrated"]
    );

    Ok(())
}

#[tokio::test]
async fn accounts_status_json_keeps_migrated_source_after_reassignment() -> Result<()> {
    let codex_home = prepared_legacy_auth_only_home().await?;
    let runtime =
        StateRuntime::init(codex_home.path().to_path_buf(), "test-provider".to_string()).await?;
    runtime
        .import_legacy_default_account(LegacyAccountImport {
            account_id: "acct-1".to_string(),
        })
        .await?;
    runtime.assign_account_pool("acct-1", "team-main").await?;
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("team-main".to_string()),
            preferred_account_id: Some("acct-1".to_string()),
            suppressed: false,
        })
        .await?;

    let output = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["effectivePoolId"], "team-main");
    assert_eq!(json["effectivePoolSource"], "migrated");
    assert_eq!(json["accounts"][0]["poolId"], "team-main");
    assert_eq!(json["accounts"][0]["source"], "migrated");

    Ok(())
}

#[tokio::test]
async fn accounts_status_json_does_not_infer_migrated_from_legacy_default_pool_id() -> Result<()> {
    let codex_home = prepared_legacy_auth_only_home().await?;
    let runtime =
        StateRuntime::init(codex_home.path().to_path_buf(), "test-provider".to_string()).await?;
    let pool = SqlitePool::connect(&format!(
        "sqlite://{}",
        state_db_path(codex_home.path()).display()
    ))
    .await?;
    seed_account(&pool, "acct-local", "pool-main", 0).await?;
    runtime
        .assign_account_pool("acct-local", "legacy-default")
        .await?;
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("legacy-default".to_string()),
            preferred_account_id: Some("acct-local".to_string()),
            suppressed: false,
        })
        .await?;

    let output = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["effectivePoolId"], "legacy-default");
    assert_eq!(json["effectivePoolSource"], serde_json::Value::Null);
    assert_eq!(json["accounts"][0]["accountId"], "acct-local");
    assert_eq!(json["accounts"][0]["poolId"], "legacy-default");
    assert_eq!(json["accounts"][0]["source"], serde_json::Value::Null);

    Ok(())
}

#[tokio::test]
async fn accounts_status_json_suppressed_preserves_hard_ineligibility_reasons() -> Result<()> {
    let codex_home = prepared_home().await?;
    seed_busy_and_unhealthy_pool_state(&codex_home).await?;

    let output = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["suppressed"], true);
    assert_eq!(json["switchReason"]["code"], "suppressed");
    assert_eq!(json["healthState"], "coolingDown");

    let accounts = json["accounts"].as_array().expect("accounts array");
    assert_eq!(accounts.len(), 2);
    assert_eq!(accounts[0]["accountId"], "acct-1");
    assert_eq!(accounts[0]["eligibility"]["code"], "preferredAccountBusy");
    assert_eq!(
        accounts[0]["eligibility"]["reason"],
        "preferred account is currently leased by another runtime"
    );
    assert_eq!(accounts[1]["accountId"], "acct-2");
    assert_eq!(accounts[1]["eligibility"]["code"], "rateLimited");
    assert_eq!(
        accounts[1]["eligibility"]["reason"],
        "account is rate limited"
    );
    for account in accounts {
        let code = account["eligibility"]["code"]
            .as_str()
            .expect("eligibility code");
        assert_ne!(code, "preferredAccountSelected");
        assert_ne!(code, "automaticAccountSelected");
        assert_ne!(code, "eligible");
    }

    Ok(())
}

#[tokio::test]
async fn accounts_status_json_reports_pool_diagnostics_and_per_account_reasons() -> Result<()> {
    let codex_home = prepared_home().await?;
    write_startup_selection(
        &codex_home,
        AccountStartupSelectionUpdate {
            default_pool_id: Some("team-main".to_string()),
            preferred_account_id: None,
            suppressed: false,
        },
    )
    .await?;
    seed_busy_and_unhealthy_pool_state(&codex_home).await?;

    let output = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["effectivePoolId"], "team-main");
    assert_eq!(json["healthState"], "coolingDown");
    assert_eq!(json["predictedAccountId"], serde_json::Value::Null);
    assert_eq!(json["switchReason"]["code"], "noEligibleAccount");
    assert!(json["nextEligibleAt"].as_str().is_some());

    let accounts = json["accounts"].as_array().expect("accounts array");
    assert_eq!(accounts.len(), 2);
    assert_eq!(accounts[0]["accountId"], "acct-1");
    assert_eq!(accounts[0]["healthState"], "healthy");
    assert_eq!(accounts[0]["eligibility"]["code"], "busy");
    assert_eq!(
        accounts[0]["eligibility"]["reason"],
        "account is currently leased by another runtime"
    );
    assert!(accounts[0]["nextEligibleAt"].as_str().is_some());
    assert_eq!(accounts[1]["accountId"], "acct-2");
    assert_eq!(accounts[1]["healthState"], "rateLimited");
    assert_eq!(accounts[1]["eligibility"]["code"], "rateLimited");
    assert_eq!(
        accounts[1]["eligibility"]["reason"],
        "account is rate limited"
    );

    Ok(())
}

#[tokio::test]
async fn accounts_status_json_preserves_preferred_rate_limited_reason() -> Result<()> {
    let codex_home = prepared_home().await?;
    write_startup_selection(
        &codex_home,
        AccountStartupSelectionUpdate {
            default_pool_id: Some("team-main".to_string()),
            preferred_account_id: Some("acct-2".to_string()),
            suppressed: false,
        },
    )
    .await?;
    seed_busy_and_unhealthy_pool_state(&codex_home).await?;

    let output = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["healthState"], "healthy");

    let accounts = json["accounts"].as_array().expect("accounts array");
    assert_eq!(accounts[0]["accountId"], "acct-2");
    assert_eq!(
        accounts[0]["eligibility"]["code"],
        "preferredAccountRateLimited"
    );
    assert_eq!(
        accounts[0]["eligibility"]["reason"],
        "preferred account is rate limited"
    );

    Ok(())
}

#[tokio::test]
async fn accounts_status_json_distinguishes_automatic_selection_from_other_eligible_accounts()
-> Result<()> {
    let codex_home = prepared_home().await?;
    write_startup_selection(
        &codex_home,
        AccountStartupSelectionUpdate {
            default_pool_id: Some("team-main".to_string()),
            preferred_account_id: None,
            suppressed: false,
        },
    )
    .await?;

    let output = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["effectivePoolId"], "team-main");
    assert_eq!(json["healthState"], "healthy");
    assert_eq!(json["predictedAccountId"], "acct-1");

    let accounts = json["accounts"].as_array().expect("accounts array");
    assert_eq!(accounts.len(), 2);
    assert_eq!(accounts[0]["accountId"], "acct-1");
    assert_eq!(
        accounts[0]["eligibility"]["code"],
        "automaticAccountSelected"
    );
    assert_eq!(
        accounts[0]["eligibility"]["reason"],
        "account is selected for automatic startup selection"
    );
    assert_eq!(accounts[1]["accountId"], "acct-2");
    assert_eq!(accounts[1]["eligibility"]["code"], "eligible");
    assert_eq!(
        accounts[1]["eligibility"]["reason"],
        "account is eligible for automatic startup selection"
    );

    Ok(())
}

#[tokio::test]
async fn accounts_disable_excludes_account_from_automatic_selection_and_reports_enabled()
-> Result<()> {
    let codex_home = prepared_home().await?;
    write_startup_selection(
        &codex_home,
        AccountStartupSelectionUpdate {
            default_pool_id: Some("team-main".to_string()),
            preferred_account_id: None,
            suppressed: false,
        },
    )
    .await?;

    let disable = run_codex(&codex_home, &["accounts", "disable", "acct-1"]).await?;
    assert!(disable.success, "stderr: {}", disable.stderr);

    let output = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["predictedAccountId"], "acct-2");

    let accounts = json["accounts"].as_array().expect("accounts array");
    assert_eq!(accounts[0]["accountId"], "acct-1");
    assert_eq!(accounts[0]["enabled"], false);
    assert_eq!(accounts[0]["eligibility"]["code"], "disabled");
    assert_eq!(accounts[1]["accountId"], "acct-2");
    assert_eq!(accounts[1]["enabled"], true);
    assert_eq!(
        read_pool_membership(&codex_home, "acct-1")
            .await?
            .expect("membership")
            .enabled,
        false
    );

    Ok(())
}

#[tokio::test]
async fn accounts_enable_restores_automatic_selection() -> Result<()> {
    let codex_home = prepared_home().await?;
    write_startup_selection(
        &codex_home,
        AccountStartupSelectionUpdate {
            default_pool_id: Some("team-main".to_string()),
            preferred_account_id: None,
            suppressed: false,
        },
    )
    .await?;

    let disable = run_codex(&codex_home, &["accounts", "disable", "acct-1"]).await?;
    assert!(disable.success, "stderr: {}", disable.stderr);
    let enable = run_codex(&codex_home, &["accounts", "enable", "acct-1"]).await?;
    assert!(enable.success, "stderr: {}", enable.stderr);

    let output = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["predictedAccountId"], "acct-1");

    let accounts = json["accounts"].as_array().expect("accounts array");
    assert_eq!(accounts[0]["accountId"], "acct-1");
    assert_eq!(accounts[0]["enabled"], true);
    assert_eq!(
        read_pool_membership(&codex_home, "acct-1")
            .await?
            .expect("membership")
            .enabled,
        true
    );

    Ok(())
}

#[tokio::test]
async fn accounts_current_json_reports_disabled_preferred_account_reason() -> Result<()> {
    let codex_home = prepared_home().await?;
    write_startup_selection(
        &codex_home,
        AccountStartupSelectionUpdate {
            default_pool_id: Some("team-main".to_string()),
            preferred_account_id: Some("acct-1".to_string()),
            suppressed: false,
        },
    )
    .await?;

    let disable = run_codex(&codex_home, &["accounts", "disable", "acct-1"]).await?;
    assert!(disable.success, "stderr: {}", disable.stderr);

    let output = run_codex(&codex_home, &["accounts", "current", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["predictedAccountId"], serde_json::Value::Null);
    assert_eq!(json["eligibility"]["code"], "preferredAccountDisabled");

    Ok(())
}

#[tokio::test]
async fn accounts_status_accepts_account_pool_override_without_persisting_it() -> Result<()> {
    let codex_home = prepared_home().await?;
    write_startup_selection(
        &codex_home,
        AccountStartupSelectionUpdate {
            default_pool_id: Some("team-main".to_string()),
            preferred_account_id: None,
            suppressed: false,
        },
    )
    .await?;

    let output = run_codex(
        &codex_home,
        &[
            "accounts",
            "--account-pool",
            "team-other",
            "status",
            "--json",
        ],
    )
    .await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["effectivePoolId"], "team-other");
    assert_eq!(json["accountPoolOverrideId"], "team-other");
    assert_eq!(json["healthState"], "healthy");
    assert_eq!(json["predictedAccountId"], "acct-other");
    assert_eq!(json["switchReason"]["code"], "automaticAccountSelected");

    assert_eq!(
        read_startup_selection(&codex_home).await?,
        AccountStartupSelectionState {
            default_pool_id: Some("team-main".to_string()),
            preferred_account_id: None,
            suppressed: false,
        }
    );

    Ok(())
}

#[tokio::test]
async fn accounts_resume_clears_durable_suppression() -> Result<()> {
    let codex_home = prepared_home().await?;
    let output = run_codex(&codex_home, &["accounts", "resume"]).await?;
    assert!(output.success);
    assert!(output.stdout.contains("automatic selection resumed"));
    let selection = read_startup_selection(&codex_home).await?;
    assert!(!selection.suppressed);
    assert_eq!(selection.preferred_account_id, None);
    Ok(())
}

#[tokio::test]
async fn accounts_resume_does_not_persist_transient_default_pool_override() -> Result<()> {
    let codex_home = prepared_home().await?;
    let output = run_codex(
        &codex_home,
        &[
            "-c",
            "accounts.default_pool=\"team-override\"",
            "accounts",
            "resume",
        ],
    )
    .await?;
    assert!(output.success);

    let selection = read_startup_selection(&codex_home).await?;
    assert_eq!(
        selection,
        AccountStartupSelectionState {
            default_pool_id: Some("team-main".to_string()),
            preferred_account_id: None,
            suppressed: false,
        }
    );
    Ok(())
}

#[tokio::test]
async fn accounts_switch_sets_preferred_account_override() -> Result<()> {
    let codex_home = prepared_home().await?;
    let output = run_codex(&codex_home, &["accounts", "switch", "acct-2"]).await?;
    assert!(output.success);
    assert!(output.stdout.contains("preferred account"));
    let selection = read_startup_selection(&codex_home).await?;
    assert_eq!(selection.preferred_account_id.as_deref(), Some("acct-2"));
    assert!(!selection.suppressed);
    Ok(())
}

#[tokio::test]
async fn accounts_remove_deletes_registry_entry() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(&codex_home, &["accounts", "remove", "acct-2"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    assert_eq!(read_pool_membership(&codex_home, "acct-2").await?, None);

    Ok(())
}

#[tokio::test]
async fn accounts_add_chatgpt_stops_at_credential_storage_gap() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(&codex_home, &["accounts", "add", "chatgpt"]).await?;
    assert!(!output.success);
    assert!(output.stderr.contains("credential_ref"));
    assert!(output.stderr.contains("codex login"));

    Ok(())
}

#[tokio::test]
async fn accounts_add_without_mode_stops_at_credential_storage_gap() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(&codex_home, &["accounts", "add"]).await?;
    assert!(!output.success);
    assert!(output.stderr.contains("credential_ref"));
    assert!(output.stderr.contains("codex login"));

    Ok(())
}

#[tokio::test]
async fn accounts_add_chatgpt_device_auth_stops_at_credential_storage_gap() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(
        &codex_home,
        &["accounts", "add", "chatgpt", "--device-auth"],
    )
    .await?;
    assert!(!output.success);
    assert!(output.stderr.contains("credential_ref"));
    assert!(output.stderr.contains("codex login"));

    Ok(())
}

#[tokio::test]
async fn accounts_add_api_key_stops_at_credential_storage_gap() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(&codex_home, &["accounts", "add", "api-key"]).await?;
    assert!(!output.success);
    assert!(output.stderr.contains("credential_ref"));
    assert!(output.stderr.contains("codex login"));

    Ok(())
}

#[tokio::test]
async fn accounts_pool_assign_changes_membership_without_mutating_startup_selection() -> Result<()>
{
    let codex_home = prepared_home().await?;
    let before = read_startup_selection(&codex_home).await?;

    let output = run_codex(
        &codex_home,
        &["accounts", "pool", "assign", "acct-2", "team-other"],
    )
    .await?;
    assert!(output.success, "stderr: {}", output.stderr);

    assert_eq!(
        read_pool_membership(&codex_home, "acct-2")
            .await?
            .expect("membership")
            .pool_id,
        "team-other"
    );
    assert_eq!(read_startup_selection(&codex_home).await?, before);

    Ok(())
}

#[tokio::test]
async fn accounts_pool_list_reports_known_pool_ids_from_registry_state() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(&codex_home, &["accounts", "pool", "list"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let lines = output
        .stdout
        .lines()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(lines, vec!["team-main", "team-other"]);

    Ok(())
}

#[tokio::test]
async fn accounts_switch_rejects_cross_pool_override() -> Result<()> {
    let codex_home = prepared_home().await?;
    let output = run_codex(&codex_home, &["accounts", "switch", "acct-other"]).await?;
    assert!(!output.success);
    assert!(output.stderr.contains("current effective pool"));
    Ok(())
}

#[tokio::test]
async fn logout_enables_durable_startup_suppression_for_future_runtimes() -> Result<()> {
    let codex_home = prepared_home().await?;
    let output = run_codex(&codex_home, &["logout"]).await?;
    assert!(output.success);
    assert!(
        output
            .stderr
            .contains("automatic pooled selection suppressed")
    );
    assert_eq!(
        read_startup_selection(&codex_home).await?,
        AccountStartupSelectionState {
            default_pool_id: Some("team-main".to_string()),
            preferred_account_id: Some("acct-1".to_string()),
            suppressed: true,
        }
    );
    Ok(())
}

#[tokio::test]
async fn logout_does_not_persist_transient_default_pool_override() -> Result<()> {
    let codex_home = prepared_home().await?;
    let output = run_codex(
        &codex_home,
        &["-c", "accounts.default_pool=\"team-override\"", "logout"],
    )
    .await?;
    assert!(output.success);
    assert_eq!(
        read_startup_selection(&codex_home).await?,
        AccountStartupSelectionState {
            default_pool_id: Some("team-main".to_string()),
            preferred_account_id: Some("acct-1".to_string()),
            suppressed: true,
        }
    );
    Ok(())
}
