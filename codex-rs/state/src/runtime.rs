use crate::AgentJob;
use crate::AgentJobCreateParams;
use crate::AgentJobItem;
use crate::AgentJobItemCreateParams;
use crate::AgentJobItemStatus;
use crate::AgentJobProgress;
use crate::AgentJobStatus;
use crate::LOGS_DB_FILENAME;
use crate::LOGS_DB_VERSION;
use crate::LogEntry;
use crate::LogQuery;
use crate::LogRow;
use crate::STATE_DB_FILENAME;
use crate::STATE_DB_VERSION;
use crate::SortKey;
use crate::ThreadConfigBaselineSnapshot;
use crate::ThreadMetadata;
use crate::ThreadMetadataBuilder;
use crate::ThreadsPage;
use crate::apply_rollout_item;
use crate::migrations::runtime_logs_migrator;
use crate::migrations::runtime_state_migrator;
use crate::model::AgentJobRow;
use crate::model::ThreadConfigBaselineRow;
use crate::model::ThreadRow;
use crate::model::anchor_from_item;
use crate::model::datetime_to_epoch_millis;
use crate::model::datetime_to_epoch_seconds;
use crate::model::epoch_millis_to_datetime;
use crate::paths::file_modified_time_utc;
use chrono::DateTime;
use chrono::Utc;
use codex_protocol::ThreadId;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_protocol::protocol::RolloutItem;
use log::LevelFilter;
use serde_json::Value;
use sqlx::ConnectOptions;
use sqlx::QueryBuilder;
use sqlx::Row;
use sqlx::Sqlite;
use sqlx::SqliteConnection;
use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use sqlx::sqlite::SqliteAutoVacuum;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqliteJournalMode;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::sqlite::SqliteSynchronous;
use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::time::Duration;
use tracing::warn;

mod account_pool;
mod account_pool_control;
mod account_pool_observability;
mod account_pool_quota;
mod agent_jobs;
mod backfill;
mod logs;
mod memories;
mod remote_control;
#[cfg(test)]
mod test_support;
mod thread_config_baselines;
mod threads;

pub use remote_control::RemoteControlEnrollmentRecord;
pub use threads::ThreadFilterOptions;

// "Partition" is the retained-log-content bucket we cap at 10 MiB:
// - one bucket per non-null thread_id
// - one bucket per threadless (thread_id IS NULL) non-null process_uuid
// - one bucket for threadless rows with process_uuid IS NULL
// This budget tracks each row's persisted rendered log body plus non-body
// metadata, rather than the exact sum of all persisted SQLite column bytes.
const LOG_PARTITION_SIZE_LIMIT_BYTES: i64 = 10 * 1024 * 1024;
const LOG_PARTITION_ROW_LIMIT: i64 = 1_000;
const ACCOUNT_LEASES_ACTIVE_HOLDER_INDEX_MIGRATION_VERSION: i64 = 26;
const HISTORICAL_MODIFIED_0026_CHECKSUM_HEX: &str = "C4E09A43E03215676C68CA4F357675BC1555C03C272B41ABBB92B2F71E0F381B0AA207775EE5D9D18DD4A613A216812E";

#[derive(Clone)]
pub struct StateRuntime {
    codex_home: PathBuf,
    default_provider: String,
    pool: Arc<sqlx::SqlitePool>,
    logs_pool: Arc<sqlx::SqlitePool>,
    thread_updated_at_millis: Arc<AtomicI64>,
}

