use super::*;
use codex_config::config_toml::ConfigToml;
use codex_config::types::AccountAllocationModeToml;
use codex_protocol::ThreadId;
use codex_protocol::config_types::Personality;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::RolloutLine;
use codex_protocol::protocol::SessionMeta;
use codex_protocol::protocol::SessionMetaLine;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::UserMessageEvent;
use pretty_assertions::assert_eq;
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use tempfile::TempDir;

const TEST_TIMESTAMP: &str = "2025-01-01T00-00-00";

#[derive(Debug, Default)]
struct StubUi {
    should_migrate: bool,
    prompt_count: usize,
}

impl StubUi {
    fn accepting() -> Self {
        Self {
            should_migrate: true,
            prompt_count: 0,
        }
    }

    fn declining() -> Self {
        Self {
            should_migrate: false,
            prompt_count: 0,
        }
    }
}

impl ProductIdentityMigrationUi for StubUi {
    fn should_migrate_product_identity(
        &mut self,
        _legacy_home: &Path,
        _mcodex_home: &Path,
    ) -> io::Result<bool> {
        self.prompt_count += 1;
        Ok(self.should_migrate)
    }
}

fn write_legacy_config(legacy_home: &Path) -> io::Result<()> {
    fs::write(
        legacy_home.join("config.toml"),
        r#"
[accounts]
default_pool = "legacy-default"
allocation_mode = "exclusive"

[accounts.pools.legacy-default]
allow_context_reuse = true
"#,
    )
}

fn write_legacy_config_with_personality(
    legacy_home: &Path,
    personality: Personality,
) -> io::Result<()> {
    fs::write(
        legacy_home.join("config.toml"),
        format!(
            r#"
personality = "{}"

[accounts]
default_pool = "legacy-default"
allocation_mode = "exclusive"

[accounts.pools.legacy-default]
allow_context_reuse = true
"#,
            match personality {
                Personality::None => "none",
                Personality::Friendly => "friendly",
                Personality::Pragmatic => "pragmatic",
            }
        ),
    )
}

fn write_legacy_auth(legacy_home: &Path) -> io::Result<()> {
    fs::write(
        legacy_home.join("auth.json"),
        r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":"sk-legacy"}"#,
    )
}

fn read_config(path: &Path) -> io::Result<ConfigToml> {
    let contents = fs::read_to_string(path.join("config.toml"))?;
    toml::from_str(&contents).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn some_legacy_home(legacy_home: &TempDir) -> io::Result<Option<PathBuf>> {
    Ok(Some(legacy_home.path().to_path_buf()))
}

fn write_session_with_user_event(codex_home: &Path) -> io::Result<()> {
    let thread_id = ThreadId::new();
    let dir = codex_home
        .join(crate::SESSIONS_SUBDIR)
        .join("2025")
        .join("01")
        .join("01");
    fs::create_dir_all(&dir)?;
    let file_path = dir.join(format!("rollout-{TEST_TIMESTAMP}-{thread_id}.jsonl"));

    let session_meta = SessionMetaLine {
        meta: SessionMeta {
            id: thread_id,
            forked_from_id: None,
            timestamp: TEST_TIMESTAMP.to_string(),
            cwd: PathBuf::from("."),
            originator: "test_originator".to_string(),
            cli_version: "test_version".to_string(),
            source: SessionSource::Cli,
            agent_path: None,
            agent_nickname: None,
            agent_role: None,
            model_provider: None,
            base_instructions: None,
            dynamic_tools: None,
            memory_mode: None,
        },
        git: None,
    };
    let meta_line = RolloutLine {
        timestamp: TEST_TIMESTAMP.to_string(),
        item: RolloutItem::SessionMeta(session_meta),
    };
    let user_event = RolloutLine {
        timestamp: TEST_TIMESTAMP.to_string(),
        item: RolloutItem::EventMsg(EventMsg::UserMessage(UserMessageEvent {
            message: "hello".to_string(),
            images: None,
            local_images: Vec::new(),
            text_elements: Vec::new(),
        })),
    };
    let contents = format!(
        "{}\n{}\n",
        serde_json::to_string(&meta_line).map_err(io::Error::other)?,
        serde_json::to_string(&user_event).map_err(io::Error::other)?
    );
    fs::write(file_path, contents)?;
    Ok(())
}

#[tokio::test]
async fn config_transform_drops_startup_selection_fields() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_config(legacy_home.path())?;

    let mut ui = StubUi::accepting();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        some_legacy_home(&legacy_home),
        &mut ui,
    )
    .await?;

    assert_eq!(outcome.status, ProductIdentityMigrationStatus::Imported);
    assert_eq!(outcome.config_import, MigrationImportOutcome::Imported);
    let migrated = read_config(active_home.path())?;
    let accounts = migrated.accounts.expect("accounts should be preserved");
    assert_eq!(accounts.default_pool, None);
    assert_eq!(
        accounts.allocation_mode,
        Some(AccountAllocationModeToml::Exclusive)
    );
    Ok(())
}

