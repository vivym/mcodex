use std::fmt;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use anyhow::bail;
use chrono::Duration;
use chrono::Utc;
use clap::ValueEnum;
use codex_state::AccountPoolEventRecord;
use codex_state::AccountQuotaStateRecord;
use codex_state::AccountStartupSelectionUpdate;
use codex_state::QuotaExhaustedWindows;
use codex_state::QuotaProbeResult;
use codex_state::RegisteredAccountMembership;
use codex_state::RegisteredAccountUpsert;
use codex_state::StateRuntime;
use serde::Serialize;

const MAIN_POOL_ID: &str = "team-main";
const OTHER_POOL_ID: &str = "team-other";
const MAIN_ACCOUNT_ID: &str = "acct-main-1";
const OTHER_ACCOUNT_ID: &str = "acct-other-1";
const SECOND_MAIN_ACCOUNT_ID: &str = "acct-main-2";
const MISSING_POOL_ID: &str = "missing-pool";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum SmokeScenario {
    Empty,
    SinglePool,
    MultiPool,
    PersistedDefault,
    ConfigDefaultConflict,
    InvalidPersistedDefault,
    InvalidConfigDefault,
    Observability,
}

impl SmokeScenario {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::SinglePool => "single-pool",
            Self::MultiPool => "multi-pool",
            Self::PersistedDefault => "persisted-default",
            Self::ConfigDefaultConflict => "config-default-conflict",
            Self::InvalidPersistedDefault => "invalid-persisted-default",
            Self::InvalidConfigDefault => "invalid-config-default",
            Self::Observability => "observability",
        }
    }
}

impl fmt::Display for SmokeScenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmokeFixtureSummary {
    pub home: String,
    pub scenario: String,
    pub pools: Vec<String>,
    pub accounts: Vec<String>,
    pub credentials: &'static str,
}

pub async fn seed_fixture(home: &Path, scenario: SmokeScenario) -> Result<SmokeFixtureSummary> {
    validate_smoke_home(home)?;
    tokio::fs::create_dir_all(home).await?;
    let runtime =
        StateRuntime::init(home.to_path_buf(), "mcodex-smoke-fixture".to_string()).await?;

    match scenario {
        SmokeScenario::Empty => {}
        SmokeScenario::SinglePool => {
            seed_account(&runtime, MAIN_ACCOUNT_ID, MAIN_POOL_ID, 0).await?;
        }
        SmokeScenario::MultiPool => {
            seed_multi_pool(&runtime).await?;
        }
        SmokeScenario::PersistedDefault => {
            seed_multi_pool(&runtime).await?;
            write_startup_default(&runtime, MAIN_POOL_ID).await?;
        }
        SmokeScenario::ConfigDefaultConflict => {
            seed_multi_pool(&runtime).await?;
            write_startup_default(&runtime, OTHER_POOL_ID).await?;
            write_config_default(home, MAIN_POOL_ID).await?;
        }
        SmokeScenario::InvalidPersistedDefault => {
            seed_account(&runtime, MAIN_ACCOUNT_ID, MAIN_POOL_ID, 0).await?;
            write_startup_default(&runtime, MISSING_POOL_ID).await?;
        }
        SmokeScenario::InvalidConfigDefault => {
            seed_account(&runtime, MAIN_ACCOUNT_ID, MAIN_POOL_ID, 0).await?;
            write_config_default(home, MISSING_POOL_ID).await?;
        }
        SmokeScenario::Observability => {
            seed_observability(&runtime).await?;
        }
    }

    Ok(summary_for(home, scenario))
}

fn validate_smoke_home(home: &Path) -> Result<()> {
    let mut protected_paths = Vec::new();
    for env_name in ["MCODEX_HOME", "CODEX_HOME", "CODEX_SQLITE_HOME"] {
        if let Some(value) = std::env::var_os(env_name) {
            protected_paths.push((env_name.to_string(), PathBuf::from(value)));
        }
    }
    if let Some(home_dir) = std::env::var_os("HOME") {
        let home_dir = PathBuf::from(home_dir);
        protected_paths.push(("~/.mcodex".to_string(), home_dir.join(".mcodex")));
        protected_paths.push(("~/.codex".to_string(), home_dir.join(".codex")));
    }
    validate_smoke_home_against(home, &protected_paths)
}

