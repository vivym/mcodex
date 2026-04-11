use super::*;
use crate::model::AccountHealthEvent;
use crate::model::AccountLeaseError;
use crate::model::AccountLeaseRecord;
use crate::model::AccountStartupSelectionState;
use crate::model::AccountStartupSelectionUpdate;
use crate::model::LeaseKey;
use crate::model::LeaseRenewal;
use crate::model::LegacyAccountImport;
use crate::model::account_datetime_to_epoch_seconds;
use crate::model::account_epoch_seconds_to_datetime;
use sqlx::Executor;
use uuid::Uuid;

const DEFAULT_ACCOUNT_LEASE_SECONDS: i64 = 300;
const LEGACY_DEFAULT_POOL_ID: &str = "legacy-default";
const ACTIVE_HOLDER_INDEX_MIGRATION_VERSION: i64 = 26;

pub(super) async fn clean_up_duplicate_active_holder_leases_before_0026(
    pool: &SqlitePool,
) -> anyhow::Result<()> {
    let account_leases_exists: i64 = sqlx::query_scalar(
        r#"
SELECT COUNT(*)
FROM sqlite_master
WHERE type = 'table'
  AND name = 'account_leases'
        "#,
    )
    .fetch_one(pool)
    .await?;
    if account_leases_exists == 0 {
        return Ok(());
    }

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
    if migrations_table_exists != 0 {
        let active_holder_index_applied: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM _sqlx_migrations
WHERE version = ?
  AND success = 1
            "#,
        )
        .bind(ACTIVE_HOLDER_INDEX_MIGRATION_VERSION)
        .fetch_one(pool)
        .await?;
        if active_holder_index_applied != 0 {
            return Ok(());
        }
    }

    sqlx::query(
        r#"
WITH ranked_active_leases AS (
    SELECT
        lease_id,
        ROW_NUMBER() OVER (
            PARTITION BY holder_instance_id
            ORDER BY acquired_at DESC, lease_id DESC
        ) AS row_num
    FROM account_leases
    WHERE released_at IS NULL
)
UPDATE account_leases
SET released_at = unixepoch('now')
WHERE lease_id IN (
    SELECT lease_id
    FROM ranked_active_leases
    WHERE row_num > 1
)
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}

impl StateRuntime {
    pub async fn acquire_account_lease(
        &self,
        pool_id: &str,
        holder_instance_id: &str,
    ) -> std::result::Result<AccountLeaseRecord, AccountLeaseError> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(account_lease_storage_error)?;

        sqlx::query(
            r#"
UPDATE account_leases
SET released_at = ?
WHERE released_at IS NULL
  AND expires_at <= ?
            "#,
        )
        .bind(account_datetime_to_epoch_seconds(now))
        .bind(account_datetime_to_epoch_seconds(now))
        .execute(&mut *tx)
        .await
        .map_err(account_lease_storage_error)?;

        if let Some(existing_lease) = load_active_holder_lease(&mut *tx, holder_instance_id)
            .await
            .map_err(|err| AccountLeaseError::Storage(err.to_string()))?
        {
            tx.commit().await.map_err(account_lease_storage_error)?;
            return Ok(existing_lease);
        }

        let lease = AccountLeaseRecord {
            lease_id: Uuid::new_v4().to_string(),
            pool_id: pool_id.to_string(),
            account_id: String::new(),
            holder_instance_id: holder_instance_id.to_string(),
            lease_epoch: 0,
            acquired_at: now,
            renewed_at: now,
            expires_at: now + chrono::Duration::seconds(DEFAULT_ACCOUNT_LEASE_SECONDS),
            released_at: None,
        };

        let result = sqlx::query(
            r#"
INSERT INTO account_leases (
    lease_id,
    account_id,
    pool_id,
    holder_instance_id,
    lease_epoch,
    acquired_at,
    renewed_at,
    expires_at,
    released_at
) SELECT
    ?,
    account_registry.account_id,
    ?,
    ?,
    COALESCE((
        SELECT MAX(existing.lease_epoch) + 1
        FROM account_leases AS existing
        WHERE existing.account_id = account_registry.account_id
    ), 1),
    ?,
    ?,
    ?,
    NULL
FROM account_registry
WHERE pool_id = ?
  AND healthy = 1
  AND NOT EXISTS (
      SELECT 1
      FROM account_leases
      WHERE account_leases.account_id = account_registry.account_id
        AND account_leases.released_at IS NULL
  )
ORDER BY position ASC, account_id ASC
LIMIT 1
            "#,
        )
        .bind(&lease.lease_id)
        .bind(&lease.pool_id)
        .bind(&lease.holder_instance_id)
        .bind(account_datetime_to_epoch_seconds(lease.acquired_at))
        .bind(account_datetime_to_epoch_seconds(lease.renewed_at))
        .bind(account_datetime_to_epoch_seconds(lease.expires_at))
        .bind(pool_id)
        .execute(&mut *tx)
        .await;

