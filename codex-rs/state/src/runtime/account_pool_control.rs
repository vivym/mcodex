use super::*;
use crate::model::AccountCompatMigrationState;
use crate::model::AccountPoolMembership;
use crate::model::AccountSource;
use crate::model::NewPendingAccountRegistration;
use crate::model::PendingAccountRegistration;
use crate::model::RegisteredAccountRecord;
use crate::model::RegisteredAccountUpsert;
use crate::model::account_datetime_to_epoch_seconds;
use sqlx::Executor;
use sqlx::Row;
use sqlx::Sqlite;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AccountPoolMembershipRow {
    pub(super) account_id: String,
    pub(super) pool_id: String,
    pub(super) position: i64,
    pub(super) source: Option<AccountSource>,
    pub(super) enabled: bool,
    pub(super) healthy: bool,
}

impl AccountPoolMembershipRow {
    pub(super) fn membership(self) -> AccountPoolMembership {
        AccountPoolMembership {
            account_id: self.account_id,
            pool_id: self.pool_id,
            source: self.source,
            enabled: self.enabled,
            healthy: self.healthy,
        }
    }
}

fn read_account_source(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<Option<AccountSource>> {
    row.try_get::<Option<String>, _>("source")?
        .as_deref()
        .map(AccountSource::try_from)
        .transpose()
}

pub(super) async fn read_account_pool_membership_row<'e, E>(
    executor: E,
    account_id: &str,
) -> anyhow::Result<Option<AccountPoolMembershipRow>>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query(
        r#"
SELECT
    membership.account_id,
    membership.pool_id,
    membership.position,
    account_registry.source,
    account_registry.enabled,
    account_registry.healthy
FROM account_pool_membership AS membership
JOIN account_registry
  ON account_registry.account_id = membership.account_id
WHERE membership.account_id = ?
        "#,
    )
    .bind(account_id)
    .fetch_optional(executor)
    .await?;

    row.map(|row| {
        Ok(AccountPoolMembershipRow {
            account_id: row.try_get("account_id")?,
            pool_id: row.try_get("pool_id")?,
            position: row.try_get("position")?,
            source: read_account_source(&row)?,
            enabled: row.try_get::<i64, _>("enabled")? != 0,
            healthy: row.try_get::<i64, _>("healthy")? != 0,
        })
    })
    .transpose()
}

pub(super) async fn list_account_pool_membership_rows<'e, E>(
    executor: E,
    pool_id: Option<&str>,
) -> anyhow::Result<Vec<AccountPoolMembershipRow>>
where
    E: Executor<'e, Database = Sqlite>,
{
    let rows = sqlx::query(
        r#"
SELECT
    membership.account_id,
    membership.pool_id,
    membership.position,
    account_registry.source,
    account_registry.enabled,
    account_registry.healthy
FROM account_pool_membership AS membership
JOIN account_registry
  ON account_registry.account_id = membership.account_id
WHERE (? IS NULL OR membership.pool_id = ?)
ORDER BY membership.pool_id ASC, membership.position ASC, membership.account_id ASC
        "#,
    )
    .bind(pool_id)
    .bind(pool_id)
    .fetch_all(executor)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(AccountPoolMembershipRow {
                account_id: row.try_get("account_id")?,
                pool_id: row.try_get("pool_id")?,
                position: row.try_get("position")?,
                source: read_account_source(&row)?,
                enabled: row.try_get::<i64, _>("enabled")? != 0,
                healthy: row.try_get::<i64, _>("healthy")? != 0,
            })
        })
        .collect()
}

pub(super) async fn read_account_pool_position<'e, E>(
    executor: E,
    account_id: &str,
) -> anyhow::Result<Option<i64>>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query_scalar::<_, Option<i64>>(
        r#"
SELECT COALESCE(
    (
        SELECT membership.position
        FROM account_pool_membership AS membership
        WHERE membership.account_id = ?
    ),
    (
        SELECT account_registry.position
        FROM account_registry
        WHERE account_registry.account_id = ?
    )
)
        "#,
    )
    .bind(account_id)
    .bind(account_id)
    .fetch_one(executor)
    .await
    .map_err(Into::into)
}

