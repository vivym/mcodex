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
async fn accounts_pool_show_renders_sorted_quota_families() -> Result<()> {
    let codex_home = prepared_home().await?;
    seed_quota_rows(&codex_home, ["codex", "chatgpt"]).await?;

    let output = run_codex(
        &codex_home,
        &["accounts", "pool", "show", "--pool", "team-main"],
    )
    .await?;

    assert!(output.success, "stderr: {}", output.stderr);
    assert!(output.stdout.contains("chatgpt"));
    assert!(output.stdout.contains("codex"));
    assert!(output.stdout.contains("secondary exhausted"));
    let quotas = output
        .stdout
        .split_once("quotas:")
        .map(|(_, quotas)| quotas)
        .expect("quota rows");
    let chatgpt = quotas.find("chatgpt").expect("chatgpt quota row");
    let codex = quotas.find("codex").expect("codex quota row");
    assert!(
        chatgpt < codex,
        "quota families should render sorted by family:\n{}",
        output.stdout
    );
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
        "--account",
        "acct-1",
        "--type",
        "leaseAcquired",
        "--type",
        "quotaObserved",
        "--limit",
        "25",
        "--cursor",
        "cursor-1",
        "--json",
    ])
    .expect("events parses");
    assert_eq!(
        format!("{:?}", events.subcommand),
        "Events(AccountsEventsCommand { pool: Some(\"team-main\"), account: Some(\"acct-1\"), types: [LeaseAcquired, QuotaObserved], limit: Some(25), cursor: Some(\"cursor-1\"), json: true })"
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
async fn accounts_events_json_preserves_cursor_and_details_payload() -> Result<()> {
    let codex_home = prepared_home().await?;
    seed_account_pool_events(&codex_home).await?;

    let output = run_codex(
        &codex_home,
        &[
            "accounts",
            "events",
            "--type",
            "leaseAcquired",
            "--type",
            "quotaObserved",
            "--limit",
            "1",
            "--json",
        ],
    )
    .await?;

    assert!(output.success, "stderr: {}", output.stderr);
    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["poolId"], "team-main");
    let data = json["data"].as_array().expect("data");
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["eventType"], "leaseAcquired");
    assert_eq!(data[0]["details"], serde_json::json!(["soft-limit", 42]));
    let next_cursor = json["nextCursor"].as_str().expect("next cursor");

    let next_output = run_codex(
        &codex_home,
        &[
            "accounts",
            "events",
            "--type",
            "leaseAcquired",
            "--type",
            "quotaObserved",
            "--limit",
            "1",
            "--cursor",
            next_cursor,
            "--json",
        ],
    )
    .await?;

    assert!(next_output.success, "stderr: {}", next_output.stderr);
    let next_json: serde_json::Value = serde_json::from_str(&next_output.stdout)?;
    let next_data = next_json["data"].as_array().expect("data");
    assert_eq!(next_data.len(), 1);
    assert_eq!(next_data[0]["eventType"], "quotaObserved");
    assert_eq!(
        next_data[0]["details"],
        serde_json::json!({"remainingPercent": 12.5})
    );
    assert!(next_json["nextCursor"].is_null());
    Ok(())
}

#[tokio::test]
async fn accounts_events_text_reports_events_none_when_filter_matches_nothing() -> Result<()> {
    let codex_home = prepared_home().await?;
    seed_account_pool_events(&codex_home).await?;

    let output = run_codex(
        &codex_home,
        &["accounts", "events", "--account", "missing-account"],
    )
    .await?;

    assert!(output.success, "stderr: {}", output.stderr);
    assert!(output.stdout.contains("events: none"));
    Ok(())
}

#[tokio::test]
async fn accounts_events_rejects_invalid_cursor() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(
        &codex_home,
        &["accounts", "events", "--cursor", "not-a-valid-cursor"],
    )
    .await?;

    assert!(!output.success, "stdout: {}", output.stdout);
    assert!(output.stderr.contains("invalid"));
    Ok(())
}

#[tokio::test]
async fn accounts_diagnostics_text_reports_issues_none_for_healthy_pool() -> Result<()> {
    let codex_home = prepared_home().await?;
    let output = run_codex(&codex_home, &["accounts", "diagnostics"]).await?;

    assert!(output.success, "stderr: {}", output.stderr);
    assert!(output.stdout.contains("status: healthy"));
    assert!(output.stdout.contains("issues: none"));
    Ok(())
}

#[tokio::test]
async fn accounts_status_json_adds_pool_observability_on_success() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["effectivePoolId"], "team-main");
    assert_eq!(json["poolObservability"]["poolId"], "team-main");
    assert_eq!(json["poolObservability"]["summary"]["totalAccounts"], 2);
    assert!(json["poolObservability"]["diagnostics"]["status"].is_string());
    assert!(json["poolObservability"]["warning"].is_null());
    Ok(())
}