        let result = match result {
            Ok(result) => result,
            Err(err) if account_lease_is_contention_error(&err) => {
                let existing_lease =
                    load_active_holder_lease(self.pool.as_ref(), holder_instance_id)
                        .await
                        .map_err(|load_err| AccountLeaseError::Storage(load_err.to_string()))?;
                return match existing_lease {
                    Some(existing_lease) => Ok(existing_lease),
                    None => Err(AccountLeaseError::NoEligibleAccount),
                };
            }
            Err(err) => return Err(account_lease_storage_error(err)),
        };

        if result.rows_affected() == 0 {
            return Err(AccountLeaseError::NoEligibleAccount);
        }

        tx.commit().await.map_err(account_lease_storage_error)?;

        let lease = load_lease(self.pool.as_ref(), &lease.lease_id)
            .await
            .map_err(|err| AccountLeaseError::Storage(err.to_string()))?
            .ok_or_else(|| {
                AccountLeaseError::Storage(format!("missing inserted lease {}", lease.lease_id))
            })?;

        Ok(lease)
    }

    pub async fn renew_account_lease(
        &self,
        lease: &LeaseKey,
        now: DateTime<Utc>,
    ) -> anyhow::Result<LeaseRenewal> {
        let expires_at = now + chrono::Duration::seconds(DEFAULT_ACCOUNT_LEASE_SECONDS);
        let result = sqlx::query(
            r#"
UPDATE account_leases
SET renewed_at = ?, expires_at = ?
WHERE lease_id = ?
  AND account_id = ?
  AND lease_epoch = ?
  AND released_at IS NULL
  AND expires_at > ?
            "#,
        )
        .bind(account_datetime_to_epoch_seconds(now))
        .bind(account_datetime_to_epoch_seconds(expires_at))
        .bind(&lease.lease_id)
        .bind(&lease.account_id)
        .bind(lease.lease_epoch)
        .bind(account_datetime_to_epoch_seconds(now))
        .execute(self.pool.as_ref())
        .await?;

        if result.rows_affected() == 0 {
            return Ok(LeaseRenewal::Missing);
        }

        let renewed = load_lease(self.pool.as_ref(), &lease.lease_id).await?;
        match renewed {
            Some(record) => Ok(LeaseRenewal::Renewed(record)),
            None => Ok(LeaseRenewal::Missing),
        }
    }

    pub async fn record_account_health_event(
        &self,
        event: AccountHealthEvent,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        let updated_at = account_datetime_to_epoch_seconds(Utc::now());
        let observed_at = account_datetime_to_epoch_seconds(event.observed_at);

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
ON CONFLICT(account_id) DO UPDATE SET
    pool_id = CASE
        WHEN excluded.last_health_event_sequence > account_runtime_state.last_health_event_sequence
            THEN excluded.pool_id
        ELSE account_runtime_state.pool_id
    END,
    health_state = CASE
        WHEN excluded.last_health_event_sequence > account_runtime_state.last_health_event_sequence
            THEN excluded.health_state
        ELSE account_runtime_state.health_state
    END,
    last_health_event_sequence = MAX(
        account_runtime_state.last_health_event_sequence,
        excluded.last_health_event_sequence
    ),
    last_health_event_at = CASE
        WHEN excluded.last_health_event_sequence > account_runtime_state.last_health_event_sequence
            THEN excluded.last_health_event_at
        ELSE account_runtime_state.last_health_event_at
    END,
    updated_at = excluded.updated_at
            "#,
        )
        .bind(&event.account_id)
        .bind(&event.pool_id)
        .bind(event.health_state.as_str())
        .bind(event.sequence_number)
        .bind(observed_at)
        .bind(updated_at)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