#[tokio::test]
async fn auth_import_failure_records_warning_without_blocking_migration() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_config(legacy_home.path())?;
    write_legacy_auth(legacy_home.path())?;
    fs::create_dir_all(active_home.path().join("auth.json"))?;

    let mut ui = StubUi::accepting();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        some_legacy_home(&legacy_home),
        &mut ui,
    )
    .await?;

    assert_eq!(
        outcome.status,
        ProductIdentityMigrationStatus::ImportedWithWarnings
    );
    assert_eq!(outcome.config_import, MigrationImportOutcome::Imported);
    match outcome.auth_import {
        MigrationImportOutcome::Failed { warning } => {
            assert!(
                warning.contains("auth.json"),
                "unexpected warning: {warning}"
            );
        }
        other => panic!("unexpected auth import outcome: {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn does_not_import_legacy_skills_or_plugin_cache_by_default() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_config(legacy_home.path())?;
    write_legacy_auth(legacy_home.path())?;
    fs::create_dir_all(legacy_home.path().join("skills/sample"))?;
    fs::create_dir_all(legacy_home.path().join("plugins/cache"))?;
    fs::create_dir_all(legacy_home.path().join("marketplace/cache"))?;
    fs::create_dir_all(legacy_home.path().join("logs"))?;
    fs::create_dir_all(legacy_home.path().join("sessions/2025/01/01"))?;
    fs::write(legacy_home.path().join("state.db"), "sqlite")?;

    let mut ui = StubUi::accepting();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        some_legacy_home(&legacy_home),
        &mut ui,
    )
    .await?;

    assert_eq!(outcome.status, ProductIdentityMigrationStatus::Imported);
    assert!(active_home.path().join("config.toml").exists());
    assert!(active_home.path().join("auth.json").exists());
    assert!(!active_home.path().join("skills").exists());
    assert!(!active_home.path().join("plugins").exists());
    assert!(!active_home.path().join("marketplace").exists());
    assert!(!active_home.path().join("logs").exists());
    assert!(!active_home.path().join("sessions").exists());
    assert!(!active_home.path().join("state.db").exists());
    Ok(())
}

#[tokio::test]
async fn skips_when_marker_exists() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    fs::write(
        active_home.path().join(PRODUCT_IDENTITY_MIGRATION_FILENAME),
        "v1\n",
    )?;
    write_legacy_config(legacy_home.path())?;
    write_legacy_auth(legacy_home.path())?;

    let mut ui = StubUi::accepting();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        some_legacy_home(&legacy_home),
        &mut ui,
    )
    .await?;

    assert_eq!(ui.prompt_count, 0);
    assert_eq!(
        outcome.status,
        ProductIdentityMigrationStatus::SkippedMarker
    );
    assert_eq!(outcome.config_import, MigrationImportOutcome::NotAttempted);
    assert_eq!(outcome.auth_import, MigrationImportOutcome::NotAttempted);
    Ok(())
}

