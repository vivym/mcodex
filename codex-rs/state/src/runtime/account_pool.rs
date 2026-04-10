use super::*;
use crate::model::AccountHealthEvent;
use crate::model::AccountHealthState;
use crate::model::AccountLeaseError;
use crate::model::AccountLeaseRecord;
use crate::model::AccountPoolHealthState;
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

        let account_id = sqlx::query_scalar::<_, String>(
            r#"
SELECT account_id
FROM account_registry
WHERE pool_id = ?
  AND healthy = 1
  AND account_id NOT IN (
      SELECT account_id
      FROM account_leases
      WHERE released_at IS NULL
  )
ORDER BY position ASC, account_id ASC
LIMIT 1
            "#,
        )
        .bind(pool_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(account_lease_storage_error)?
        .ok_or(AccountLeaseError::NoEligibleAccount)?;

        let lease_epoch = sqlx::query_scalar::<_, i64>(
            r#"
SELECT COALESCE(MAX(lease_epoch), 0) + 1
FROM account_leases
WHERE account_id = ?
            "#,
        )
        .bind(&account_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(account_lease_storage_error)?;

        let lease = AccountLeaseRecord {
            lease_id: Uuid::new_v4().to_string(),
            pool_id: pool_id.to_string(),
            account_id,
            holder_instance_id: holder_instance_id.to_string(),
            lease_epoch,
            acquired_at: now,
            renewed_at: now,
            expires_at: now + chrono::Duration::seconds(DEFAULT_ACCOUNT_LEASE_SECONDS),
            released_at: None,
        };

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
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&lease.lease_id)
        .bind(&lease.account_id)
        .bind(&lease.pool_id)
        .bind(&lease.holder_instance_id)
        .bind(lease.lease_epoch)
        .bind(account_datetime_to_epoch_seconds(lease.acquired_at))
        .bind(account_datetime_to_epoch_seconds(lease.renewed_at))
        .bind(account_datetime_to_epoch_seconds(lease.expires_at))
        .bind(Option::<i64>::None)
        .execute(&mut *tx)
        .await
        .map_err(account_lease_storage_error)?;

        tx.commit().await.map_err(account_lease_storage_error)?;

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
            "#,
        )
        .bind(account_datetime_to_epoch_seconds(now))
        .bind(account_datetime_to_epoch_seconds(expires_at))
        .bind(&lease.lease_id)
        .bind(&lease.account_id)
        .bind(lease.lease_epoch)
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
    pool_id = excluded.pool_id,
    health_state = CASE
        WHEN excluded.last_health_event_sequence >= account_runtime_state.last_health_event_sequence
            THEN excluded.health_state
        ELSE account_runtime_state.health_state
    END,
    last_health_event_sequence = MAX(
        account_runtime_state.last_health_event_sequence,
        excluded.last_health_event_sequence
    ),
    last_health_event_at = CASE
        WHEN excluded.last_health_event_sequence >= account_runtime_state.last_health_event_sequence
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
SET healthy = ?, updated_at = ?
WHERE account_id = ?
            "#,
        )
        .bind(i64::from(event.health_state.is_healthy()))
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
ON CONFLICT(account_id) DO UPDATE SET
    pool_id = excluded.pool_id,
    updated_at = excluded.updated_at
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
        .bind(LEGACY_DEFAULT_POOL_ID)
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

    pub async fn read_account_health_state(
        &self,
        account_id: &str,
    ) -> anyhow::Result<Option<AccountPoolHealthState>> {
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
            Some(row) => Ok(Some(AccountPoolHealthState {
                account_id: row.try_get("account_id")?,
                pool_id: row.try_get("pool_id")?,
                health_state: AccountHealthState::try_from(
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

#[cfg(test)]
mod tests {
    use super::StateRuntime;
    use super::test_support::unique_temp_dir;
    use crate::AccountLeaseError;
    use crate::LegacyAccountImport;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;

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
}
