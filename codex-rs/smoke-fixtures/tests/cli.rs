use codex_state::EffectivePoolResolutionSource;
use codex_state::StateRuntime;
use pretty_assertions::assert_eq;

const MAIN_POOL_ID: &str = "team-main";
const OTHER_POOL_ID: &str = "team-other";

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
    let runtime = StateRuntime::init(home.path().to_path_buf(), "codex".to_string()).await?;
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