impl StateRuntime {
    /// Initialize the state runtime using the provided Codex home and default provider.
    ///
    /// This opens (and migrates) the SQLite databases under `codex_home`,
    /// keeping logs in a dedicated file to reduce lock contention with the
    /// rest of the state store.
    pub async fn init(codex_home: PathBuf, default_provider: String) -> anyhow::Result<Arc<Self>> {
        tokio::fs::create_dir_all(&codex_home).await?;
        let state_migrator = runtime_state_migrator();
        let logs_migrator = runtime_logs_migrator();
        let current_state_name = state_db_filename();
        let current_logs_name = logs_db_filename();
        let legacy_state_paths =
            find_legacy_db_paths(&codex_home, current_state_name.as_str(), STATE_DB_FILENAME).await;
        remove_legacy_db_files(
            &codex_home,
            current_logs_name.as_str(),
            LOGS_DB_FILENAME,
            "logs",
        )
        .await;
        let state_path = state_db_path(codex_home.as_path());
        let logs_path = logs_db_path(codex_home.as_path());
        let pool = match open_state_sqlite(&state_path, &state_migrator).await {
            Ok(db) => Arc::new(db),
            Err(err) => {
                warn!("failed to open state db at {}: {err}", state_path.display());
                return Err(err);
            }
        };
        let logs_pool = match open_logs_sqlite(&logs_path, &logs_migrator).await {
            Ok(db) => Arc::new(db),
            Err(err) => {
                warn!("failed to open logs db at {}: {err}", logs_path.display());
                return Err(err);
            }
        };
        let thread_updated_at_millis: Option<i64> =
            sqlx::query_scalar("SELECT MAX(threads.updated_at_ms) FROM threads")
                .fetch_one(pool.as_ref())
                .await?;
        let thread_updated_at_millis = thread_updated_at_millis.unwrap_or(0);
        let runtime = Arc::new(Self {
            pool,
            logs_pool,
            codex_home,
            default_provider,
            thread_updated_at_millis: Arc::new(AtomicI64::new(thread_updated_at_millis)),
        });
        let mut imported_legacy_state_path: Option<PathBuf> = None;
        for legacy_state_path in &legacy_state_paths {
            match runtime
                .import_legacy_threads_from_db(legacy_state_path)
                .await
            {
                Ok(()) => {
                    imported_legacy_state_path = Some(legacy_state_path.clone());
                    break;
                }
                Err(err) => {
                    warn!(
                        "failed to import legacy state db threads from {}: {err}",
                        legacy_state_path.display()
                    );
                }
            }
        }
        if let Some(imported_legacy_state_path) = imported_legacy_state_path.as_ref() {
            for suffix in ["", "-wal", "-shm", "-journal"] {
                let mut family_path = imported_legacy_state_path.as_os_str().to_os_string();
                family_path.push(suffix);
                let family_path = PathBuf::from(family_path);
                if let Err(err) = tokio::fs::remove_file(&family_path).await
                    && err.kind() != std::io::ErrorKind::NotFound
                {
                    warn!(
                        "failed to remove imported legacy state db file {}: {err}",
                        family_path.display(),
                    );
                }
            }
        }
        if let Err(err) = runtime.run_logs_startup_maintenance().await {
            warn!(
                "failed to run startup maintenance for logs db at {}: {err}",
                logs_path.display(),
            );
        }
        Ok(runtime)
    }

    /// Return the configured Codex home directory for this runtime.
    pub fn codex_home(&self) -> &Path {
        self.codex_home.as_path()
    }