UPDATE account_registry
SET healthy = CASE
        WHEN (
            SELECT health_state
            FROM account_runtime_state
            WHERE account_runtime_state.account_id = account_registry.account_id
        ) = 'healthy'
            THEN 1
        ELSE 0
    END,
    updated_at = ?
WHERE account_id = ?
            "#,
        )
        .bind(updated_at)
        .bind(&event.account_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn import_legacy_default_account(
        &self,
        legacy_account: LegacyAccountImport,
    ) -> anyhow::Result<()> {
        let now = account_datetime_to_epoch_seconds(Utc::now());
        let mut tx = self.pool.begin().await?;

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
ON CONFLICT(account_id) DO NOTHING
            "#,
        )
        .bind(&legacy_account.account_id)
        .bind(LEGACY_DEFAULT_POOL_ID)
        .bind(0_i64)
        .bind("chatgpt")
        .bind("chatgpt")
        .bind(Option::<String>::None)
        .bind(1_i64)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        let default_pool_id: String = sqlx::query_scalar(
            r#"
SELECT pool_id
FROM account_registry
WHERE account_id = ?
            "#,
        )
        .bind(&legacy_account.account_id)
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query(
            r#"
INSERT INTO account_startup_selection (
    singleton,
    default_pool_id,
    preferred_account_id,
    suppressed,
    updated_at
) VALUES (1, ?, ?, ?, ?)
ON CONFLICT(singleton) DO NOTHING
            "#,
        )
        .bind(default_pool_id)
        .bind(&legacy_account.account_id)
        .bind(0_i64)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn read_account_startup_selection(
        &self,
    ) -> anyhow::Result<AccountStartupSelectionState> {
        let row = sqlx::query(
            r#"
SELECT default_pool_id, preferred_account_id, suppressed
FROM account_startup_selection
WHERE singleton = 1
            "#,
        )
        .fetch_optional(self.pool.as_ref())
        .await?;

        match row {
            Some(row) => Ok(AccountStartupSelectionState {
                default_pool_id: row.try_get("default_pool_id")?,
                preferred_account_id: row.try_get("preferred_account_id")?,
                suppressed: row.try_get::<i64, _>("suppressed")? != 0,
            }),
            None => Ok(AccountStartupSelectionState::default()),
        }
    }

    pub async fn write_account_startup_selection(
        &self,
        update: AccountStartupSelectionUpdate,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
INSERT INTO account_startup_selection (
    singleton,
    default_pool_id,
    preferred_account_id,
    suppressed,
    updated_at
) VALUES (1, ?, ?, ?, ?)
ON CONFLICT(singleton) DO UPDATE SET
    default_pool_id = excluded.default_pool_id,
    preferred_account_id = excluded.preferred_account_id,
    suppressed = excluded.suppressed,
    updated_at = excluded.updated_at
            "#,
        )
        .bind(update.default_pool_id)
        .bind(update.preferred_account_id)
        .bind(i64::from(update.suppressed))
        .bind(account_datetime_to_epoch_seconds(Utc::now()))
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    #[cfg(test)]
    async fn read_account_health_state(
        &self,
        account_id: &str,
    ) -> anyhow::Result<Option<crate::model::AccountPoolHealthState>> {
        let row = sqlx::query(
            r#"
SELECT
    account_id,
    pool_id,
    health_state,
    last_health_event_sequence,
    last_health_event_at,
    updated_at
FROM account_runtime_state
WHERE account_id = ?
            "#,
        )
        .bind(account_id)
        .fetch_optional(self.pool.as_ref())
        .await?;

        match row {
            Some(row) => Ok(Some(crate::model::AccountPoolHealthState {
                account_id: row.try_get("account_id")?,
                pool_id: row.try_get("pool_id")?,
                health_state: crate::model::AccountHealthState::try_from(
                    row.try_get::<String, _>("health_state")?.as_str(),
                )?,
                last_health_event_sequence: row.try_get("last_health_event_sequence")?,
                last_health_event_at: account_epoch_seconds_to_datetime(
                    row.try_get("last_health_event_at")?,
                )?,
                updated_at: account_epoch_seconds_to_datetime(row.try_get("updated_at")?)?,
            })),
            None => Ok(None),
        }
    }
}

fn account_lease_storage_error(err: sqlx::Error) -> AccountLeaseError {
    AccountLeaseError::Storage(err.to_string())
}

fn account_lease_is_contention_error(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Database(db_err) => {
            let message = db_err.message().to_ascii_lowercase();
            message.contains("unique")
                || message.contains("constraint failed")
                || message.contains("constraint violation")
        }
        _ => false,
    }
}