pub(super) async fn read_effective_account_pool_id<'e, E>(
    executor: E,
    account_id: &str,
) -> anyhow::Result<Option<String>>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query_scalar::<_, Option<String>>(
        r#"
SELECT COALESCE(
    (
        SELECT membership.pool_id
        FROM account_pool_membership AS membership
        WHERE membership.account_id = ?
    ),
    (
        SELECT account_registry.pool_id
        FROM account_registry
        WHERE account_registry.account_id = ?
    )
)
        "#,
    )
    .bind(account_id)
    .bind(account_id)
    .fetch_one(executor)
    .await
    .map_err(Into::into)
}

pub(super) async fn upsert_account_pool_membership<'e, E>(
    executor: E,
    account_id: &str,
    pool_id: &str,
    position: i64,
    assigned_at: i64,
    updated_at: i64,
) -> anyhow::Result<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query(
        r#"
INSERT INTO account_pool_membership (
    account_id,
    pool_id,
    position,
    assigned_at,
    updated_at
) VALUES (?, ?, ?, ?, ?)
ON CONFLICT(account_id) DO UPDATE SET
    pool_id = excluded.pool_id,
    position = excluded.position,
    updated_at = excluded.updated_at
        "#,
    )
    .bind(account_id)
    .bind(pool_id)
    .bind(position)
    .bind(assigned_at)
    .bind(updated_at)
    .execute(executor)
    .await?;

    Ok(())
}

pub(super) async fn sync_account_membership_compat(
    executor: &mut sqlx::SqliteConnection,
    account_id: &str,
    pool_id: &str,
    position: i64,
    updated_at: i64,
) -> anyhow::Result<bool> {
    upsert_account_pool_membership(
        &mut *executor,
        account_id,
        pool_id,
        position,
        updated_at,
        updated_at,
    )
    .await?;

    let result = sqlx::query(
        r#"
UPDATE account_registry
SET pool_id = ?, position = ?, updated_at = ?
WHERE account_id = ?
        "#,
    )
    .bind(pool_id)
    .bind(position)
    .bind(updated_at)
    .bind(account_id)
    .execute(&mut *executor)
    .await?;

    sqlx::query(
        r#"
UPDATE account_runtime_state
SET pool_id = ?, updated_at = ?
WHERE account_id = ?
        "#,
    )
    .bind(pool_id)
    .bind(updated_at)
    .bind(account_id)
    .execute(&mut *executor)
    .await?;

    Ok(result.rows_affected() != 0)
}

impl StateRuntime {
    pub async fn upsert_registered_account(
        &self,
        entry: RegisteredAccountUpsert,
    ) -> anyhow::Result<()> {
        let now = account_datetime_to_epoch_seconds(Utc::now());
        let mut tx = self.pool.begin().await?;
        let previous_pool_id = read_effective_account_pool_id(&mut *tx, &entry.account_id).await?;

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
    display_name,
    enabled,
    healthy,
    source,
    created_at,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(account_id) DO UPDATE SET
    pool_id = excluded.pool_id,
    position = excluded.position,
    account_kind = excluded.account_kind,
    backend_family = excluded.backend_family,
    workspace_id = COALESCE(excluded.workspace_id, account_registry.workspace_id),
    backend_id = excluded.backend_id,
    backend_account_handle = excluded.backend_account_handle,
    provider_fingerprint = excluded.provider_fingerprint,
    display_name = excluded.display_name,
    enabled = excluded.enabled,
    healthy = excluded.healthy,
    source = excluded.source,
    updated_at = excluded.updated_at
            "#,
        )
        .bind(&entry.account_id)
        .bind(&entry.pool_id)
        .bind(entry.position)
        .bind(&entry.account_kind)
        .bind(&entry.backend_family)
        .bind(&entry.workspace_id)
        .bind(&entry.backend_id)
        .bind(&entry.backend_account_handle)
        .bind(&entry.provider_fingerprint)
        .bind(&entry.display_name)
        .bind(i64::from(entry.enabled))
        .bind(i64::from(entry.healthy))
        .bind(entry.source.map(AccountSource::as_str))
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        upsert_account_pool_membership(
            &mut *tx,
            &entry.account_id,
            &entry.pool_id,
            entry.position,
            now,
            now,
        )
        .await?;

