use super::*;
use crate::config_toml::ConfigToml;
use pretty_assertions::assert_eq;

pub(super) fn assert_parses_accounts_pool_config() {
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

#[test]
fn parses_accounts_pool_config() {
    assert_parses_accounts_pool_config();
}

#[test]
fn deserialize_skill_config_with_name_selector() {
    let cfg: SkillConfig = toml::from_str(
        r#"
            name = "github:yeet"
            enabled = false
        "#,
    )
    .expect("should deserialize skill config with name selector");

    assert_eq!(cfg.name.as_deref(), Some("github:yeet"));
    assert_eq!(cfg.path, None);
    assert!(!cfg.enabled);
}

#[test]
fn deserialize_skill_config_with_path_selector() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let skill_path = tempdir.path().join("skills").join("demo").join("SKILL.md");
    let cfg: SkillConfig = toml::from_str(&format!(
        r#"
            path = {path:?}
            enabled = false
        "#,
        path = skill_path.display().to_string(),
    ))
    .expect("should deserialize skill config with path selector");

    assert_eq!(
        cfg,
        SkillConfig {
            path: Some(
                AbsolutePathBuf::from_absolute_path(&skill_path)
                    .expect("skill path should be absolute"),
            ),
            name: None,
            enabled: false,
        }
    );
}