async fn load_lease<'e, E>(
    executor: E,
    lease_id: &str,
) -> anyhow::Result<Option<AccountLeaseRecord>>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query(
        r#"
SELECT
    lease_id,
    pool_id,
    account_id,
    holder_instance_id,
    lease_epoch,
    acquired_at,
    renewed_at,
    expires_at,
    released_at
FROM account_leases
WHERE lease_id = ?
        "#,
    )
    .bind(lease_id)
    .fetch_optional(executor)
    .await?;

    match row {
        Some(row) => Ok(Some(AccountLeaseRecord {
            lease_id: row.try_get("lease_id")?,
            pool_id: row.try_get("pool_id")?,
            account_id: row.try_get("account_id")?,
            holder_instance_id: row.try_get("holder_instance_id")?,
            lease_epoch: row.try_get("lease_epoch")?,
            acquired_at: account_epoch_seconds_to_datetime(row.try_get("acquired_at")?)?,
            renewed_at: account_epoch_seconds_to_datetime(row.try_get("renewed_at")?)?,
            expires_at: account_epoch_seconds_to_datetime(row.try_get("expires_at")?)?,
            released_at: row
                .try_get::<Option<i64>, _>("released_at")?
                .map(account_epoch_seconds_to_datetime)
                .transpose()?,
        })),
        None => Ok(None),
    }
}

async fn load_active_holder_lease<'e, E>(
    executor: E,
    holder_instance_id: &str,
) -> anyhow::Result<Option<AccountLeaseRecord>>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query(
        r#"
SELECT
    lease_id,
    pool_id,
    account_id,
    holder_instance_id,
    lease_epoch,
    acquired_at,
    renewed_at,
    expires_at,
    released_at
FROM account_leases
WHERE holder_instance_id = ?
  AND released_at IS NULL
ORDER BY acquired_at DESC, lease_id DESC
LIMIT 1
        "#,
    )
    .bind(holder_instance_id)
    .fetch_optional(executor)
    .await?;

    match row {
        Some(row) => Ok(Some(AccountLeaseRecord {
            lease_id: row.try_get("lease_id")?,
            pool_id: row.try_get("pool_id")?,
            account_id: row.try_get("account_id")?,
            holder_instance_id: row.try_get("holder_instance_id")?,
            lease_epoch: row.try_get("lease_epoch")?,
            acquired_at: account_epoch_seconds_to_datetime(row.try_get("acquired_at")?)?,
            renewed_at: account_epoch_seconds_to_datetime(row.try_get("renewed_at")?)?,
            expires_at: account_epoch_seconds_to_datetime(row.try_get("expires_at")?)?,
            released_at: row
                .try_get::<Option<i64>, _>("released_at")?
                .map(account_epoch_seconds_to_datetime)
                .transpose()?,
        })),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::StateRuntime;
    use super::test_support::unique_temp_dir;
    use crate::AccountHealthEvent;
    use crate::AccountHealthState;
    use crate::AccountLeaseError;
    use crate::LegacyAccountImport;
    use crate::migrations::STATE_MIGRATOR;
    use chrono::DateTime;
    use chrono::Utc;
    use pretty_assertions::assert_eq;
    use sqlx::SqlitePool;
    use sqlx::migrate::Migration;
    use sqlx::migrate::Migrator;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::borrow::Cow;
    use std::sync::Arc;

    const ORIGINAL_ACTIVE_HOLDER_INDEX_SQL: &str = r#"CREATE UNIQUE INDEX account_leases_active_holder_idx
ON account_leases(holder_instance_id)
WHERE released_at IS NULL;
"#;

    fn original_0026_migrator() -> Migrator {
        let mut migrations = STATE_MIGRATOR.migrations.to_vec();
        let current_0026 = migrations.last().expect("current 0026 migration").clone();
        migrations.pop();
        migrations.push(Migration::new(
            26,
            current_0026.description.clone(),
            current_0026.migration_type,
            Cow::Borrowed(ORIGINAL_ACTIVE_HOLDER_INDEX_SQL),
            current_0026.no_tx,
        ));
        Migrator {
            migrations: Cow::Owned(migrations),
            ignore_missing: false,
            locking: true,
            no_tx: false,
        }
    }

    #[tokio::test]
    async fn acquire_exclusive_lease_rejects_second_holder() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;

        let first = runtime
            .acquire_account_lease("pool-main", "inst-a")
            .await
            .unwrap();
        let second = runtime.acquire_account_lease("pool-main", "inst-b").await;

        assert_eq!(second.unwrap_err(), AccountLeaseError::NoEligibleAccount);
        assert_eq!(first.account_id, "acct-1");
    }

    #[tokio::test]
    async fn acquire_account_lease_returns_existing_active_lease_for_holder() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;
        seed_account(runtime.as_ref(), "acct-2").await;

        let first = runtime
            .acquire_account_lease("pool-main", "inst-a")
            .await
            .unwrap();
        let second = runtime
            .acquire_account_lease("pool-main", "inst-a")
            .await
            .unwrap();

        let active_lease_count: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM account_leases