#[tokio::test]
async fn accounts_status_json_keeps_startup_fields_when_observability_read_fails() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(
        &codex_home,
        &[
            "accounts",
            "--account-pool",
            "missing-pool",
            "status",
            "--json",
        ],
    )
    .await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["effectivePoolId"], "missing-pool");
    assert_eq!(json["poolObservability"]["poolId"], "missing-pool");
    assert!(json["poolObservability"]["summary"].is_null());
    assert!(json["poolObservability"]["diagnostics"].is_null());
    assert!(json["poolObservability"]["warning"].is_string());
    Ok(())
}

#[tokio::test]
async fn accounts_status_json_sets_pool_observability_null_when_no_effective_pool_resolves()
-> Result<()> {
    let codex_home = prepared_empty_home()?;

    let output = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert!(json["effectivePoolId"].is_null());
    assert!(json["poolObservability"].is_null());
    Ok(())
}

#[tokio::test]
async fn accounts_status_json_keeps_summary_when_diagnostics_read_fails() -> Result<()> {
    let codex_home = prepared_home().await?;
    seed_invalid_account_pool_event_details(&codex_home).await?;

    let output = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["poolObservability"]["poolId"], "team-main");
    assert_eq!(json["poolObservability"]["summary"]["totalAccounts"], 2);
    assert!(json["poolObservability"]["diagnostics"].is_null());
    assert!(json["poolObservability"]["warning"].is_string());
    Ok(())
}

