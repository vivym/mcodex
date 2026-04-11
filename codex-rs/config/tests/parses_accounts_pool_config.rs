use codex_config::config_toml::ConfigToml;
use pretty_assertions::assert_eq;

#[test]
fn parses_accounts_pool_config() {
    let cfg: ConfigToml = toml::from_str(
        r#"
[accounts]
backend = "local"
default_pool = "team-main"
proactive_switch_threshold_percent = 85
allocation_mode = "exclusive"

[accounts.pools.team-main]
allow_context_reuse = false
account_kinds = ["chatgpt"]
"#,
    )
    .unwrap();

    assert_eq!(
        cfg.accounts.unwrap().default_pool.as_deref(),
        Some("team-main")
    );
}