WHERE holder_instance_id = ?
  AND released_at IS NULL
            "#,
        )
        .bind("inst-a")
        .fetch_one(runtime.pool.as_ref())
        .await
        .unwrap();

        assert_eq!(second, first);
        assert_eq!(active_lease_count, 1);
    }

    #[tokio::test]
    async fn migrated_install_creates_legacy_default_selection_state() {
        let runtime = test_runtime_with_legacy_auth("acct-legacy").await;
        let selection = runtime.read_account_startup_selection().await.unwrap();

        assert_eq!(selection.default_pool_id.as_deref(), Some("legacy-default"));
        assert_eq!(
            selection.preferred_account_id.as_deref(),
            Some("acct-legacy")
        );
        assert_eq!(selection.suppressed, false);
    }

    #[tokio::test]
    async fn init_cleans_up_duplicate_active_holder_leases_before_indexing() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let state_path = super::state_db_path(codex_home.as_path());
        let old_state_migrator = Migrator {
            migrations: Cow::Owned(
                STATE_MIGRATOR.migrations[..STATE_MIGRATOR.migrations.len() - 1].to_vec(),
            ),
            ignore_missing: false,
            locking: true,
            no_tx: false,
        };
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&state_path)
                .create_if_missing(true),
        )
        .await
        .expect("open old state db");
        old_state_migrator
            .run(&pool)
            .await
            .expect("apply pre-0026 state schema");
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
        .bind("acct-1")
        .bind("pool-main")
        .bind(0_i64)
        .bind("chatgpt")
        .bind("chatgpt")
        .bind(Option::<String>::None)
        .bind(1_i64)
        .bind(1_i64)
        .bind(1_i64)
        .execute(&pool)
        .await
        .expect("insert first account");
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
        .bind("acct-2")
        .bind("pool-main")
        .bind(1_i64)
        .bind("chatgpt")
        .bind("chatgpt")
        .bind(Option::<String>::None)
        .bind(1_i64)
        .bind(2_i64)
        .bind(2_i64)
        .execute(&pool)
        .await
        .expect("insert second account");
        sqlx::query(
            r#"
INSERT INTO account_leases (
    lease_id,
    account_id,
    pool_id,
    holder_instance_id,
    lease_epoch,
    acquired_at,
    renewed_at,
    expires_at,
    released_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL)
            "#,
        )
        .bind("lease-1")
        .bind("acct-1")
        .bind("pool-main")
        .bind("inst-a")
        .bind(1_i64)
        .bind(10_i64)
        .bind(10_i64)
        .bind(310_i64)
        .execute(&pool)
        .await
        .expect("insert first active lease");
        sqlx::query(
            r#"
INSERT INTO account_leases (
    lease_id,
    account_id,
    pool_id,
    holder_instance_id,
    lease_epoch,
    acquired_at,
    renewed_at,
    expires_at,
    released_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL)
            "#,
        )
        .bind("lease-2")
        .bind("acct-2")
        .bind("pool-main")
        .bind("inst-a")
        .bind(1_i64)
        .bind(20_i64)
        .bind(20_i64)
        .bind(320_i64)
        .execute(&pool)
        .await
        .expect("insert second active lease");
        pool.close().await;

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");
        let active_lease_count: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM account_leases
WHERE holder_instance_id = ?
  AND released_at IS NULL
            "#,
        )
        .bind("inst-a")
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("count active holder leases");
        let remaining_active_account_id: String = sqlx::query_scalar(
            r#"
SELECT account_id
FROM account_leases
WHERE holder_instance_id = ?
  AND released_at IS NULL
ORDER BY acquired_at DESC, lease_id DESC
LIMIT 1
            "#,
        )
        .bind("inst-a")
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("load remaining active lease");
        let active_holder_index_exists: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM sqlite_master