#[tokio::test]
async fn skips_when_active_home_is_already_initialized() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_config(legacy_home.path())?;
    write_legacy_auth(legacy_home.path())?;
    fs::write(
        active_home.path().join("auth.json"),
        r#"{"auth_mode":"chatgpt"}"#,
    )?;

    let mut ui = StubUi::accepting();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        some_legacy_home(&legacy_home),
        &mut ui,
    )
    .await?;

    assert_eq!(ui.prompt_count, 0);
    assert_eq!(
        outcome.status,
        ProductIdentityMigrationStatus::SkippedInitializedHome
    );
    assert_eq!(outcome.config_import, MigrationImportOutcome::NotAttempted);
    assert_eq!(outcome.auth_import, MigrationImportOutcome::NotAttempted);
    assert!(
        active_home
            .path()
            .join(PRODUCT_IDENTITY_MIGRATION_FILENAME)
            .exists()
    );
    assert_eq!(
        fs::read_to_string(active_home.path().join("auth.json"))?,
        r#"{"auth_mode":"chatgpt"}"#
    );
    Ok(())
}

#[tokio::test]
async fn pending_migration_resumes_auth_import_without_prompting() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_config(legacy_home.path())?;
    write_legacy_auth(legacy_home.path())?;
    fs::write(
        active_home
            .path()
            .join(PRODUCT_IDENTITY_MIGRATION_PENDING_FILENAME),
        "v1\n",
    )?;
    fs::write(
        active_home.path().join("config.toml"),
        "model = \"already-imported\"\n",
    )?;

    let mut ui = StubUi::accepting();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        some_legacy_home(&legacy_home),
        &mut ui,
    )
    .await?;

    assert_eq!(ui.prompt_count, 0);
    assert_eq!(outcome.status, ProductIdentityMigrationStatus::Imported);
    assert_eq!(
        outcome.config_import,
        MigrationImportOutcome::AlreadyPresent
    );
    assert_eq!(outcome.auth_import, MigrationImportOutcome::Imported);
    assert!(
        active_home
            .path()
            .join(PRODUCT_IDENTITY_MIGRATION_FILENAME)
            .exists()
    );
    assert!(
        !active_home
            .path()
            .join(PRODUCT_IDENTITY_MIGRATION_PENDING_FILENAME)
            .exists()
    );
    assert_eq!(
        fs::read_to_string(active_home.path().join("config.toml"))?,
        "model = \"already-imported\"\n"
    );
    assert_eq!(
        fs::read_to_string(active_home.path().join("auth.json"))?,
        r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":"sk-legacy"}"#
    );
    Ok(())
}

#[tokio::test]
async fn pending_migration_resumes_config_import_without_prompting() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_config(legacy_home.path())?;
    write_legacy_auth(legacy_home.path())?;
    fs::write(
        active_home
            .path()
            .join(PRODUCT_IDENTITY_MIGRATION_PENDING_FILENAME),
        "v1\n",
    )?;
    fs::write(
        active_home.path().join("auth.json"),
        r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":"sk-already"}"#,
    )?;

    let mut ui = StubUi::accepting();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        some_legacy_home(&legacy_home),
        &mut ui,
    )
    .await?;

    assert_eq!(ui.prompt_count, 0);
    assert_eq!(outcome.status, ProductIdentityMigrationStatus::Imported);
    assert_eq!(outcome.config_import, MigrationImportOutcome::Imported);
    assert_eq!(outcome.auth_import, MigrationImportOutcome::AlreadyPresent);
    assert!(
        active_home
            .path()
            .join(PRODUCT_IDENTITY_MIGRATION_FILENAME)
            .exists()
    );
    assert!(
        !active_home
            .path()
            .join(PRODUCT_IDENTITY_MIGRATION_PENDING_FILENAME)
            .exists()
    );
    assert_eq!(
        fs::read_to_string(active_home.path().join("auth.json"))?,
        r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":"sk-already"}"#
    );
    let migrated = read_config(active_home.path())?;
    let accounts = migrated.accounts.expect("accounts should be preserved");
    assert_eq!(accounts.default_pool, None);
    Ok(())
}