    async fn import_legacy_threads_from_db(&self, legacy_path: &Path) -> anyhow::Result<()> {
        let legacy_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(legacy_path)
                    .create_if_missing(false)
                    .read_only(true)
                    .busy_timeout(Duration::from_secs(5))
                    .log_statements(LevelFilter::Off),
            )
            .await?;
        let legacy_columns: BTreeSet<String> = sqlx::query("PRAGMA table_info(threads)")
            .fetch_all(&legacy_pool)
            .await?
            .into_iter()
            .map(|row| row.try_get("name"))
            .collect::<std::result::Result<_, _>>()?;
        let select_column = |column: &str, fallback: &str| {
            if legacy_columns.contains(column) {
                column.to_string()
            } else {
                format!("{fallback} AS {column}")
            }
        };
        let query = format!(
            r#"
SELECT
    id,
    rollout_path,
    created_at,
    updated_at,
    source,
    {},
    {},
    {},
    model_provider,
    {},
    {},
    cwd,
    {},
    title,
    sandbox_policy,
    approval_mode,
    tokens_used,
    {},
    {},
    {},
    {},
    {},
    {}
FROM threads
            "#,
            select_column("agent_nickname", "NULL"),
            select_column("agent_role", "NULL"),
            select_column("agent_path", "NULL"),
            select_column("model", "NULL"),
            select_column("reasoning_effort", "NULL"),
            select_column("cli_version", "''"),
            select_column("first_user_message", "''"),
            select_column("archived_at", "NULL"),
            select_column("git_sha", "NULL"),
            select_column("git_branch", "NULL"),
            select_column("git_origin_url", "NULL"),
            select_column("memory_mode", "'enabled'"),
        );
        let rows = sqlx::query(&query).fetch_all(&legacy_pool).await?;
        let legacy_threads: Vec<(ThreadMetadata, String)> = rows
            .into_iter()
            .map(|row| {
                let metadata = ThreadRow::try_from_row(&row).and_then(ThreadMetadata::try_from)?;
                let memory_mode: String = row.try_get("memory_mode")?;
                Ok::<_, anyhow::Error>((metadata, memory_mode))
            })
            .collect::<anyhow::Result<_>>()?;
        let mut tx = self.pool.begin().await?;
        for (metadata, memory_mode) in &legacy_threads {
            self.upsert_imported_thread_with_memory_mode(&mut tx, metadata, memory_mode.as_str())
                .await?;
        }
        tx.commit().await?;
        legacy_pool.close().await;
        Ok(())
    }

    async fn upsert_imported_thread_with_memory_mode(
        &self,
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        metadata: &ThreadMetadata,
        memory_mode: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
INSERT INTO threads (
    id,
    rollout_path,
    created_at,
    updated_at,
    source,
    agent_nickname,
    agent_role,
    agent_path,
    model_provider,
    model,
    reasoning_effort,
    cwd,
    cli_version,
    title,
    sandbox_policy,
    approval_mode,
    tokens_used,
    first_user_message,
    archived,
    archived_at,
    git_sha,
    git_branch,
    git_origin_url,
    memory_mode
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(id) DO UPDATE SET
    rollout_path = excluded.rollout_path,
    created_at = excluded.created_at,
    updated_at = excluded.updated_at,
    source = excluded.source,
    agent_nickname = excluded.agent_nickname,
    agent_role = excluded.agent_role,
    agent_path = excluded.agent_path,
    model_provider = excluded.model_provider,
    model = excluded.model,
    reasoning_effort = excluded.reasoning_effort,
    cwd = excluded.cwd,
    cli_version = excluded.cli_version,
    title = excluded.title,
    sandbox_policy = excluded.sandbox_policy,
    approval_mode = excluded.approval_mode,
    tokens_used = excluded.tokens_used,
    first_user_message = excluded.first_user_message,
    archived = excluded.archived,
    archived_at = excluded.archived_at,
    git_sha = excluded.git_sha,
    git_branch = excluded.git_branch,
    git_origin_url = excluded.git_origin_url,
    memory_mode = excluded.memory_mode
            "#,
        )
        .bind(metadata.id.to_string())
        .bind(metadata.rollout_path.display().to_string())
        .bind(datetime_to_epoch_seconds(metadata.created_at))
        .bind(datetime_to_epoch_seconds(metadata.updated_at))
        .bind(metadata.source.as_str())
        .bind(metadata.agent_nickname.as_deref())
        .bind(metadata.agent_role.as_deref())
        .bind(metadata.agent_path.as_deref())
        .bind(metadata.model_provider.as_str())
        .bind(metadata.model.as_deref())
        .bind(
            metadata
                .reasoning_effort
                .as_ref()
                .map(crate::extract::enum_to_string),
        )
        .bind(metadata.cwd.display().to_string())
        .bind(metadata.cli_version.as_str())
        .bind(metadata.title.as_str())
        .bind(metadata.sandbox_policy.as_str())
        .bind(metadata.approval_mode.as_str())
        .bind(metadata.tokens_used)
        .bind(metadata.first_user_message.as_deref().unwrap_or_default())
        .bind(metadata.archived_at.is_some())
        .bind(metadata.archived_at.map(datetime_to_epoch_seconds))
        .bind(metadata.git_sha.as_deref())
        .bind(metadata.git_branch.as_deref())
        .bind(metadata.git_origin_url.as_deref())
        .bind(memory_mode)
        .execute(&mut **tx)
        .await?;
        if let Some(parent_thread_id) =
            threads::thread_spawn_parent_thread_id_from_source_str(metadata.source.as_str())
        {
            sqlx::query(
                r#"
INSERT INTO thread_spawn_edges (
    parent_thread_id,
    child_thread_id,
    status
) VALUES (?, ?, ?)
ON CONFLICT(child_thread_id) DO NOTHING
                "#,
            )
            .bind(parent_thread_id.to_string())
            .bind(metadata.id.to_string())
            .bind(crate::DirectionalThreadSpawnEdgeStatus::Open.as_ref())
            .execute(&mut **tx)
            .await?;
        }
        Ok(())
    }
}

fn base_sqlite_options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .log_statements(LevelFilter::Off)
}

async fn open_state_sqlite(path: &Path, migrator: &Migrator) -> anyhow::Result<SqlitePool> {
    let options = base_sqlite_options(path).auto_vacuum(SqliteAutoVacuum::Incremental);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;
    account_pool::clean_up_duplicate_active_holder_leases_before_0026(&pool).await?;
    rewrite_historical_modified_0026_checksum(&pool, migrator).await?;
    migrator.run(&pool).await?;
    let auto_vacuum = sqlx::query_scalar::<_, i64>("PRAGMA auto_vacuum")
        .fetch_one(&pool)
        .await?;
    if auto_vacuum != SqliteAutoVacuum::Incremental as i64 {
        // Existing state DBs need one non-transactional `VACUUM` before
        // SQLite persists `auto_vacuum = INCREMENTAL` in the database header.
        sqlx::query("PRAGMA auto_vacuum = INCREMENTAL")
            .execute(&pool)
            .await?;
        // We do it on best effort. If the lock can't be acquired, it will be done at next run.
        let _ = sqlx::query("VACUUM").execute(&pool).await;
    }
    // We do it on best effort. If the lock can't be acquired, it will be done at next run.
    let _ = sqlx::query("PRAGMA incremental_vacuum")
        .execute(&pool)
        .await;
    Ok(pool)
}