#[tokio::test]
async fn accounts_status_text_reports_degraded_issue_summary() -> Result<()> {
    let codex_home = prepared_home().await?;
    seed_busy_and_rate_limited_pool_state(&codex_home).await?;

    let output = run_codex(&codex_home, &["accounts", "status"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    assert!(
        output
            .stdout
            .contains("pooled diagnostics status: degraded")
    );
    assert!(output.stdout.contains("issue:"));
    Ok(())
}

#[tokio::test]
async fn accounts_status_text_reports_warning_when_diagnostics_read_fails() -> Result<()> {
    let codex_home = prepared_home().await?;
    seed_invalid_account_pool_event_details(&codex_home).await?;

    let output = run_codex(&codex_home, &["accounts", "status"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    assert!(output.stdout.contains("pooled accounts total: 2"));
    assert!(output.stdout.contains("warning:"));
    Ok(())
}

async fn prepared_home() -> Result<TempDir> {
    let codex_home = TempDir::new()?;
    seed_accounts_config(codex_home.path())?;
    seed_state(codex_home.path()).await?;
    Ok(codex_home)
}

#[tokio::test]
async fn accounts_diagnostics_json_reports_degraded_issue_details() -> Result<()> {
    let codex_home = prepared_home().await?;
    seed_busy_and_rate_limited_pool_state(&codex_home).await?;

    let output = run_codex(&codex_home, &["accounts", "diagnostics", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["poolId"], "team-main");
    assert_eq!(json["status"], "degraded");
    assert!(json["generatedAt"].as_str().is_some());
    let issues = json["issues"].as_array().expect("issues");
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0]["severity"], "warning");
    assert_eq!(issues[0]["reasonCode"], "cooldownActive");
    assert_eq!(issues[0]["message"], "account acct-2 is in cooldown");
    assert_eq!(issues[0]["accountId"], "acct-2");
    assert_eq!(issues[0]["holderInstanceId"], serde_json::Value::Null);
    assert!(issues[0]["nextRelevantAt"].as_str().is_some());
    Ok(())
}

#[tokio::test]
async fn accounts_diagnostics_requires_pool_when_no_effective_pool_resolves() -> Result<()> {
    let codex_home = prepared_empty_home()?;
    let output = run_codex(&codex_home, &["accounts", "diagnostics"]).await?;

    assert!(!output.success, "stdout: {}", output.stdout);
    assert!(output.stderr.contains("pass --pool <POOL_ID>"));
    Ok(())
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

async fn seed_busy_and_rate_limited_pool_state(codex_home: &TempDir) -> Result<()> {
    let pool = SqlitePool::connect(&format!(
        "sqlite://{}",
        state_db_path(codex_home.path()).display()
    ))
    .await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    let lease_expires_at = now + 300;
    let quota_blocked_until = now + 300;
    let quota_probe_after = now + 240;

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
    .bind(lease_expires_at)
    .bind(Option::<i64>::None)
    .execute(&pool)
    .await?;

    sqlx::query(
        r#"
INSERT INTO account_quota_state (
    account_id,
    limit_id,
    primary_used_percent,
    primary_resets_at,
    secondary_used_percent,
    secondary_resets_at,
    observed_at,
    observed_at_nanos,
    exhausted_windows,
    predicted_blocked_until,
    next_probe_after,
    probe_backoff_level,
    last_probe_result,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind("acct-2")
    .bind("codex")
    .bind(100.0_f64)
    .bind(quota_blocked_until)
    .bind(Option::<f64>::None)
    .bind(Option::<i64>::None)
    .bind(now)
    .bind(now.saturating_mul(1_000_000_000))
    .bind("primary")
    .bind(quota_blocked_until)
    .bind(quota_probe_after)
    .bind(0_i64)
    .bind(Option::<String>::None)
    .bind(now)
    .execute(&pool)
    .await?;

    Ok(())
}

async fn seed_quota_rows<const N: usize>(
    codex_home: &TempDir,
    limit_ids: [&'static str; N],
) -> Result<()> {
    let pool = SqlitePool::connect(&format!(
        "sqlite://{}",
        state_db_path(codex_home.path()).display()
    ))
    .await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    let primary_resets_at = now + 300;
    let secondary_resets_at = now + 600;
    let next_probe_after = now + 120;

    for limit_id in limit_ids {
        sqlx::query(
            r#"
INSERT INTO account_quota_state (
    account_id,
    limit_id,
    primary_used_percent,
    primary_resets_at,
    secondary_used_percent,
    secondary_resets_at,
    observed_at,
    observed_at_nanos,
    exhausted_windows,
    predicted_blocked_until,
    next_probe_after,
    probe_backoff_level,
    last_probe_result,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind("acct-1")
        .bind(limit_id)
        .bind(42.0_f64)
        .bind(primary_resets_at)
        .bind(100.0_f64)
        .bind(secondary_resets_at)
        .bind(now)
        .bind(now.saturating_mul(1_000_000_000))
        .bind("secondary")
        .bind(secondary_resets_at)
        .bind(next_probe_after)
        .bind(1_i64)
        .bind(Some("still_blocked"))
        .bind(now)
        .execute(&pool)
        .await?;
    }

    Ok(())
}

async fn seed_account_pool_events(codex_home: &TempDir) -> Result<()> {
    let pool = SqlitePool::connect(&format!(
        "sqlite://{}",
        state_db_path(codex_home.path()).display()
    ))
    .await?;

    insert_account_pool_event(
        &pool,
        SeedEvent {
            event_id: "event-array",
            occurred_at: 20,
            pool_id: "team-main",
            account_id: Some("acct-1"),
            lease_id: Some("lease-1"),
            holder_instance_id: Some("holder-1"),
            event_type: "leaseAcquired",
            reason_code: Some("automaticAccountSelected"),
            message: "lease acquired",
            details_json: Some(serde_json::json!(["soft-limit", 42])),
        },
    )
    .await?;
    insert_account_pool_event(
        &pool,
        SeedEvent {
            event_id: "event-object",
            occurred_at: 10,
            pool_id: "team-main",
            account_id: Some("acct-2"),
            lease_id: None,
            holder_instance_id: None,
            event_type: "quotaObserved",
            reason_code: Some("quotaNearExhausted"),
            message: "quota observed",
            details_json: Some(serde_json::json!({"remainingPercent": 12.5})),
        },
    )
    .await?;
    Ok(())
}

async fn seed_invalid_account_pool_event_details(codex_home: &TempDir) -> Result<()> {
    let pool = SqlitePool::connect(&format!(
        "sqlite://{}",
        state_db_path(codex_home.path()).display()
    ))
    .await?;

    sqlx::query(
        r#"
INSERT INTO account_pool_events (
    event_id,
    occurred_at,
    pool_id,
    account_id,
    lease_id,
    holder_instance_id,
    event_type,
    reason_code,
    message,
    details_json
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind("event-invalid-json")
    .bind(30_i64)
    .bind("team-main")
    .bind(Option::<&str>::None)
    .bind(Option::<&str>::None)
    .bind(Option::<&str>::None)
    .bind("leaseAcquireFailed")
    .bind(Some("noEligibleAccount"))
    .bind("invalid event details")
    .bind("{not json")
    .execute(&pool)
    .await?;
    Ok(())
}

struct SeedEvent {
    event_id: &'static str,
    occurred_at: i64,
    pool_id: &'static str,
    account_id: Option<&'static str>,
    lease_id: Option<&'static str>,
    holder_instance_id: Option<&'static str>,
    event_type: &'static str,
    reason_code: Option<&'static str>,
    message: &'static str,
    details_json: Option<serde_json::Value>,
}

async fn insert_account_pool_event(pool: &SqlitePool, event: SeedEvent) -> Result<()> {
    sqlx::query(
        r#"
INSERT INTO account_pool_events (
    event_id,
    occurred_at,
    pool_id,
    account_id,
    lease_id,
    holder_instance_id,
    event_type,
    reason_code,
    message,
    details_json
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(event.event_id)
    .bind(event.occurred_at)
    .bind(event.pool_id)
    .bind(event.account_id)
    .bind(event.lease_id)
    .bind(event.holder_instance_id)
    .bind(event.event_type)
    .bind(event.reason_code)
    .bind(event.message)
    .bind(event.details_json.map(|details| details.to_string()))
    .execute(pool)
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