fn validate_smoke_home_against(home: &Path, protected_paths: &[(String, PathBuf)]) -> Result<()> {
    let target = comparable_path(home);
    for (label, protected_path) in protected_paths {
        if target == comparable_path(protected_path) {
            bail!(
                "refusing to seed smoke fixture into protected home {} ({label}); choose an isolated SMOKE_ROOT path",
                home.display()
            );
        }
    }
    Ok(())
}

fn comparable_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match std::env::current_dir() {
        Ok(current_dir) => current_dir.join(path),
        Err(_) => path.to_path_buf(),
    }
}

async fn seed_multi_pool(runtime: &StateRuntime) -> Result<()> {
    seed_account(runtime, MAIN_ACCOUNT_ID, MAIN_POOL_ID, 0).await?;
    seed_account(runtime, OTHER_ACCOUNT_ID, OTHER_POOL_ID, 0).await
}

async fn seed_observability(runtime: &StateRuntime) -> Result<()> {
    seed_account(runtime, MAIN_ACCOUNT_ID, MAIN_POOL_ID, 0).await?;
    seed_account(runtime, SECOND_MAIN_ACCOUNT_ID, MAIN_POOL_ID, 1).await?;
    runtime
        .acquire_account_lease(MAIN_POOL_ID, "smoke-holder", Duration::seconds(300))
        .await?;
    seed_quota(runtime, SECOND_MAIN_ACCOUNT_ID).await?;
    runtime
        .append_account_pool_event(AccountPoolEventRecord {
            event_id: format!(
                "smoke-quota-observed-{}",
                Utc::now()
                    .timestamp_nanos_opt()
                    .unwrap_or_else(|| Utc::now().timestamp_millis())
            ),
            occurred_at: Utc::now(),
            pool_id: MAIN_POOL_ID.to_string(),
            account_id: Some(SECOND_MAIN_ACCOUNT_ID.to_string()),
            lease_id: None,
            holder_instance_id: Some("smoke-holder".to_string()),
            event_type: "quotaObserved".to_string(),
            reason_code: Some("quotaNearExhausted".to_string()),
            message: "smoke quota observation".to_string(),
            details_json: Some(serde_json::json!({"fixture": "observability"})),
        })
        .await?;
    Ok(())
}

async fn seed_account(
    runtime: &StateRuntime,
    account_id: &str,
    pool_id: &str,
    position: i64,
) -> Result<()> {
    runtime
        .upsert_registered_account(RegisteredAccountUpsert {
            account_id: account_id.to_string(),
            backend_id: "smoke-local".to_string(),
            backend_family: "chatgpt".to_string(),
            workspace_id: Some("workspace-smoke".to_string()),
            backend_account_handle: account_id.to_string(),
            account_kind: "chatgpt".to_string(),
            provider_fingerprint: format!("smoke:{account_id}"),
            display_name: Some(format!("Smoke {account_id}")),
            source: None,
            enabled: true,
            healthy: true,
            membership: Some(RegisteredAccountMembership {
                pool_id: pool_id.to_string(),
                position,
            }),
        })
        .await?;
    Ok(())
}

async fn write_startup_default(runtime: &StateRuntime, pool_id: &str) -> Result<()> {
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some(pool_id.to_string()),
            preferred_account_id: None,
            suppressed: false,
        })
        .await
}

async fn write_config_default(home: &Path, pool_id: &str) -> Result<()> {
    tokio::fs::write(
        home.join("config.toml"),
        format!(
            r#"[accounts]
default_pool = "{pool_id}"

[accounts.pools.team-main]
allow_context_reuse = false

[accounts.pools.team-other]
allow_context_reuse = false
"#,
        ),
    )
    .await?;
    Ok(())
}

async fn seed_quota(runtime: &StateRuntime, account_id: &str) -> Result<()> {
    let now = Utc::now();
    runtime
        .upsert_account_quota_state(AccountQuotaStateRecord {
            account_id: account_id.to_string(),
            limit_id: "chatgpt".to_string(),
            primary_used_percent: Some(100.0),
            primary_resets_at: Some(now + Duration::minutes(30)),
            secondary_used_percent: None,
            secondary_resets_at: None,
            observed_at: now,
            exhausted_windows: QuotaExhaustedWindows::Unknown,
            predicted_blocked_until: Some(now + Duration::minutes(30)),
            next_probe_after: Some(now + Duration::minutes(10)),
            probe_backoff_level: 1,
            last_probe_result: Some(QuotaProbeResult::StillBlocked),
        })
        .await?;
    Ok(())
}