async fn rewrite_historical_modified_0026_checksum(
    pool: &SqlitePool,
    migrator: &Migrator,
) -> anyhow::Result<()> {
    let migrations_table_exists: i64 = sqlx::query_scalar(
        r#"
SELECT COUNT(*)
FROM sqlite_master
WHERE type = 'table'
  AND name = '_sqlx_migrations'
        "#,
    )
    .fetch_one(pool)
    .await?;
    if migrations_table_exists == 0 {
        return Ok(());
    }

    let Some(current_0026_checksum) = migrator
        .migrations
        .iter()
        .find(|migration| migration.version == ACCOUNT_LEASES_ACTIVE_HOLDER_INDEX_MIGRATION_VERSION)
        .map(|migration| migration.checksum.as_ref())
    else {
        return Ok(());
    };

    sqlx::query(
        r#"
UPDATE _sqlx_migrations
SET checksum = ?
WHERE version = ?
  AND success = 1
  AND hex(checksum) = ?
        "#,
    )
    .bind(current_0026_checksum)
    .bind(ACCOUNT_LEASES_ACTIVE_HOLDER_INDEX_MIGRATION_VERSION)
    .bind(HISTORICAL_MODIFIED_0026_CHECKSUM_HEX)
    .execute(pool)
    .await?;

    Ok(())
}

async fn open_logs_sqlite(path: &Path, migrator: &Migrator) -> anyhow::Result<SqlitePool> {
    let options = base_sqlite_options(path).auto_vacuum(SqliteAutoVacuum::Incremental);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;
    migrator.run(&pool).await?;
    Ok(pool)
}

fn db_filename(base_name: &str, version: u32) -> String {
    format!("{base_name}_{version}.sqlite")
}

pub fn state_db_filename() -> String {
    db_filename(STATE_DB_FILENAME, STATE_DB_VERSION)
}

pub fn state_db_path(codex_home: &Path) -> PathBuf {
    codex_home.join(state_db_filename())
}

pub fn logs_db_filename() -> String {
    db_filename(LOGS_DB_FILENAME, LOGS_DB_VERSION)
}

pub fn logs_db_path(codex_home: &Path) -> PathBuf {
    codex_home.join(logs_db_filename())
}

async fn remove_legacy_db_files(
    codex_home: &Path,
    current_name: &str,
    base_name: &str,
    db_label: &str,
) {
    let mut entries = match tokio::fs::read_dir(codex_home).await {
        Ok(entries) => entries,
        Err(err) => {
            warn!(
                "failed to read codex_home for {db_label} db cleanup {}: {err}",
                codex_home.display(),
            );
            return;
        }
    };
    let mut legacy_paths = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if !entry
            .file_type()
            .await
            .map(|file_type| file_type.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if !should_remove_db_file(file_name.as_ref(), current_name, base_name) {
            continue;
        }

        legacy_paths.push(entry.path());
    }

    // On Windows, SQLite can keep the main database file undeletable until the
    // matching `-wal` / `-shm` sidecars are removed. Remove the longest
    // sidecar-style paths first so the main file is attempted last.
    legacy_paths.sort_by_key(|path| std::cmp::Reverse(path.as_os_str().len()));
    for legacy_path in legacy_paths {
        if let Err(err) = tokio::fs::remove_file(&legacy_path).await {
            warn!(
                "failed to remove legacy {db_label} db file {}: {err}",
                legacy_path.display(),
            );
        }
    }
}