#[tokio::test]
async fn skips_when_no_legacy_home() -> io::Result<()> {
    let active_home = TempDir::new()?;

    let mut ui = StubUi::accepting();
    let outcome =
        maybe_migrate_product_identity_with_legacy_home(active_home.path(), Ok(None), &mut ui)
            .await?;

    assert_eq!(ui.prompt_count, 0);
    assert_eq!(
        outcome.status,
        ProductIdentityMigrationStatus::SkippedNoLegacyHome
    );
    assert_eq!(outcome.config_import, MigrationImportOutcome::NotAttempted);
    assert_eq!(outcome.auth_import, MigrationImportOutcome::NotAttempted);
    Ok(())
}

#[tokio::test]
async fn skips_unreadable_legacy_home_without_prompting() -> io::Result<()> {
    let active_home = TempDir::new()?;

    let mut ui = StubUi::accepting();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        Err(io::Error::other("legacy home is unreadable")),
        &mut ui,
    )
    .await?;

    assert_eq!(ui.prompt_count, 0);
    assert_eq!(
        outcome.status,
        ProductIdentityMigrationStatus::SkippedUnreadableLegacyHome
    );
    assert_eq!(outcome.config_import, MigrationImportOutcome::NotAttempted);
    assert_eq!(outcome.auth_import, MigrationImportOutcome::NotAttempted);
    Ok(())
}

#[tokio::test]
async fn skips_when_user_declines_migration() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_config(legacy_home.path())?;
    write_legacy_auth(legacy_home.path())?;

    let mut ui = StubUi::declining();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        some_legacy_home(&legacy_home),
        &mut ui,
    )
    .await?;

    assert_eq!(ui.prompt_count, 1);
    assert_eq!(
        outcome.status,
        ProductIdentityMigrationStatus::SkippedByUser
    );
    assert_eq!(outcome.config_import, MigrationImportOutcome::NotAttempted);
    assert_eq!(outcome.auth_import, MigrationImportOutcome::NotAttempted);
    assert!(
        active_home
            .path()
            .join(PRODUCT_IDENTITY_MIGRATION_FILENAME)
            .exists()
    );
    assert!(!active_home.path().join("config.toml").exists());
    assert!(!active_home.path().join("auth.json").exists());
    Ok(())
}

#[tokio::test]
async fn marker_write_failure_is_reported_as_warning() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_config(legacy_home.path())?;
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(active_home.path())?.permissions();
        permissions.set_mode(0o555);
        fs::set_permissions(active_home.path(), permissions)?;
    }

    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(active_home.path())?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(active_home.path(), permissions)?;
    }

    let mut ui = StubUi::declining();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        some_legacy_home(&legacy_home),
        &mut ui,
    )
    .await?;

    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(active_home.path())?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(active_home.path(), permissions)?;
    }

    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(active_home.path())?.permissions();
        permissions.set_readonly(false);
        fs::set_permissions(active_home.path(), permissions)?;
    }

    assert_eq!(
        outcome.status,
        ProductIdentityMigrationStatus::SkippedByUser
    );
    assert!(outcome.marker_warning.is_some());
    Ok(())
}

#[tokio::test]
async fn product_migration_marker_does_not_suppress_personality_migration() -> io::Result<()> {
    let active_home = TempDir::new()?;
    fs::write(
        active_home.path().join(PRODUCT_IDENTITY_MIGRATION_FILENAME),
        "v1\n",
    )?;
    write_session_with_user_event(active_home.path())?;

    let mut ui = StubUi::accepting();
    let outcome = run_startup_migrations_with_legacy_home(
        active_home.path(),
        &ConfigToml::default(),
        Ok(None),
        &mut ui,
    )
    .await?;

    assert_eq!(
        outcome.product_identity,
        ProductIdentityMigrationStatus::SkippedMarker
    );
    assert_eq!(
        outcome.personality,
        crate::personality_migration::PersonalityMigrationStatus::Applied
    );
    let migrated = read_config(active_home.path())?;
    assert_eq!(migrated.personality, Some(Personality::Pragmatic));
    Ok(())
}