        sqlx::query(
            r#"
DELETE FROM account_runtime_state
WHERE account_id = ?
  AND (
      (? != 0 AND health_state != 'healthy')
      OR (? = 0 AND health_state = 'healthy')
  )
            "#,
        )
        .bind(&entry.account_id)
        .bind(i64::from(entry.healthy))
        .bind(i64::from(entry.healthy))
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
UPDATE account_runtime_state
SET pool_id = ?, updated_at = ?
WHERE account_id = ?
            "#,
        )
        .bind(&entry.pool_id)
        .bind(now)
        .bind(&entry.account_id)
        .execute(&mut *tx)
        .await?;

        if previous_pool_id
            .as_deref()
            .is_some_and(|pool_id| pool_id != entry.pool_id)
            || !entry.enabled
            || !entry.healthy
        {
            super::account_pool::release_unreleased_account_leases(
                &mut *tx,
                &entry.account_id,
                now,
            )
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn assign_account_pool_membership(
        &self,
        account_id: &str,
        pool_id: &str,
    ) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;
        let updated_at = account_datetime_to_epoch_seconds(Utc::now());
        let position = read_account_pool_position(&mut *tx, account_id)
            .await?
            .unwrap_or(0);
        let updated =
            sync_account_membership_compat(&mut tx, account_id, pool_id, position, updated_at)
                .await?;
        if !updated {
            tx.rollback().await?;
            return Ok(false);
        }

        super::account_pool::release_unreleased_account_leases(&mut *tx, account_id, updated_at)
            .await?;

        tx.commit().await?;
        Ok(true)
    }

    pub async fn create_pending_account_registration(
        &self,
        entry: NewPendingAccountRegistration,
    ) -> anyhow::Result<()> {
        let now = account_datetime_to_epoch_seconds(Utc::now());
        sqlx::query(
            r#"
INSERT INTO pending_account_registration (
    idempotency_key,
    backend_id,
    provider_kind,
    target_pool_id,
    backend_account_handle,
    account_id,
    started_at,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(idempotency_key) DO UPDATE SET
    backend_id = excluded.backend_id,
    provider_kind = excluded.provider_kind,
    target_pool_id = excluded.target_pool_id,
    backend_account_handle = excluded.backend_account_handle,
    account_id = excluded.account_id,
    updated_at = excluded.updated_at
            "#,
        )
        .bind(&entry.idempotency_key)
        .bind(&entry.backend_id)
        .bind(&entry.provider_kind)
        .bind(&entry.target_pool_id)
        .bind(&entry.backend_account_handle)
        .bind(&entry.account_id)
        .bind(now)
        .bind(now)
        .execute(self.pool.as_ref())
        .await?;

        Ok(())
    }

    pub async fn list_pending_account_registrations(
        &self,
    ) -> anyhow::Result<Vec<PendingAccountRegistration>> {
        let rows = sqlx::query(
            r#"
SELECT
    idempotency_key,
    backend_id,
    provider_kind,
    target_pool_id,
    backend_account_handle,
    account_id
FROM pending_account_registration
ORDER BY started_at ASC, idempotency_key ASC
            "#,
        )
        .fetch_all(self.pool.as_ref())
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(PendingAccountRegistration {
                    idempotency_key: row.try_get("idempotency_key")?,
                    backend_id: row.try_get("backend_id")?,
                    provider_kind: row.try_get("provider_kind")?,
                    target_pool_id: row.try_get("target_pool_id")?,
                    backend_account_handle: row.try_get("backend_account_handle")?,
                    account_id: row.try_get("account_id")?,
                })
            })
            .collect()
    }

    pub async fn read_pending_account_registration(
        &self,
        idempotency_key: &str,
    ) -> anyhow::Result<Option<PendingAccountRegistration>> {
        let row = sqlx::query(
            r#"
SELECT
    idempotency_key,
    backend_id,
    provider_kind,
    target_pool_id,
    backend_account_handle,
    account_id
FROM pending_account_registration
WHERE idempotency_key = ?
            "#,
        )
        .bind(idempotency_key)
        .fetch_optional(self.pool.as_ref())
        .await?;

        row.map(|row| {
            Ok(PendingAccountRegistration {
                idempotency_key: row.try_get("idempotency_key")?,
                backend_id: row.try_get("backend_id")?,
                provider_kind: row.try_get("provider_kind")?,
                target_pool_id: row.try_get("target_pool_id")?,
                backend_account_handle: row.try_get("backend_account_handle")?,
                account_id: row.try_get("account_id")?,
            })
        })
        .transpose()
    }

    pub async fn finalize_pending_account_registration(
        &self,
        idempotency_key: &str,
        backend_account_handle: &str,
        account_id: &str,
    ) -> anyhow::Result<bool> {
        let result = sqlx::query(
            r#"
UPDATE pending_account_registration
SET backend_account_handle = ?, account_id = ?, updated_at = ?
WHERE idempotency_key = ?
            "#,
        )
        .bind(backend_account_handle)
        .bind(account_id)
        .bind(account_datetime_to_epoch_seconds(Utc::now()))
        .bind(idempotency_key)
        .execute(self.pool.as_ref())
        .await?;

        Ok(result.rows_affected() != 0)
    }

    pub async fn clear_pending_account_registration(
        &self,
        idempotency_key: &str,
    ) -> anyhow::Result<bool> {
        let result = sqlx::query(
            r#"
DELETE FROM pending_account_registration
WHERE idempotency_key = ?
            "#,
        )
        .bind(idempotency_key)
        .execute(self.pool.as_ref())
        .await?;

        Ok(result.rows_affected() != 0)
    }

    pub async fn read_account_compat_migration_state(
        &self,
    ) -> anyhow::Result<AccountCompatMigrationState> {
        let legacy_import_completed: i64 = sqlx::query_scalar(
            r#"
SELECT legacy_import_completed
FROM account_compat_migration_state
WHERE singleton = 1
            "#,
        )
        .fetch_one(self.pool.as_ref())
        .await?;

        Ok(AccountCompatMigrationState {
            legacy_import_completed: legacy_import_completed != 0,
        })
    }

    pub async fn write_account_compat_migration_state(
        &self,
        completed: bool,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
INSERT INTO account_compat_migration_state (
    singleton,
    legacy_import_completed,
    updated_at
) VALUES (1, ?, ?)
ON CONFLICT(singleton) DO UPDATE SET
    legacy_import_completed = excluded.legacy_import_completed,
    updated_at = excluded.updated_at
            "#,
        )
        .bind(i64::from(completed))
        .bind(account_datetime_to_epoch_seconds(Utc::now()))
        .execute(self.pool.as_ref())
        .await?;

        Ok(())
    }

    pub async fn read_registered_account(
        &self,
        account_id: &str,
    ) -> anyhow::Result<Option<RegisteredAccountRecord>> {
        let row = sqlx::query(
            r#"
SELECT
    account_id,
    backend_id,
    backend_family,
    workspace_id,
    backend_account_handle,
    account_kind,
    provider_fingerprint,
    display_name,
    source,
    enabled,
    healthy
FROM account_registry
WHERE account_id = ?
            "#,
        )
        .bind(account_id)
        .fetch_optional(self.pool.as_ref())
        .await?;

        row.map(|row| {
            Ok(RegisteredAccountRecord {
                account_id: row.try_get("account_id")?,
                backend_id: row.try_get("backend_id")?,
                backend_family: row.try_get("backend_family")?,
                workspace_id: row.try_get("workspace_id")?,
                backend_account_handle: row.try_get("backend_account_handle")?,
                account_kind: row.try_get("account_kind")?,
                provider_fingerprint: row.try_get("provider_fingerprint")?,
                display_name: row.try_get("display_name")?,
                source: read_account_source(&row)?,
                enabled: row.try_get::<i64, _>("enabled")? != 0,
                healthy: row.try_get::<i64, _>("healthy")? != 0,
            })
        })
        .transpose()
    }
}