async fn find_legacy_db_paths(
    codex_home: &Path,
    current_name: &str,
    base_name: &str,
) -> Vec<PathBuf> {
    let mut entries = match tokio::fs::read_dir(codex_home).await {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut legacy_paths: Vec<(i64, String)> = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if !entry
            .file_type()
            .await
            .map(|file_type| file_type.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        let Some(normalized_name) = normalize_db_file_name(file_name.as_ref()) else {
            continue;
        };
        if normalized_name == current_name {
            continue;
        }
        let Some(rank) = legacy_db_rank(normalized_name, base_name) else {
            continue;
        };
        legacy_paths.push((rank, normalized_name.to_string()));
    }
    legacy_paths.sort_by(|(left_rank, left_name), (right_rank, right_name)| {
        right_rank
            .cmp(left_rank)
            .then_with(|| right_name.cmp(left_name))
    });
    legacy_paths
        .into_iter()
        .map(|(_, file_name)| codex_home.join(file_name))
        .collect()
}

fn normalize_db_file_name(file_name: &str) -> Option<&str> {
    for suffix in ["-wal", "-shm", "-journal"] {
        if let Some(stripped) = file_name.strip_suffix(suffix) {
            return Some(stripped);
        }
    }
    Some(file_name)
}

fn legacy_db_rank(file_name: &str, base_name: &str) -> Option<i64> {
    let unversioned_name = format!("{base_name}.sqlite");
    if file_name == unversioned_name {
        return Some(0);
    }
    let version_with_extension = file_name.strip_prefix(&format!("{base_name}_"))?;
    let version_suffix = version_with_extension.strip_suffix(".sqlite")?;
    if version_suffix.is_empty() || !version_suffix.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    version_suffix.parse::<i64>().ok()
}

fn should_remove_db_file(file_name: &str, current_name: &str, base_name: &str) -> bool {
    let Some(normalized_name) = normalize_db_file_name(file_name) else {
        return false;
    };
    if normalized_name == current_name {
        return false;
    }
    legacy_db_rank(normalized_name, base_name).is_some()
}

#[cfg(test)]
mod tests {
    use super::StateRuntime;
    use super::open_state_sqlite;
    use super::runtime_state_migrator;
    use super::state_db_path;
    use super::test_support::test_thread_metadata;
    use super::test_support::unique_temp_dir;
    use crate::STATE_DB_FILENAME;
    use crate::STATE_DB_VERSION;
    use crate::migrations::STATE_MIGRATOR;
    use codex_protocol::ThreadId;
    use pretty_assertions::assert_eq;
    use sqlx::SqlitePool;
    use sqlx::migrate::MigrateError;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::collections::BTreeSet;
    use std::path::Path;

    async fn open_db_pool(path: &Path) -> SqlitePool {
        SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(false),
        )
        .await
        .expect("open sqlite pool")
    }

    #[tokio::test]
    async fn open_state_sqlite_tolerates_newer_applied_migrations() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let state_path = state_db_path(codex_home.as_path());
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&state_path)
                .create_if_missing(true),
        )
        .await
        .expect("open state db");
        STATE_MIGRATOR
            .run(&pool)
            .await
            .expect("apply current state schema");
        sqlx::query(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(9_999_i64)
        .bind("future migration")
        .bind(true)
        .bind(vec![1_u8, 2, 3, 4])
        .bind(1_i64)
        .execute(&pool)
        .await
        .expect("insert future migration record");
        pool.close().await;

        let strict_pool = open_db_pool(state_path.as_path()).await;
        let strict_err = STATE_MIGRATOR
            .run(&strict_pool)
            .await
            .expect_err("strict migrator should reject newer applied migrations");
        assert!(matches!(strict_err, MigrateError::VersionMissing(9_999)));
        strict_pool.close().await;

        let tolerant_migrator = runtime_state_migrator();
        let tolerant_pool = open_state_sqlite(state_path.as_path(), &tolerant_migrator)
            .await
            .expect("runtime migrator should tolerate newer applied migrations");
        tolerant_pool.close().await;

        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[test]
    fn state_migration_versions_are_unique() {
        let mut seen = BTreeSet::new();
        for migration in STATE_MIGRATOR.migrations.iter() {
            assert!(
                seen.insert(migration.version),
                "duplicate state migration version {}: {}",
                migration.version,
                migration.description
            );
        }
    }

    #[tokio::test]
    async fn init_imports_threads_from_legacy_state_db_before_cleanup() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let legacy_path = codex_home.join(format!(
            "{STATE_DB_FILENAME}_{}.sqlite",
            STATE_DB_VERSION.saturating_sub(1)
        ));
        let legacy_pool = open_state_sqlite(legacy_path.as_path(), &runtime_state_migrator())
            .await
            .expect("open legacy state db");
        let thread_id =
            ThreadId::from_string("2b9bd0a1-c35c-4f39-915c-d3c6458ad782").expect("thread id");
        let metadata = test_thread_metadata(&codex_home, thread_id, codex_home.clone());
        sqlx::query(
            r#"
INSERT INTO threads (
    id,
    rollout_path,
    created_at,
    updated_at,
    source,
    agent_nickname,
    agent_role,
    agent_path,
    model_provider,
    model,
    reasoning_effort,
    cwd,
    cli_version,
    title,
    sandbox_policy,
    approval_mode,
    tokens_used,
    first_user_message,
    memory_mode,
    archived_at,
    git_sha,
    git_branch,
    git_origin_url
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(metadata.id.to_string())
        .bind(metadata.rollout_path.display().to_string())
        .bind(metadata.created_at.timestamp())
        .bind(metadata.updated_at.timestamp())
        .bind(metadata.source.as_str())
        .bind(metadata.agent_nickname.as_deref())
        .bind(metadata.agent_role.as_deref())
        .bind(metadata.agent_path.as_deref())
        .bind(metadata.model_provider.as_str())
        .bind(metadata.model.as_deref())
        .bind(
            metadata
                .reasoning_effort
                .as_ref()
                .map(crate::extract::enum_to_string),
        )
        .bind(metadata.cwd.display().to_string())
        .bind(metadata.cli_version.as_str())
        .bind(metadata.title.as_str())
        .bind(metadata.sandbox_policy.as_str())
        .bind(metadata.approval_mode.as_str())
        .bind(metadata.tokens_used)
        .bind(metadata.first_user_message.as_deref())
        .bind("enabled")
        .bind(
            metadata
                .archived_at
                .map(|archived_at| archived_at.timestamp()),
        )
        .bind(metadata.git_sha.as_deref())
        .bind(metadata.git_branch.as_deref())
        .bind(metadata.git_origin_url.as_deref())
        .execute(&legacy_pool)
        .await
        .expect("insert legacy thread row");
        legacy_pool.close().await;

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");

        assert_eq!(
            runtime
                .get_thread(thread_id)
                .await
                .expect("get imported thread"),
            Some(metadata)
        );
        assert_eq!(
            tokio::fs::try_exists(legacy_path.as_path())
                .await
                .expect("check legacy path"),
            false
        );
    }

    #[tokio::test]
    async fn init_import_falls_back_to_older_legacy_state_db_before_cleanup() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let invalid_latest_path = codex_home.join(format!(
            "{STATE_DB_FILENAME}_{}.sqlite",
            STATE_DB_VERSION.saturating_sub(1)
        ));
        tokio::fs::write(&invalid_latest_path, "not a sqlite database")
            .await
            .expect("write invalid legacy db");

        let valid_older_path = codex_home.join(format!(
            "{STATE_DB_FILENAME}_{}.sqlite",
            STATE_DB_VERSION.saturating_sub(2)
        ));
        let legacy_pool = open_state_sqlite(valid_older_path.as_path(), &runtime_state_migrator())
            .await
            .expect("open legacy state db");
        let thread_id =
            ThreadId::from_string("6d61f8bb-0d6f-4fcb-8d0f-3dd2e2cfa2d3").expect("thread id");
        let metadata = test_thread_metadata(&codex_home, thread_id, codex_home.clone());
        sqlx::query(
            r#"
INSERT INTO threads (
    id,
    rollout_path,
    created_at,
    updated_at,
    source,
    agent_nickname,
    agent_role,
    agent_path,
    model_provider,
    model,
    reasoning_effort,
    cwd,
    cli_version,
    title,
    sandbox_policy,
    approval_mode,
    tokens_used,
    first_user_message,
    memory_mode,
    archived_at,
    git_sha,
    git_branch,
    git_origin_url
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(metadata.id.to_string())
        .bind(metadata.rollout_path.display().to_string())
        .bind(metadata.created_at.timestamp())
        .bind(metadata.updated_at.timestamp())
        .bind(metadata.source.as_str())
        .bind(metadata.agent_nickname.as_deref())
        .bind(metadata.agent_role.as_deref())
        .bind(metadata.agent_path.as_deref())
        .bind(metadata.model_provider.as_str())
        .bind(metadata.model.as_deref())
        .bind(
            metadata
                .reasoning_effort
                .as_ref()
                .map(crate::extract::enum_to_string),
        )
        .bind(metadata.cwd.display().to_string())
        .bind(metadata.cli_version.as_str())
        .bind(metadata.title.as_str())
        .bind(metadata.sandbox_policy.as_str())
        .bind(metadata.approval_mode.as_str())
        .bind(metadata.tokens_used)
        .bind(metadata.first_user_message.as_deref())
        .bind("enabled")
        .bind(
            metadata
                .archived_at
                .map(|archived_at| archived_at.timestamp()),
        )
        .bind(metadata.git_sha.as_deref())
        .bind(metadata.git_branch.as_deref())
        .bind(metadata.git_origin_url.as_deref())
        .execute(&legacy_pool)
        .await
        .expect("insert legacy thread row");
        legacy_pool.close().await;

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");

        assert_eq!(
            runtime
                .get_thread(thread_id)
                .await
                .expect("get imported thread"),
            Some(metadata)
        );
        assert_eq!(
            tokio::fs::try_exists(invalid_latest_path.as_path())
                .await
                .expect("check invalid legacy path"),
            true
        );
        assert_eq!(
            tokio::fs::try_exists(valid_older_path.as_path())
                .await
                .expect("check valid legacy path"),
            false
        );
    }

    #[tokio::test]
    async fn init_does_not_mix_partial_rows_from_failed_newer_legacy_import() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let invalid_latest_path = codex_home.join(format!(
            "{STATE_DB_FILENAME}_{}.sqlite",
            STATE_DB_VERSION.saturating_sub(1)
        ));
        let invalid_latest_pool =
            open_state_sqlite(invalid_latest_path.as_path(), &runtime_state_migrator())
                .await
                .expect("open newer legacy state db");
        let newer_thread_id =
            ThreadId::from_string("f6df535d-13e7-4bec-b74a-4b724a15cf66").expect("thread id");
        let newer_metadata = test_thread_metadata(&codex_home, newer_thread_id, codex_home.clone());
        sqlx::query(
            r#"
INSERT INTO threads (
    id,
    rollout_path,
    created_at,
    updated_at,
    source,
    agent_nickname,
    agent_role,
    agent_path,
    model_provider,
    model,
    reasoning_effort,
    cwd,
    cli_version,
    title,
    sandbox_policy,
    approval_mode,
    tokens_used,
    first_user_message,
    memory_mode,
    archived_at,
    git_sha,
    git_branch,
    git_origin_url
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(newer_metadata.id.to_string())
        .bind(newer_metadata.rollout_path.display().to_string())
        .bind(newer_metadata.created_at.timestamp())
        .bind(newer_metadata.updated_at.timestamp())
        .bind(newer_metadata.source.as_str())
        .bind(newer_metadata.agent_nickname.as_deref())
        .bind(newer_metadata.agent_role.as_deref())
        .bind(newer_metadata.agent_path.as_deref())
        .bind(newer_metadata.model_provider.as_str())
        .bind(newer_metadata.model.as_deref())
        .bind(
            newer_metadata
                .reasoning_effort
                .as_ref()
                .map(crate::extract::enum_to_string),
        )
        .bind(newer_metadata.cwd.display().to_string())
        .bind(newer_metadata.cli_version.as_str())
        .bind(newer_metadata.title.as_str())
        .bind(newer_metadata.sandbox_policy.as_str())
        .bind(newer_metadata.approval_mode.as_str())
        .bind(newer_metadata.tokens_used)
        .bind(newer_metadata.first_user_message.as_deref())
        .bind("enabled")
        .bind(
            newer_metadata
                .archived_at
                .map(|archived_at| archived_at.timestamp()),
        )
        .bind(newer_metadata.git_sha.as_deref())
        .bind(newer_metadata.git_branch.as_deref())
        .bind(newer_metadata.git_origin_url.as_deref())
        .execute(&invalid_latest_pool)
        .await
        .expect("insert valid newer legacy thread row");
        sqlx::query(
            r#"
INSERT INTO threads (
    id,
    rollout_path,
    created_at,
    updated_at,
    source,
    agent_nickname,
    agent_role,
    agent_path,
    model_provider,
    model,
    reasoning_effort,
    cwd,
    cli_version,
    title,
    sandbox_policy,
    approval_mode,
    tokens_used,
    first_user_message,
    memory_mode,
    archived_at,
    git_sha,
    git_branch,
    git_origin_url
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind("not-a-valid-thread-id")
        .bind(
            codex_home
                .join("broken-rollout.jsonl")
                .display()
                .to_string(),
        )
        .bind(newer_metadata.created_at.timestamp())
        .bind(newer_metadata.updated_at.timestamp())
        .bind(newer_metadata.source.as_str())
        .bind(newer_metadata.agent_nickname.as_deref())
        .bind(newer_metadata.agent_role.as_deref())
        .bind(newer_metadata.agent_path.as_deref())
        .bind(newer_metadata.model_provider.as_str())
        .bind(newer_metadata.model.as_deref())
        .bind(
            newer_metadata
                .reasoning_effort
                .as_ref()
                .map(crate::extract::enum_to_string),
        )
        .bind(newer_metadata.cwd.display().to_string())
        .bind(newer_metadata.cli_version.as_str())
        .bind(newer_metadata.title.as_str())
        .bind(newer_metadata.sandbox_policy.as_str())
        .bind(newer_metadata.approval_mode.as_str())
        .bind(newer_metadata.tokens_used)
        .bind(newer_metadata.first_user_message.as_deref())
        .bind("enabled")
        .bind(
            newer_metadata
                .archived_at
                .map(|archived_at| archived_at.timestamp()),
        )
        .bind(newer_metadata.git_sha.as_deref())
        .bind(newer_metadata.git_branch.as_deref())
        .bind(newer_metadata.git_origin_url.as_deref())
        .execute(&invalid_latest_pool)
        .await
        .expect("insert invalid newer legacy thread row");
        invalid_latest_pool.close().await;

        let valid_older_path = codex_home.join(format!(
            "{STATE_DB_FILENAME}_{}.sqlite",
            STATE_DB_VERSION.saturating_sub(2)
        ));
        let valid_older_pool =
            open_state_sqlite(valid_older_path.as_path(), &runtime_state_migrator())
                .await
                .expect("open older legacy state db");
        let older_thread_id =
            ThreadId::from_string("9ce6c696-f48d-49bc-ac82-c5861b537367").expect("thread id");
        let older_metadata = test_thread_metadata(&codex_home, older_thread_id, codex_home.clone());
        sqlx::query(
            r#"
INSERT INTO threads (
    id,
    rollout_path,
    created_at,
    updated_at,
    source,
    agent_nickname,
    agent_role,
    agent_path,
    model_provider,
    model,
    reasoning_effort,
    cwd,
    cli_version,
    title,
    sandbox_policy,
    approval_mode,
    tokens_used,
    first_user_message,
    memory_mode,
    archived_at,
    git_sha,
    git_branch,
    git_origin_url
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(older_metadata.id.to_string())
        .bind(older_metadata.rollout_path.display().to_string())
        .bind(older_metadata.created_at.timestamp())
        .bind(older_metadata.updated_at.timestamp())
        .bind(older_metadata.source.as_str())
        .bind(older_metadata.agent_nickname.as_deref())
        .bind(older_metadata.agent_role.as_deref())
        .bind(older_metadata.agent_path.as_deref())
        .bind(older_metadata.model_provider.as_str())
        .bind(older_metadata.model.as_deref())
        .bind(
            older_metadata
                .reasoning_effort
                .as_ref()
                .map(crate::extract::enum_to_string),
        )
        .bind(older_metadata.cwd.display().to_string())
        .bind(older_metadata.cli_version.as_str())
        .bind(older_metadata.title.as_str())
        .bind(older_metadata.sandbox_policy.as_str())
        .bind(older_metadata.approval_mode.as_str())
        .bind(older_metadata.tokens_used)
        .bind(older_metadata.first_user_message.as_deref())
        .bind("enabled")
        .bind(
            older_metadata
                .archived_at
                .map(|archived_at| archived_at.timestamp()),
        )
        .bind(older_metadata.git_sha.as_deref())
        .bind(older_metadata.git_branch.as_deref())
        .bind(older_metadata.git_origin_url.as_deref())
        .execute(&valid_older_pool)
        .await
        .expect("insert older legacy thread row");
        valid_older_pool.close().await;

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");

        assert_eq!(
            runtime
                .get_thread(newer_thread_id)
                .await
                .expect("get newer thread"),
            None
        );
        assert_eq!(
            runtime
                .get_thread(older_thread_id)
                .await
                .expect("get older thread"),
            Some(older_metadata)
        );
    }

    #[tokio::test]
    async fn init_imports_threads_from_older_legacy_schema_without_newer_columns() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let legacy_path = codex_home.join(format!(
            "{STATE_DB_FILENAME}_{}.sqlite",
            STATE_DB_VERSION.saturating_sub(1)
        ));
        let legacy_pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&legacy_path)
                .create_if_missing(true),
        )
        .await
        .expect("open legacy state db");
        sqlx::query(
            r#"
CREATE TABLE threads (
    id TEXT PRIMARY KEY NOT NULL,
    rollout_path TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    source TEXT NOT NULL,
    model_provider TEXT NOT NULL,
    cwd TEXT NOT NULL,
    title TEXT NOT NULL,
    sandbox_policy TEXT NOT NULL,
    approval_mode TEXT NOT NULL,
    tokens_used INTEGER NOT NULL,
    archived_at INTEGER,
    git_sha TEXT,
    git_branch TEXT,
    git_origin_url TEXT
)
            "#,
        )
        .execute(&legacy_pool)
        .await
        .expect("create legacy threads table");
        let thread_id =
            ThreadId::from_string("55b34f43-0581-4cba-9aab-f4dfc61529ad").expect("thread id");
        let mut metadata = test_thread_metadata(&codex_home, thread_id, codex_home.clone());
        metadata.model = None;
        metadata.reasoning_effort = None;
        metadata.cli_version.clear();
        metadata.first_user_message = None;
        sqlx::query(
            r#"
INSERT INTO threads (
    id,
    rollout_path,
    created_at,
    updated_at,
    source,
    model_provider,
    cwd,
    title,
    sandbox_policy,
    approval_mode,
    tokens_used,
    archived_at,
    git_sha,
    git_branch,
    git_origin_url
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(metadata.id.to_string())
        .bind(metadata.rollout_path.display().to_string())
        .bind(metadata.created_at.timestamp())
        .bind(metadata.updated_at.timestamp())
        .bind(metadata.source.as_str())
        .bind(metadata.model_provider.as_str())
        .bind(metadata.cwd.display().to_string())
        .bind(metadata.title.as_str())
        .bind(metadata.sandbox_policy.as_str())
        .bind(metadata.approval_mode.as_str())
        .bind(metadata.tokens_used)
        .bind(
            metadata
                .archived_at
                .map(|archived_at| archived_at.timestamp()),
        )
        .bind(metadata.git_sha.as_deref())
        .bind(metadata.git_branch.as_deref())
        .bind(metadata.git_origin_url.as_deref())
        .execute(&legacy_pool)
        .await
        .expect("insert legacy thread row");
        legacy_pool.close().await;

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");

        assert_eq!(
            runtime
                .get_thread(thread_id)
                .await
                .expect("get imported thread"),
            Some(metadata)
        );
        assert_eq!(
            runtime
                .get_thread_memory_mode(thread_id)
                .await
                .expect("get imported thread memory mode"),
            Some("enabled".to_string())
        );
    }

    #[tokio::test]
    async fn init_keeps_legacy_state_db_when_import_fails() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let legacy_path = codex_home.join(format!(
            "{STATE_DB_FILENAME}_{}.sqlite",
            STATE_DB_VERSION.saturating_sub(1)
        ));
        tokio::fs::write(&legacy_path, "not a sqlite database")
            .await
            .expect("write invalid legacy db");

        let _runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");

        assert_eq!(
            tokio::fs::try_exists(legacy_path.as_path())
                .await
                .expect("check legacy path"),
            true
        );
    }
}