WHERE type = 'index'
  AND name = 'account_leases_active_holder_idx'
            "#,
        )
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("check active holder index");

        assert_eq!(active_lease_count, 1);
        assert_eq!(remaining_active_account_id, "acct-2");
        assert_eq!(active_holder_index_exists, 1);

        drop(runtime);
        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn init_accepts_databases_with_original_0026_already_applied() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let state_path = super::state_db_path(codex_home.as_path());
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&state_path)
                .create_if_missing(true),
        )
        .await
        .expect("open old state db");
        original_0026_migrator()
            .run(&pool)
            .await
            .expect("apply original 0026 state schema");
        pool.close().await;

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");
        let active_holder_index_exists: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM sqlite_master
WHERE type = 'index'
  AND name = 'account_leases_active_holder_idx'
            "#,
        )
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("check active holder index");

        assert_eq!(active_holder_index_exists, 1);

        drop(runtime);
        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn stale_health_event_does_not_restore_lease_eligibility() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;

        runtime
            .record_account_health_event(AccountHealthEvent {
                account_id: "acct-1".to_string(),
                pool_id: "pool-main".to_string(),
                health_state: AccountHealthState::Unauthorized,
                sequence_number: 2,
                observed_at: test_timestamp(2),
            })
            .await
            .unwrap();
        runtime
            .record_account_health_event(AccountHealthEvent {
                account_id: "acct-1".to_string(),
                pool_id: "pool-main".to_string(),
                health_state: AccountHealthState::Healthy,
                sequence_number: 1,
                observed_at: test_timestamp(1),
            })
            .await
            .unwrap();

        let health = runtime.read_account_health_state("acct-1").await.unwrap();
        let lease = runtime.acquire_account_lease("pool-main", "inst-a").await;

        assert_eq!(
            health.expect("persisted health state").health_state,
            AccountHealthState::Unauthorized
        );
        assert_eq!(lease.unwrap_err(), AccountLeaseError::NoEligibleAccount);
    }

    #[tokio::test]
    async fn renew_account_lease_rejects_expired_lease() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;

        let lease = runtime
            .acquire_account_lease("pool-main", "inst-a")
            .await
            .unwrap();
        let renewal = runtime
            .renew_account_lease(
                &lease.lease_key(),
                lease.expires_at + chrono::Duration::seconds(1),
            )
            .await
            .unwrap();

        assert_eq!(renewal, crate::LeaseRenewal::Missing);
    }

    #[tokio::test]
    async fn concurrent_acquisition_returns_one_winner_and_one_no_eligible() {
        let codex_home = unique_temp_dir();
        let runtime_a = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize first runtime");
        let runtime_b = StateRuntime::init(codex_home, "test-provider".to_string())
            .await
            .expect("initialize second runtime");
        seed_account(runtime_a.as_ref(), "acct-1").await;

        let (first, second) = tokio::join!(
            runtime_a.acquire_account_lease("pool-main", "inst-a"),
            runtime_b.acquire_account_lease("pool-main", "inst-b")
        );

        match (first, second) {
            (Ok(lease), Err(AccountLeaseError::NoEligibleAccount))
            | (Err(AccountLeaseError::NoEligibleAccount), Ok(lease)) => {
                assert_eq!(lease.account_id, "acct-1");
            }
            other => panic!("unexpected concurrent acquisition result: {other:?}"),
        }
    }

    #[tokio::test]
    async fn stale_health_event_does_not_overwrite_pool_id() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;

        runtime
            .record_account_health_event(AccountHealthEvent {
                account_id: "acct-1".to_string(),
                pool_id: "pool-new".to_string(),
                health_state: AccountHealthState::Unauthorized,
                sequence_number: 2,
                observed_at: test_timestamp(2),
            })
            .await
            .unwrap();
        runtime
            .record_account_health_event(AccountHealthEvent {
                account_id: "acct-1".to_string(),
                pool_id: "pool-stale".to_string(),
                health_state: AccountHealthState::Healthy,
                sequence_number: 1,
                observed_at: test_timestamp(1),
            })
            .await
            .unwrap();

        let health = runtime
            .read_account_health_state("acct-1")
            .await
            .unwrap()
            .expect("persisted health state");

        assert_eq!(health.pool_id, "pool-new");
        assert_eq!(health.health_state, AccountHealthState::Unauthorized);
        assert_eq!(health.last_health_event_sequence, 2);
    }

    #[tokio::test]
    async fn equal_sequence_health_event_does_not_reopen_account() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;

        runtime
            .record_account_health_event(AccountHealthEvent {
                account_id: "acct-1".to_string(),
                pool_id: "pool-main".to_string(),
                health_state: AccountHealthState::Unauthorized,
                sequence_number: 2,
                observed_at: test_timestamp(2),
            })
            .await
            .unwrap();
        runtime
            .record_account_health_event(AccountHealthEvent {
                account_id: "acct-1".to_string(),
                pool_id: "pool-main".to_string(),
                health_state: AccountHealthState::Healthy,
                sequence_number: 2,
                observed_at: test_timestamp(3),
            })
            .await
            .unwrap();

        let health = runtime
            .read_account_health_state("acct-1")
            .await
            .unwrap()
            .expect("persisted health state");
        let lease = runtime.acquire_account_lease("pool-main", "inst-a").await;

        assert_eq!(health.health_state, AccountHealthState::Unauthorized);
        assert_eq!(health.last_health_event_sequence, 2);
        assert_eq!(health.last_health_event_at, test_timestamp(2));
        assert_eq!(lease.unwrap_err(), AccountLeaseError::NoEligibleAccount);
    }

    #[tokio::test]
    async fn import_legacy_default_account_preserves_existing_startup_selection() {
        let runtime = test_runtime().await;

        runtime
            .write_account_startup_selection(crate::AccountStartupSelectionUpdate {
                default_pool_id: Some("pool-user".to_string()),
                preferred_account_id: Some("acct-user".to_string()),
                suppressed: true,
            })
            .await
            .unwrap();
        runtime
            .import_legacy_default_account(LegacyAccountImport {
                account_id: "acct-legacy".to_string(),
            })
            .await
            .unwrap();

        let selection = runtime.read_account_startup_selection().await.unwrap();

        assert_eq!(
            selection,
            crate::AccountStartupSelectionState {
                default_pool_id: Some("pool-user".to_string()),
                preferred_account_id: Some("acct-user".to_string()),
                suppressed: true,
            }
        );
    }

    #[tokio::test]
    async fn import_legacy_default_account_preserves_existing_pool_membership() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;

        runtime
            .import_legacy_default_account(LegacyAccountImport {
                account_id: "acct-1".to_string(),
            })
            .await
            .unwrap();

        let pool_id: String = sqlx::query_scalar(
            r#"
SELECT pool_id
FROM account_registry
WHERE account_id = ?
            "#,
        )
        .bind("acct-1")
        .fetch_one(runtime.pool.as_ref())
        .await
        .unwrap();

        assert_eq!(pool_id, "pool-main");
    }

    #[tokio::test]
    async fn import_legacy_default_account_uses_existing_pool_for_startup_selection() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;

        runtime
            .import_legacy_default_account(LegacyAccountImport {
                account_id: "acct-1".to_string(),
            })
            .await
            .unwrap();

        let selection = runtime.read_account_startup_selection().await.unwrap();

        assert_eq!(
            selection,
            crate::AccountStartupSelectionState {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-1".to_string()),
                suppressed: false,
            }
        );
    }

    async fn test_runtime() -> Arc<StateRuntime> {
        StateRuntime::init(unique_temp_dir(), "test-provider".to_string())
            .await
            .expect("initialize runtime")
    }

    async fn test_runtime_with_legacy_auth(account_id: &str) -> Arc<StateRuntime> {
        let runtime = test_runtime().await;
        runtime
            .import_legacy_default_account(LegacyAccountImport {
                account_id: account_id.to_string(),
            })
            .await
            .expect("import legacy account");
        runtime
    }

    async fn seed_account(runtime: &StateRuntime, account_id: &str) {
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
        .bind("pool-main")
        .bind(0_i64)
        .bind("chatgpt")
        .bind("chatgpt")
        .bind("workspace-main")
        .bind(1_i64)
        .bind(1_i64)
        .bind(1_i64)
        .execute(runtime.pool.as_ref())
        .await
        .expect("seed account");
    }

    fn test_timestamp(seconds: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(seconds, 0).expect("timestamp")
    }
}