fn summary_for(home: &Path, scenario: SmokeScenario) -> SmokeFixtureSummary {
    let (pools, accounts, credentials) = match scenario {
        SmokeScenario::Empty => (vec![], vec![], "absent"),
        SmokeScenario::SinglePool
        | SmokeScenario::InvalidPersistedDefault
        | SmokeScenario::InvalidConfigDefault => (
            vec![MAIN_POOL_ID.to_string()],
            vec![MAIN_ACCOUNT_ID.to_string()],
            "fake",
        ),
        SmokeScenario::MultiPool
        | SmokeScenario::PersistedDefault
        | SmokeScenario::ConfigDefaultConflict => (
            vec![MAIN_POOL_ID.to_string(), OTHER_POOL_ID.to_string()],
            vec![MAIN_ACCOUNT_ID.to_string(), OTHER_ACCOUNT_ID.to_string()],
            "fake",
        ),
        SmokeScenario::Observability => (
            vec![MAIN_POOL_ID.to_string()],
            vec![
                MAIN_ACCOUNT_ID.to_string(),
                SECOND_MAIN_ACCOUNT_ID.to_string(),
            ],
            "fake",
        ),
    };

    SmokeFixtureSummary {
        home: home.display().to_string(),
        scenario: scenario.to_string(),
        pools,
        accounts,
        credentials,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ValueEnum;
    use codex_state::AccountPoolAccountsListQuery;
    use codex_state::AccountPoolEventsListQuery;
    use codex_state::AccountStartupAvailability;
    use codex_state::AccountStartupResolutionIssueKind;
    use codex_state::AccountStartupResolutionIssueSource;
    use codex_state::EffectivePoolResolutionSource;
    use pretty_assertions::assert_eq;

    #[test]
    fn exposes_runbook_scenario_names() {
        let scenarios = [
            ("empty", SmokeScenario::Empty),
            ("single-pool", SmokeScenario::SinglePool),
            ("multi-pool", SmokeScenario::MultiPool),
            ("persisted-default", SmokeScenario::PersistedDefault),
            (
                "config-default-conflict",
                SmokeScenario::ConfigDefaultConflict,
            ),
            (
                "invalid-persisted-default",
                SmokeScenario::InvalidPersistedDefault,
            ),
            (
                "invalid-config-default",
                SmokeScenario::InvalidConfigDefault,
            ),
            ("observability", SmokeScenario::Observability),
        ];

        for (name, scenario) in scenarios {
            assert_eq!(
                SmokeScenario::from_str(name, /*ignore_case*/ true).unwrap(),
                scenario
            );
            assert_eq!(scenario.to_string(), name);
        }
    }

    #[test]
    fn validate_smoke_home_rejects_protected_paths() -> anyhow::Result<()> {
        let home = tempfile::tempdir()?;
        let protected_paths = vec![("MCODEX_HOME".to_string(), home.path().to_path_buf())];

        let err =
            validate_smoke_home_against(home.path(), &protected_paths).expect_err("protected path");

        assert!(err.to_string().contains("MCODEX_HOME"));
        assert!(err.to_string().contains("refusing to seed smoke fixture"));
        Ok(())
    }

    #[tokio::test]
    async fn seed_cli_outputs_json_and_writes_fixture_state() -> anyhow::Result<()> {
        let home = tempfile::tempdir()?;
        let output =
            std::process::Command::new(codex_utils_cargo_bin::cargo_bin("mcodex-smoke-fixture")?)
                .env_remove("MCODEX_HOME")
                .env_remove("CODEX_HOME")
                .env_remove("CODEX_SQLITE_HOME")
                .arg("seed")
                .arg("--home")
                .arg(home.path())
                .arg("--scenario")
                .arg("config-default-conflict")
                .arg("--json")
                .output()?;

        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let summary: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(summary["scenario"], "config-default-conflict");
        assert_eq!(
            summary["pools"],
            serde_json::json!(["team-main", "team-other"])
        );
        assert_eq!(summary["credentials"], "fake");

        let config = std::fs::read_to_string(home.path().join("config.toml"))?;
        assert!(config.contains("default_pool = \"team-main\""));
        let runtime = runtime(home.path()).await?;
        let status = runtime
            .read_account_startup_status(Some(MAIN_POOL_ID))
            .await?;
        assert_eq!(
            status.effective_pool_resolution_source,
            EffectivePoolResolutionSource::ConfigDefault
        );
        assert_eq!(
            status.persisted_default_pool_id.as_deref(),
            Some(OTHER_POOL_ID)
        );
        Ok(())
    }

    #[tokio::test]
    async fn seed_empty_fixture_reports_no_pool() -> anyhow::Result<()> {
        let home = tempfile::tempdir()?;

        let summary = seed_fixture(home.path(), SmokeScenario::Empty).await?;

        let runtime = runtime(home.path()).await?;
        let status = runtime.read_account_startup_status(None).await?;
        assert_eq!(summary.credentials, "absent");
        assert_eq!(summary.pools, Vec::<String>::new());
        assert_eq!(status.preview.effective_pool_id, None);
        assert_eq!(
            status.startup_availability,
            AccountStartupAvailability::Unavailable
        );
        Ok(())
    }

    #[tokio::test]
    async fn seed_single_pool_fixture_reports_single_visible_pool() -> anyhow::Result<()> {
        let home = tempfile::tempdir()?;

        let summary = seed_fixture(home.path(), SmokeScenario::SinglePool).await?;

        let runtime = runtime(home.path()).await?;
        let status = runtime.read_account_startup_status(None).await?;
        assert_eq!(summary.scenario, "single-pool");
        assert_eq!(
            status.effective_pool_resolution_source,
            EffectivePoolResolutionSource::SingleVisiblePool
        );
        assert_eq!(
            status.preview.effective_pool_id.as_deref(),
            Some(MAIN_POOL_ID)
        );
        assert_eq!(
            status.preview.predicted_account_id.as_deref(),
            Some(MAIN_ACCOUNT_ID)
        );
        assert_eq!(status.candidate_pools.len(), 1);
        assert_eq!(
            status.startup_availability,
            AccountStartupAvailability::Available
        );
        Ok(())
    }

    #[tokio::test]
    async fn seed_multi_pool_fixture_requires_default() -> anyhow::Result<()> {
        let home = tempfile::tempdir()?;

        seed_fixture(home.path(), SmokeScenario::MultiPool).await?;

        let runtime = runtime(home.path()).await?;
        let status = runtime.read_account_startup_status(None).await?;
        assert_eq!(
            status.startup_availability,
            AccountStartupAvailability::MultiplePoolsRequireDefault
        );
        assert_eq!(status.preview.effective_pool_id, None);
        assert_eq!(
            status
                .startup_resolution_issue
                .as_ref()
                .map(|issue| issue.kind),
            Some(AccountStartupResolutionIssueKind::MultiplePoolsRequireDefault)
        );
        assert_eq!(status.candidate_pools.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn seed_persisted_default_fixture_selects_default_pool() -> anyhow::Result<()> {
        let home = tempfile::tempdir()?;

        seed_fixture(home.path(), SmokeScenario::PersistedDefault).await?;

        let runtime = runtime(home.path()).await?;
        let status = runtime.read_account_startup_status(None).await?;
        assert_eq!(
            status.effective_pool_resolution_source,
            EffectivePoolResolutionSource::PersistedSelection
        );
        assert_eq!(
            status.preview.effective_pool_id.as_deref(),
            Some(MAIN_POOL_ID)
        );
        assert_eq!(
            status.persisted_default_pool_id.as_deref(),
            Some(MAIN_POOL_ID)
        );
        Ok(())
    }

    #[tokio::test]
    async fn seed_config_default_conflict_writes_config_and_persisted_defaults()
    -> anyhow::Result<()> {
        let home = tempfile::tempdir()?;

        seed_fixture(home.path(), SmokeScenario::ConfigDefaultConflict).await?;

        let config = std::fs::read_to_string(home.path().join("config.toml"))?;
        assert!(config.contains("default_pool = \"team-main\""));
        let runtime = runtime(home.path()).await?;
        let status = runtime
            .read_account_startup_status(Some(MAIN_POOL_ID))
            .await?;
        assert_eq!(
            status.effective_pool_resolution_source,
            EffectivePoolResolutionSource::ConfigDefault
        );
        assert_eq!(
            status.preview.effective_pool_id.as_deref(),
            Some(MAIN_POOL_ID)
        );
        assert_eq!(
            status.persisted_default_pool_id.as_deref(),
            Some(OTHER_POOL_ID)
        );
        assert_eq!(
            status.configured_default_pool_id.as_deref(),
            Some(MAIN_POOL_ID)
        );
        Ok(())
    }

    #[tokio::test]
    async fn seed_invalid_persisted_default_reports_persisted_issue() -> anyhow::Result<()> {
        let home = tempfile::tempdir()?;

        seed_fixture(home.path(), SmokeScenario::InvalidPersistedDefault).await?;

        let runtime = runtime(home.path()).await?;
        let status = runtime.read_account_startup_status(None).await?;
        let issue = status.startup_resolution_issue.expect("startup issue");
        assert_eq!(
            status.startup_availability,
            AccountStartupAvailability::InvalidExplicitDefault
        );
        assert_eq!(status.preview.effective_pool_id, None);
        assert_eq!(
            issue.kind,
            AccountStartupResolutionIssueKind::PersistedDefaultPoolUnavailable
        );
        assert_eq!(
            issue.source,
            AccountStartupResolutionIssueSource::PersistedSelection
        );
        assert_eq!(issue.pool_id.as_deref(), Some(MISSING_POOL_ID));
        assert_eq!(
            status.persisted_default_pool_id.as_deref(),
            Some(MISSING_POOL_ID)
        );
        Ok(())
    }

    #[tokio::test]
    async fn seed_invalid_config_default_reports_config_issue() -> anyhow::Result<()> {
        let home = tempfile::tempdir()?;

        seed_fixture(home.path(), SmokeScenario::InvalidConfigDefault).await?;

        let config = std::fs::read_to_string(home.path().join("config.toml"))?;
        assert!(config.contains("default_pool = \"missing-pool\""));
        let runtime = runtime(home.path()).await?;
        let status = runtime
            .read_account_startup_status(Some(MISSING_POOL_ID))
            .await?;
        let issue = status.startup_resolution_issue.expect("startup issue");
        assert_eq!(
            status.startup_availability,
            AccountStartupAvailability::InvalidExplicitDefault
        );
        assert_eq!(status.preview.effective_pool_id, None);
        assert_eq!(
            issue.kind,
            AccountStartupResolutionIssueKind::ConfigDefaultPoolUnavailable
        );
        assert_eq!(
            issue.source,
            AccountStartupResolutionIssueSource::ConfigDefault
        );
        assert_eq!(issue.pool_id.as_deref(), Some(MISSING_POOL_ID));
        assert_eq!(
            status.configured_default_pool_id.as_deref(),
            Some(MISSING_POOL_ID)
        );
        Ok(())
    }

    #[tokio::test]
    async fn seed_observability_fixture_reports_lease_quota_and_event() -> anyhow::Result<()> {
        let home = tempfile::tempdir()?;

        seed_fixture(home.path(), SmokeScenario::Observability).await?;

        let runtime = runtime(home.path()).await?;
        let snapshot = runtime.read_account_pool_snapshot(MAIN_POOL_ID).await?;
        assert_eq!(snapshot.summary.total_accounts, 2);
        assert_eq!(snapshot.summary.active_leases, 1);

        let diagnostics = runtime.read_account_pool_diagnostics(MAIN_POOL_ID).await?;
        assert_eq!(diagnostics.status, "degraded");
        assert!(
            diagnostics
                .issues
                .iter()
                .any(|issue| issue.reason_code == "cooldownActive")
        );

        let accounts = runtime
            .list_account_pool_accounts(AccountPoolAccountsListQuery {
                pool_id: MAIN_POOL_ID.to_string(),
                account_id: Some(SECOND_MAIN_ACCOUNT_ID.to_string()),
                cursor: None,
                limit: Some(10),
                states: None,
                account_kinds: None,
            })
            .await?;
        assert_eq!(
            accounts.data[0].operational_state.as_deref(),
            Some("coolingDown")
        );

        let events = runtime
            .list_account_pool_events(AccountPoolEventsListQuery {
                pool_id: MAIN_POOL_ID.to_string(),
                account_id: None,
                types: Some(vec!["quotaObserved".to_string()]),
                cursor: None,
                limit: Some(1),
            })
            .await?;
        assert_eq!(events.data[0].event_type, "quotaObserved");
        assert_eq!(
            events.data[0]
                .details_json
                .as_ref()
                .and_then(|details| details.get("fixture"))
                .and_then(serde_json::Value::as_str),
            Some("observability")
        );
        Ok(())
    }

    async fn runtime(home: &Path) -> anyhow::Result<std::sync::Arc<StateRuntime>> {
        StateRuntime::init(home.to_path_buf(), "codex".to_string()).await
    }
}