#[tokio::test]
async fn personality_marker_does_not_suppress_product_migration() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_config(legacy_home.path())?;
    write_legacy_auth(legacy_home.path())?;
    fs::write(
        active_home
            .path()
            .join(crate::personality_migration::PERSONALITY_MIGRATION_FILENAME),
        "v1\n",
    )?;

    let mut ui = StubUi::accepting();
    let outcome = run_startup_migrations_with_legacy_home(
        active_home.path(),
        &ConfigToml::default(),
        some_legacy_home(&legacy_home),
        &mut ui,
    )
    .await?;

    assert_eq!(
        outcome.product_identity,
        ProductIdentityMigrationStatus::Imported
    );
    assert_eq!(
        outcome.personality,
        crate::personality_migration::PersonalityMigrationStatus::SkippedMarker
    );
    assert!(active_home.path().join("config.toml").exists());
    assert!(active_home.path().join("auth.json").exists());
    Ok(())
}

#[tokio::test]
async fn imported_explicit_personality_suppresses_personality_migration() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_config_with_personality(legacy_home.path(), Personality::Friendly)?;
    write_legacy_auth(legacy_home.path())?;
    write_session_with_user_event(active_home.path())?;

    let mut ui = StubUi::accepting();
    let outcome = run_startup_migrations_with_loader_and_legacy_home(
        active_home.path(),
        some_legacy_home(&legacy_home),
        &mut ui,
        || async { read_config(active_home.path()) },
    )
    .await?;

    assert_eq!(
        outcome.product_identity,
        ProductIdentityMigrationStatus::Imported
    );
    assert_eq!(
        outcome.personality,
        crate::personality_migration::PersonalityMigrationStatus::SkippedExplicitPersonality
    );
    let migrated = read_config(active_home.path())?;
    assert_eq!(migrated.personality, Some(Personality::Friendly));
    Ok(())
}

#[tokio::test]
async fn imports_config_and_auth_when_user_accepts() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_config(legacy_home.path())?;
    write_legacy_auth(legacy_home.path())?;

    let mut ui = StubUi::accepting();
    let outcome = maybe_migrate_product_identity_with_legacy_home(
        active_home.path(),
        some_legacy_home(&legacy_home),
        &mut ui,
    )
    .await?;

    assert_eq!(ui.prompt_count, 1);
    assert_eq!(outcome.status, ProductIdentityMigrationStatus::Imported);
    assert_eq!(outcome.config_import, MigrationImportOutcome::Imported);
    assert_eq!(outcome.auth_import, MigrationImportOutcome::Imported);
    assert!(outcome.marker_warning.is_none());
    assert!(active_home.path().join("config.toml").exists());
    assert!(active_home.path().join("auth.json").exists());
    assert!(
        active_home
            .path()
            .join(PRODUCT_IDENTITY_MIGRATION_FILENAME)
            .exists()
    );
    Ok(())
}

#[tokio::test]
async fn import_config_refuses_to_overwrite_existing_target() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_config(legacy_home.path())?;
    fs::write(
        active_home.path().join("config.toml"),
        "model = \"keep-me\"\n",
    )?;

    let outcome = import_config(legacy_home.path(), active_home.path()).await;

    match outcome {
        MigrationImportOutcome::Failed { warning } => {
            assert!(
                warning.contains("refusing to overwrite existing mcodex config.toml"),
                "unexpected warning: {warning}"
            );
        }
        other => panic!("unexpected config import outcome: {other:?}"),
    }
    assert_eq!(
        fs::read_to_string(active_home.path().join("config.toml"))?,
        "model = \"keep-me\"\n"
    );
    Ok(())
}

#[tokio::test]
async fn import_auth_refuses_to_overwrite_existing_target() -> io::Result<()> {
    let legacy_home = TempDir::new()?;
    let active_home = TempDir::new()?;
    write_legacy_auth(legacy_home.path())?;
    fs::write(active_home.path().join("auth.json"), r#"{"keep":"me"}"#)?;

    let outcome = import_auth(legacy_home.path(), active_home.path()).await;

    match outcome {
        MigrationImportOutcome::Failed { warning } => {
            assert!(
                warning.contains("refusing to overwrite existing mcodex auth.json"),
                "unexpected warning: {warning}"
            );
        }
        other => panic!("unexpected auth import outcome: {other:?}"),
    }
    assert_eq!(
        fs::read_to_string(active_home.path().join("auth.json"))?,
        r#"{"keep":"me"}"#
    );
    Ok(())
}
