use super::*;
use crate::model::AccountHealthEvent;
use crate::model::AccountHealthState;
use crate::model::AccountLeaseError;
use crate::model::AccountLeaseRecord;
use crate::model::AccountPoolAccountDiagnostic;
use crate::model::AccountPoolDiagnostic;
use crate::model::AccountPoolMembership;
use crate::model::AccountRegistryEntryUpdate;
use crate::model::AccountSource;
use crate::model::AccountStartupEligibility;
use crate::model::AccountStartupSelectionPreview;
use crate::model::AccountStartupSelectionState;
use crate::model::AccountStartupSelectionUpdate;
use crate::model::LeaseKey;
use crate::model::LeaseRenewal;
use crate::model::LegacyAccountImport;
use crate::model::account_datetime_to_epoch_seconds;
use crate::model::account_epoch_seconds_to_datetime;
use sqlx::Executor;
use uuid::Uuid;

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
        lease_ttl: chrono::Duration,
    ) -> std::result::Result<AccountLeaseRecord, AccountLeaseError> {
        self.acquire_account_lease_excluding(pool_id, holder_instance_id, lease_ttl, &[])
            .await
    }

    pub async fn acquire_account_lease_excluding(
        &self,
        pool_id: &str,
        holder_instance_id: &str,
        lease_ttl: chrono::Duration,
        excluded_account_ids: &[String],
    ) -> std::result::Result<AccountLeaseRecord, AccountLeaseError> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(account_lease_storage_error)?;
        release_expired_account_leases(&mut *tx, now)
            .await
            .map_err(account_lease_storage_error)?;

        if let Some(existing_lease) = load_active_holder_lease(&mut *tx, holder_instance_id, now)
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
            expires_at: now + lease_ttl,
            released_at: None,
        };

        let result =
            insert_next_eligible_lease(&mut *tx, &lease, pool_id, excluded_account_ids).await;

        let result = match result {
            Ok(result) => result,
            Err(err) if account_lease_is_contention_error(&err) => {
                let existing_lease =
                    load_active_holder_lease(self.pool.as_ref(), holder_instance_id, Utc::now())
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

    pub async fn acquire_preferred_account_lease(
        &self,
        pool_id: &str,
        account_id: &str,
        holder_instance_id: &str,
        lease_ttl: chrono::Duration,
    ) -> std::result::Result<AccountLeaseRecord, AccountLeaseError> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(account_lease_storage_error)?;
        release_expired_account_leases(&mut *tx, now)
            .await
            .map_err(account_lease_storage_error)?;

        if let Some(existing_lease) = load_active_holder_lease(&mut *tx, holder_instance_id, now)
            .await
            .map_err(|err| AccountLeaseError::Storage(err.to_string()))?
        {
            tx.commit().await.map_err(account_lease_storage_error)?;
            return Ok(existing_lease);
        }

        let lease = AccountLeaseRecord {
            lease_id: Uuid::new_v4().to_string(),
            pool_id: pool_id.to_string(),
            account_id: account_id.to_string(),
            holder_instance_id: holder_instance_id.to_string(),
            lease_epoch: 0,
            acquired_at: now,
            renewed_at: now,
            expires_at: now + lease_ttl,
            released_at: None,
        };

        let result = insert_requested_lease(&mut *tx, &lease, account_id).await;

        let result = match result {
            Ok(result) => result,
            Err(err) if account_lease_is_contention_error(&err) => {
                let existing_lease =
                    load_active_holder_lease(self.pool.as_ref(), holder_instance_id, Utc::now())
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
        lease_ttl: chrono::Duration,
    ) -> anyhow::Result<LeaseRenewal> {
        let expires_at = now + lease_ttl;
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

    pub async fn release_account_lease(
        &self,
        lease: &LeaseKey,
        now: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let result = sqlx::query(
            r#"
UPDATE account_leases
SET released_at = ?
WHERE lease_id = ?
  AND account_id = ?
  AND lease_epoch = ?
  AND released_at IS NULL
            "#,
        )
        .bind(account_datetime_to_epoch_seconds(now))
        .bind(&lease.lease_id)
        .bind(&lease.account_id)
        .bind(lease.lease_epoch)
        .execute(self.pool.as_ref())
        .await?;

        Ok(result.rows_affected() != 0)
    }

    pub async fn read_active_holder_lease(
        &self,
        holder_instance_id: &str,
    ) -> anyhow::Result<Option<AccountLeaseRecord>> {
        load_active_holder_lease(self.pool.as_ref(), holder_instance_id, Utc::now()).await
    }

    pub async fn read_account_pool_diagnostic(
        &self,
        pool_id: &str,
        preferred_account_id: Option<&str>,
    ) -> anyhow::Result<AccountPoolDiagnostic> {
        let now = Utc::now();
        let rows = sqlx::query(
            r#"
SELECT
    membership.account_id,
    membership.pool_id,
    account_registry.source,
    account_registry.enabled,
    account_registry.healthy,
    account_runtime_state.health_state,
    active_lease.lease_id,
    active_lease.holder_instance_id,
    active_lease.lease_epoch,
    active_lease.acquired_at,
    active_lease.renewed_at,
    active_lease.expires_at,
    active_lease.released_at
FROM account_pool_membership AS membership
JOIN account_registry
  ON account_registry.account_id = membership.account_id
LEFT JOIN account_runtime_state
  ON account_runtime_state.account_id = membership.account_id
LEFT JOIN account_leases AS active_lease
  ON active_lease.account_id = membership.account_id
 AND active_lease.pool_id = membership.pool_id
 AND active_lease.released_at IS NULL
 AND active_lease.expires_at > ?
WHERE membership.pool_id = ?
ORDER BY
    CASE
        WHEN ? IS NOT NULL AND membership.account_id = ? THEN 0
        ELSE 1
    END,
    membership.position ASC,
    membership.account_id ASC
            "#,
        )
        .bind(account_datetime_to_epoch_seconds(now))
        .bind(pool_id)
        .bind(preferred_account_id)
        .bind(preferred_account_id)
        .fetch_all(self.pool.as_ref())
        .await?;

        let mut accounts = Vec::with_capacity(rows.len());
        let mut any_eligible_now = false;
        let mut next_eligible_at = None;
        let mut preferred_account_found = false;
        let mut preferred_account_eligible_now = false;
        let mut preferred_account_next_eligible_at = None;
        for row in rows {
            let account_id: String = row.try_get("account_id")?;
            let account_pool_id: String = row.try_get("pool_id")?;
            let source = row
                .try_get::<Option<String>, _>("source")?
                .as_deref()
                .map(AccountSource::try_from)
                .transpose()?;
            let enabled = row.try_get::<i64, _>("enabled")? != 0;
            let healthy = row.try_get::<i64, _>("healthy")? != 0;
            let health_state = row
                .try_get::<Option<String>, _>("health_state")?
                .as_deref()
                .map(AccountHealthState::try_from)
                .transpose()?;
            let active_lease = match row.try_get::<Option<String>, _>("lease_id")? {
                Some(lease_id) => Some(AccountLeaseRecord {
                    lease_id,
                    pool_id: account_pool_id.clone(),
                    account_id: account_id.clone(),
                    holder_instance_id: row.try_get("holder_instance_id")?,
                    lease_epoch: row.try_get("lease_epoch")?,
                    acquired_at: account_epoch_seconds_to_datetime(row.try_get("acquired_at")?)?,
                    renewed_at: account_epoch_seconds_to_datetime(row.try_get("renewed_at")?)?,
                    expires_at: account_epoch_seconds_to_datetime(row.try_get("expires_at")?)?,
                    released_at: row
                        .try_get::<Option<i64>, _>("released_at")?
                        .map(account_epoch_seconds_to_datetime)
                        .transpose()?,
                }),
                None => None,
            };
            let account_next_eligible_at = if enabled && healthy {
                active_lease.as_ref().map(|lease| lease.expires_at)
            } else {
                None
            };
            let is_preferred =
                preferred_account_id.is_some_and(|preferred| preferred == account_id);
            let eligibility = if is_preferred && !enabled {
                AccountStartupEligibility::PreferredAccountDisabled
            } else if active_lease.is_some() {
                AccountStartupEligibility::PreferredAccountBusy
            } else if !healthy {
                AccountStartupEligibility::PreferredAccountUnhealthy
            } else if !enabled {
                AccountStartupEligibility::NoEligibleAccount
            } else if is_preferred {
                any_eligible_now = true;
                AccountStartupEligibility::PreferredAccountSelected
            } else {
                any_eligible_now = true;
                AccountStartupEligibility::AutomaticAccountSelected
            };
            if is_preferred {
                preferred_account_found = true;
                preferred_account_eligible_now = matches!(
                    eligibility,
                    AccountStartupEligibility::PreferredAccountSelected
                );
                preferred_account_next_eligible_at = account_next_eligible_at;
            }
            if preferred_account_id.is_none() && !any_eligible_now {
                next_eligible_at = match (next_eligible_at, account_next_eligible_at) {
                    (None, next) => next,
                    (Some(current), Some(next)) => Some(current.min(next)),
                    (current, None) => current,
                };
            }
            accounts.push(AccountPoolAccountDiagnostic {
                account_id,
                pool_id: account_pool_id,
                source,
                enabled,
                healthy,
                active_lease,
                health_state,
                eligibility,
                next_eligible_at: account_next_eligible_at,
            });
        }

        Ok(AccountPoolDiagnostic {
            pool_id: pool_id.to_string(),
            accounts,
            next_eligible_at: if preferred_account_found {
                if preferred_account_eligible_now {
                    None
                } else {
                    preferred_account_next_eligible_at
                }
            } else if any_eligible_now {
                None
            } else {
                next_eligible_at
            },
        })
    }

    pub async fn read_account_health_event_sequence(
        &self,
        account_id: &str,
    ) -> anyhow::Result<Option<i64>> {
        let sequence = sqlx::query_scalar(
            r#"
SELECT last_health_event_sequence
FROM account_runtime_state
WHERE account_id = ?
            "#,
        )
        .bind(account_id)
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(sequence)
    }

    pub async fn record_account_health_event(
        &self,
        event: AccountHealthEvent,
    ) -> anyhow::Result<()> {
        // This transaction reads membership state and then writes runtime health state. Starting
        // with BEGIN IMMEDIATE avoids deferred read->write upgrade races, and the bounded retry
        // lets us recover if another writer holds the lock longer than SQLite's busy timeout.
        for attempt in 0..5 {
            let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
                Ok(tx) => tx,
                Err(err) if err.to_string().contains("database is locked") && attempt < 4 => {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
                Err(err) => return Err(err.into()),
            };
            let updated_at = account_datetime_to_epoch_seconds(Utc::now());
            let observed_at = account_datetime_to_epoch_seconds(event.observed_at);
            let result = async {
                let has_membership = super::account_pool_control::read_account_pool_membership_row(
                    &mut *tx,
                    &event.account_id,
                )
                .await?
                .is_some();

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

                let update_registry_query = if has_membership {
                    r#"
UPDATE account_registry
SET pool_id = COALESCE((
        SELECT pool_id
        FROM account_runtime_state
        WHERE account_runtime_state.account_id = account_registry.account_id
    ), account_registry.pool_id),
    healthy = CASE
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
            "#
                } else {
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
            "#
                };
                sqlx::query(update_registry_query)
                    .bind(updated_at)
                    .bind(&event.account_id)
                    .execute(&mut *tx)
                    .await?;

                if has_membership {
                    let runtime_pool_id: Option<String> = sqlx::query_scalar(
                        r#"
SELECT pool_id
FROM account_runtime_state
WHERE account_id = ?
            "#,
                    )
                    .bind(&event.account_id)
                    .fetch_optional(&mut *tx)
                    .await?;
                    if let Some(pool_id) = runtime_pool_id {
                        let position = super::account_pool_control::read_account_pool_position(
                            &mut *tx,
                            &event.account_id,
                        )
                        .await?
                        .unwrap_or(0);
                        super::account_pool_control::sync_account_membership_compat(
                            &mut tx,
                            &event.account_id,
                            &pool_id,
                            position,
                            updated_at,
                        )
                        .await?;
                    }
                }

                tx.commit().await?;
                Ok::<(), anyhow::Error>(())
            }
            .await;

            match result {
                Ok(()) => return Ok(()),
                Err(err) if err.to_string().contains("database is locked") && attempt < 4 => {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                Err(err) => return Err(err),
            }
        }

        Err(anyhow::anyhow!(
            "record_account_health_event exceeded retry budget"
        ))
    }

    pub async fn import_legacy_default_account(
        &self,
        legacy_account: LegacyAccountImport,
    ) -> anyhow::Result<()> {
        let now = account_datetime_to_epoch_seconds(Utc::now());
        let mut tx = self.pool.begin().await?;
        let existing_membership = super::account_pool_control::read_account_pool_membership_row(
            &mut *tx,
            &legacy_account.account_id,
        )
        .await?;
        let default_pool_id = existing_membership
            .as_ref()
            .map(|membership| membership.pool_id.clone())
            .unwrap_or_else(|| LEGACY_DEFAULT_POOL_ID.to_string());
        let default_position = existing_membership
            .map(|membership| membership.position)
            .unwrap_or(0);

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
    enabled,
    healthy,
    source,
    created_at,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(account_id) DO UPDATE SET
    source = CASE
        WHEN account_registry.source IS NOT NULL THEN account_registry.source
        WHEN account_registry.pool_id = 'legacy-default' THEN account_registry.source
        ELSE excluded.source
    END
            "#,
        )
        .bind(&legacy_account.account_id)
        .bind(LEGACY_DEFAULT_POOL_ID)
        .bind(0_i64)
        .bind("chatgpt")
        .bind("chatgpt")
        .bind(Option::<String>::None)
        .bind("local")
        .bind(legacy_backend_account_handle(&legacy_account.account_id))
        .bind(legacy_provider_fingerprint(
            "chatgpt",
            None,
            &legacy_account.account_id,
        ))
        .bind(1_i64)
        .bind(1_i64)
        .bind(AccountSource::Migrated.as_str())
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        super::account_pool_control::upsert_account_pool_membership(
            &mut *tx,
            &legacy_account.account_id,
            &default_pool_id,
            default_position,
            now,
            now,
        )
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
    updated_at = excluded.updated_at
WHERE account_startup_selection.default_pool_id IS NULL
  AND account_startup_selection.preferred_account_id IS NULL
  AND account_startup_selection.suppressed = 0
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

    pub async fn preview_account_startup_selection(
        &self,
        configured_default_pool_id: Option<&str>,
    ) -> anyhow::Result<AccountStartupSelectionPreview> {
        let selection = self.read_account_startup_selection().await?;
        let effective_pool_id = configured_default_pool_id
            .map(ToOwned::to_owned)
            .or_else(|| selection.default_pool_id.clone());

        if selection.suppressed {
            return Ok(AccountStartupSelectionPreview {
                effective_pool_id,
                preferred_account_id: selection.preferred_account_id,
                suppressed: true,
                predicted_account_id: None,
                eligibility: AccountStartupEligibility::Suppressed,
            });
        }

        let Some(pool_id) = effective_pool_id.clone() else {
            return Ok(AccountStartupSelectionPreview {
                effective_pool_id: None,
                preferred_account_id: selection.preferred_account_id,
                suppressed: false,
                predicted_account_id: None,
                eligibility: AccountStartupEligibility::MissingPool,
            });
        };

        if let Some(preferred_account_id) = selection.preferred_account_id.clone() {
            let membership = self
                .read_account_pool_membership(&preferred_account_id)
                .await?;
            let eligibility = match membership {
                None => AccountStartupEligibility::PreferredAccountMissing,
                Some(membership) if membership.pool_id != pool_id => {
                    AccountStartupEligibility::PreferredAccountInOtherPool {
                        actual_pool_id: membership.pool_id,
                    }
                }
                Some(membership) if !membership.enabled => {
                    AccountStartupEligibility::PreferredAccountDisabled
                }
                Some(membership) if !membership.healthy => {
                    AccountStartupEligibility::PreferredAccountUnhealthy
                }
                Some(_)
                    if account_has_active_lease(
                        self.pool.as_ref(),
                        &preferred_account_id,
                        Utc::now(),
                    )
                    .await? =>
                {
                    AccountStartupEligibility::PreferredAccountBusy
                }
                Some(_) => AccountStartupEligibility::PreferredAccountSelected,
            };
            let predicted_account_id = match eligibility {
                AccountStartupEligibility::PreferredAccountSelected => {
                    Some(preferred_account_id.clone())
                }
                AccountStartupEligibility::Suppressed
                | AccountStartupEligibility::MissingPool
                | AccountStartupEligibility::AutomaticAccountSelected
                | AccountStartupEligibility::PreferredAccountMissing
                | AccountStartupEligibility::PreferredAccountInOtherPool { .. }
                | AccountStartupEligibility::PreferredAccountDisabled
                | AccountStartupEligibility::PreferredAccountUnhealthy
                | AccountStartupEligibility::PreferredAccountBusy
                | AccountStartupEligibility::NoEligibleAccount => None,
            };

            return Ok(AccountStartupSelectionPreview {
                effective_pool_id: Some(pool_id),
                preferred_account_id: Some(preferred_account_id),
                suppressed: false,
                predicted_account_id,
                eligibility,
            });
        }

        let predicted_account_id =
            read_first_eligible_account_id(self.pool.as_ref(), &pool_id, Utc::now()).await?;
        let eligibility = if predicted_account_id.is_some() {
            AccountStartupEligibility::AutomaticAccountSelected
        } else {
            AccountStartupEligibility::NoEligibleAccount
        };

        Ok(AccountStartupSelectionPreview {
            effective_pool_id: Some(pool_id),
            preferred_account_id: None,
            suppressed: false,
            predicted_account_id,
            eligibility,
        })
    }

    pub async fn read_account_pool_membership(
        &self,
        account_id: &str,
    ) -> anyhow::Result<Option<AccountPoolMembership>> {
        super::account_pool_control::read_account_pool_membership_row(
            self.pool.as_ref(),
            account_id,
        )
        .await
        .map(|membership| {
            membership.map(super::account_pool_control::AccountPoolMembershipRow::membership)
        })
    }

    pub async fn upsert_account_registry_entry(
        &self,
        entry: AccountRegistryEntryUpdate,
    ) -> anyhow::Result<()> {
        let now = account_datetime_to_epoch_seconds(Utc::now());
        let mut tx = self.pool.begin().await?;
        let previous_pool_id = super::account_pool_control::read_effective_account_pool_id(
            &mut *tx,
            &entry.account_id,
        )
        .await?;

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
    enabled,
    healthy,
    created_at,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(account_id) DO UPDATE SET
    pool_id = excluded.pool_id,
    position = excluded.position,
    account_kind = excluded.account_kind,
    backend_family = excluded.backend_family,
    workspace_id = excluded.workspace_id,
    backend_id = excluded.backend_id,
    backend_account_handle = excluded.backend_account_handle,
    provider_fingerprint = excluded.provider_fingerprint,
    enabled = excluded.enabled,
    healthy = excluded.healthy,
    updated_at = excluded.updated_at
            "#,
        )
        .bind(&entry.account_id)
        .bind(&entry.pool_id)
        .bind(entry.position)
        .bind(&entry.account_kind)
        .bind(&entry.backend_family)
        .bind(&entry.workspace_id)
        .bind("local")
        .bind(legacy_backend_account_handle(&entry.account_id))
        .bind(legacy_provider_fingerprint(
            &entry.account_kind,
            entry.workspace_id.as_deref(),
            &entry.account_id,
        ))
        .bind(i64::from(entry.enabled))
        .bind(i64::from(entry.healthy))
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        super::account_pool_control::upsert_account_pool_membership(
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
            release_unreleased_account_leases(&mut *tx, &entry.account_id, now).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn set_account_enabled(
        &self,
        account_id: &str,
        enabled: bool,
    ) -> anyhow::Result<bool> {
        let updated_at = account_datetime_to_epoch_seconds(Utc::now());
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            r#"
UPDATE account_registry
SET enabled = ?, updated_at = ?
WHERE account_id = ?
            "#,
        )
        .bind(i64::from(enabled))
        .bind(updated_at)
        .bind(account_id)
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() != 0 && !enabled {
            release_unreleased_account_leases(&mut *tx, account_id, updated_at).await?;
        }

        tx.commit().await?;
        Ok(result.rows_affected() != 0)
    }

    pub async fn remove_account_registry_entry(&self, account_id: &str) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;
        let updated_at = account_datetime_to_epoch_seconds(Utc::now());
        let result = sqlx::query(
            r#"
DELETE FROM account_registry
WHERE account_id = ?
            "#,
        )
        .bind(account_id)
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() != 0 {
            sqlx::query(
                r#"
UPDATE account_startup_selection
SET preferred_account_id = NULL, updated_at = ?
WHERE preferred_account_id = ?
                "#,
            )
            .bind(updated_at)
            .bind(account_id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(result.rows_affected() != 0)
    }

    pub async fn assign_account_pool(
        &self,
        account_id: &str,
        pool_id: &str,
    ) -> anyhow::Result<bool> {
        self.assign_account_pool_membership(account_id, pool_id)
            .await
    }

    pub async fn list_account_pool_memberships(
        &self,
        pool_id: Option<&str>,
    ) -> anyhow::Result<Vec<AccountPoolMembership>> {
        super::account_pool_control::list_account_pool_membership_rows(self.pool.as_ref(), pool_id)
            .await
            .map(|memberships| {
                memberships
                    .into_iter()
                    .map(super::account_pool_control::AccountPoolMembershipRow::membership)
                    .collect()
            })
    }

    pub async fn read_account_pool_position(
        &self,
        account_id: &str,
    ) -> anyhow::Result<Option<i64>> {
        super::account_pool_control::read_account_pool_position(self.pool.as_ref(), account_id)
            .await
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

async fn release_expired_account_leases<'e, E>(
    executor: E,
    now: DateTime<Utc>,
) -> std::result::Result<(), sqlx::Error>
where
    E: Executor<'e, Database = Sqlite>,
{
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
    .execute(executor)
    .await?;
    Ok(())
}

pub(super) async fn release_unreleased_account_leases<'e, E>(
    executor: E,
    account_id: &str,
    released_at: i64,
) -> std::result::Result<(), sqlx::Error>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query(
        r#"
UPDATE account_leases
SET released_at = ?
WHERE account_id = ?
  AND released_at IS NULL
        "#,
    )
    .bind(released_at)
    .bind(account_id)
    .execute(executor)
    .await?;
    Ok(())
}

async fn insert_next_eligible_lease<'e, E>(
    executor: E,
    lease: &AccountLeaseRecord,
    pool_id: &str,
    excluded_account_ids: &[String],
) -> std::result::Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error>
where
    E: Executor<'e, Database = Sqlite>,
{
    let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
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
        "#,
    );
    query.push_bind(&lease.lease_id);
    query.push(
        r#",
    membership.account_id,
        "#,
    );
    query.push_bind(&lease.pool_id);
    query.push(", ");
    query.push_bind(&lease.holder_instance_id);
    query.push(
        r#",
    COALESCE((
        SELECT MAX(existing.lease_epoch) + 1
        FROM account_leases AS existing
        WHERE existing.account_id = membership.account_id
    ), 1),
        "#,
    );
    query.push_bind(account_datetime_to_epoch_seconds(lease.acquired_at));
    query.push(", ");
    query.push_bind(account_datetime_to_epoch_seconds(lease.renewed_at));
    query.push(", ");
    query.push_bind(account_datetime_to_epoch_seconds(lease.expires_at));
    query.push(
        r#",
    NULL
FROM account_pool_membership AS membership
JOIN account_registry
  ON account_registry.account_id = membership.account_id
WHERE membership.pool_id = 
        "#,
    );
    query.push_bind(pool_id);
    query.push(
        r#"
  AND account_registry.enabled = 1
  AND account_registry.healthy = 1
  AND NOT EXISTS (
      SELECT 1
      FROM account_leases
      WHERE account_leases.account_id = membership.account_id
        AND account_leases.released_at IS NULL
  )
        "#,
    );
    if !excluded_account_ids.is_empty() {
        query.push("  AND membership.account_id NOT IN (");
        let mut separated = query.separated(", ");
        for account_id in excluded_account_ids {
            separated.push_bind(account_id);
        }
        query.push(")\n");
    }
    query.push(
        r#"ORDER BY membership.position ASC, membership.account_id ASC
LIMIT 1
        "#,
    );
    query.build().execute(executor).await
}

async fn insert_requested_lease<'e, E>(
    executor: E,
    lease: &AccountLeaseRecord,
    account_id: &str,
) -> std::result::Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error>
where
    E: Executor<'e, Database = Sqlite>,
{
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
) SELECT
    ?,
    membership.account_id,
    ?,
    ?,
    COALESCE((
        SELECT MAX(existing.lease_epoch) + 1
        FROM account_leases AS existing
        WHERE existing.account_id = membership.account_id
    ), 1),
    ?,
    ?,
    ?,
    NULL
FROM account_pool_membership AS membership
JOIN account_registry
  ON account_registry.account_id = membership.account_id
WHERE membership.pool_id = ?
  AND membership.account_id = ?
  AND account_registry.enabled = 1
  AND account_registry.healthy = 1
  AND NOT EXISTS (
      SELECT 1
      FROM account_leases
      WHERE account_leases.account_id = membership.account_id
        AND account_leases.released_at IS NULL
  )
LIMIT 1
        "#,
    )
    .bind(&lease.lease_id)
    .bind(&lease.pool_id)
    .bind(&lease.holder_instance_id)
    .bind(account_datetime_to_epoch_seconds(lease.acquired_at))
    .bind(account_datetime_to_epoch_seconds(lease.renewed_at))
    .bind(account_datetime_to_epoch_seconds(lease.expires_at))
    .bind(&lease.pool_id)
    .bind(account_id)
    .execute(executor)
    .await
}

async fn account_has_active_lease<'e, E>(
    executor: E,
    account_id: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<bool>
where
    E: Executor<'e, Database = Sqlite>,
{
    let count: i64 = sqlx::query_scalar(
        r#"
SELECT COUNT(*)
FROM account_leases
WHERE account_id = ?
  AND released_at IS NULL
  AND expires_at > ?
        "#,
    )
    .bind(account_id)
    .bind(account_datetime_to_epoch_seconds(now))
    .fetch_one(executor)
    .await?;
    Ok(count != 0)
}

async fn read_first_eligible_account_id<'e, E>(
    executor: E,
    pool_id: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<Option<String>>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query_scalar(
        r#"
SELECT membership.account_id
FROM account_pool_membership AS membership
JOIN account_registry
  ON account_registry.account_id = membership.account_id
WHERE membership.pool_id = ?
  AND account_registry.enabled = 1
  AND account_registry.healthy = 1
  AND NOT EXISTS (
      SELECT 1
      FROM account_leases
      WHERE account_leases.account_id = membership.account_id
        AND account_leases.released_at IS NULL
        AND account_leases.expires_at > ?
  )
ORDER BY membership.position ASC, membership.account_id ASC
LIMIT 1
        "#,
    )
    .bind(pool_id)
    .bind(account_datetime_to_epoch_seconds(now))
    .fetch_optional(executor)
    .await
    .map_err(Into::into)
}

fn legacy_backend_account_handle(account_id: &str) -> String {
    account_id.to_string()
}

fn legacy_provider_fingerprint(
    account_kind: &str,
    workspace_id: Option<&str>,
    account_id: &str,
) -> String {
    format!(
        "legacy:{account_kind}:{}:{account_id}",
        workspace_id.unwrap_or("")
    )
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
    now: DateTime<Utc>,
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
  AND expires_at > ?
ORDER BY acquired_at DESC, lease_id DESC
LIMIT 1
        "#,
    )
    .bind(holder_instance_id)
    .bind(account_datetime_to_epoch_seconds(now))
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
    use crate::AccountPoolAccountDiagnostic;
    use crate::AccountPoolMembership;
    use crate::AccountRegistryEntryUpdate;
    use crate::AccountSource;
    use crate::AccountStartupEligibility;
    use crate::AccountStartupSelectionPreview;
    use crate::LeaseRenewal;
    use crate::LegacyAccountImport;
    use crate::NewPendingAccountRegistration;
    use crate::RegisteredAccountMembership;
    use crate::RegisteredAccountRecord;
    use crate::RegisteredAccountUpsert;
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
    const ACCOUNT_REGISTRY_SOURCE_MIGRATION_VERSION: i64 = 28;

    const HISTORICAL_MODIFIED_0026_SQL: &str = r#"WITH ranked_active_leases AS (
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
);

CREATE UNIQUE INDEX account_leases_active_holder_idx
ON account_leases(holder_instance_id)
WHERE released_at IS NULL;
"#;

    fn original_0026_migrator() -> Migrator {
        migration_0026_migrator(ORIGINAL_ACTIVE_HOLDER_INDEX_SQL)
    }

    fn historical_modified_0026_migrator() -> Migrator {
        migration_0026_migrator(HISTORICAL_MODIFIED_0026_SQL)
    }

    fn migration_0026_migrator(migration_sql: &'static str) -> Migrator {
        let mut migrations = STATE_MIGRATOR.migrations.to_vec();
        let current_0026_index = migrations
            .iter()
            .position(|migration| migration.version == 26)
            .expect("current 0026 migration");
        let current_0026 = migrations[current_0026_index].clone();
        migrations.remove(current_0026_index);
        migrations.push(Migration::new(
            26,
            current_0026.description.clone(),
            current_0026.migration_type,
            Cow::Borrowed(migration_sql),
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
            .acquire_account_lease("pool-main", "inst-a", chrono::Duration::seconds(300))
            .await
            .unwrap();
        let second = runtime
            .acquire_account_lease("pool-main", "inst-b", chrono::Duration::seconds(300))
            .await;

        assert_eq!(second.unwrap_err(), AccountLeaseError::NoEligibleAccount);
        assert_eq!(first.account_id, "acct-1");
    }

    #[tokio::test]
    async fn acquire_account_lease_excluding_skips_excluded_accounts() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;
        seed_account_in_pool(runtime.as_ref(), "acct-2", "pool-main", 1, true).await;

        let excluded_account_ids = vec!["acct-1".to_string()];
        let lease = runtime
            .acquire_account_lease_excluding(
                "pool-main",
                "inst-a",
                chrono::Duration::seconds(300),
                &excluded_account_ids,
            )
            .await
            .unwrap();

        assert_eq!(lease.account_id, "acct-2");
    }

    #[tokio::test]
    async fn acquire_account_lease_excluding_rejects_when_all_accounts_are_excluded() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;
        seed_account_in_pool(runtime.as_ref(), "acct-2", "pool-main", 1, true).await;

        let excluded_account_ids = vec!["acct-1".to_string(), "acct-2".to_string()];
        let lease = runtime
            .acquire_account_lease_excluding(
                "pool-main",
                "inst-a",
                chrono::Duration::seconds(300),
                &excluded_account_ids,
            )
            .await;

        assert_eq!(lease.unwrap_err(), AccountLeaseError::NoEligibleAccount);
    }

    #[tokio::test]
    async fn acquire_account_lease_returns_existing_active_lease_for_holder() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;
        seed_account(runtime.as_ref(), "acct-2").await;

        let first = runtime
            .acquire_account_lease("pool-main", "inst-a", chrono::Duration::seconds(300))
            .await
            .unwrap();
        let second = runtime
            .acquire_account_lease("pool-main", "inst-a", chrono::Duration::seconds(300))
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
    async fn read_active_holder_lease_returns_current_unexpired_lease() {
        let runtime = test_runtime().await;
        seed_account_in_pool(runtime.as_ref(), "acct-1", "team-main", 0, true).await;
        let lease = runtime
            .acquire_account_lease("team-main", "holder-1", chrono::Duration::seconds(300))
            .await
            .unwrap();

        let read_back = runtime.read_active_holder_lease("holder-1").await.unwrap();

        assert_eq!(read_back, Some(lease));
    }

    #[tokio::test]
    async fn read_pool_diagnostics_reports_per_account_eligibility_and_next_eligible_time() {
        let runtime = test_runtime().await;
        seed_account_in_pool(runtime.as_ref(), "acct-1", "team-main", 0, true).await;
        seed_account_in_pool(runtime.as_ref(), "acct-2", "team-main", 1, true).await;
        let lease = runtime
            .acquire_account_lease("team-main", "holder-1", chrono::Duration::seconds(300))
            .await
            .unwrap();
        runtime
            .record_account_health_event(AccountHealthEvent {
                account_id: "acct-2".to_string(),
                pool_id: "team-main".to_string(),
                health_state: AccountHealthState::RateLimited,
                sequence_number: 1,
                observed_at: test_timestamp(1),
            })
            .await
            .unwrap();

        let diagnostic = runtime
            .read_account_pool_diagnostic("team-main", None)
            .await
            .unwrap();

        assert_eq!(diagnostic.pool_id, "team-main");
        assert_eq!(diagnostic.next_eligible_at, Some(lease.expires_at));
        assert_eq!(diagnostic.accounts.len(), 2);
        assert_eq!(diagnostic.accounts[0].account_id, "acct-1");
        assert_eq!(diagnostic.accounts[0].pool_id, "team-main");
        assert_eq!(diagnostic.accounts[0].healthy, true);
        assert_eq!(diagnostic.accounts[0].active_lease, Some(lease.clone()));
        assert_eq!(diagnostic.accounts[0].health_state, None);
        assert_eq!(
            diagnostic.accounts[0].eligibility,
            crate::AccountStartupEligibility::PreferredAccountBusy
        );
        assert_eq!(
            diagnostic.accounts[0].next_eligible_at,
            Some(lease.expires_at)
        );
        assert_eq!(diagnostic.accounts[1].account_id, "acct-2");
        assert_eq!(diagnostic.accounts[1].pool_id, "team-main");
        assert_eq!(diagnostic.accounts[1].healthy, false);
        assert_eq!(diagnostic.accounts[1].active_lease, None);
        assert_eq!(
            diagnostic.accounts[1].health_state,
            Some(AccountHealthState::RateLimited)
        );
        assert_eq!(
            diagnostic.accounts[1].eligibility,
            crate::AccountStartupEligibility::PreferredAccountUnhealthy
        );
        assert_eq!(diagnostic.accounts[1].next_eligible_at, None);
    }

    #[tokio::test]
    async fn read_pool_diagnostics_keeps_preferred_busy_account_blocked() {
        let runtime = test_runtime().await;
        seed_account_in_pool(runtime.as_ref(), "acct-1", "team-main", 0, true).await;
        seed_account_in_pool(runtime.as_ref(), "acct-2", "team-main", 1, true).await;
        let lease = runtime
            .acquire_preferred_account_lease(
                "team-main",
                "acct-1",
                "holder-1",
                chrono::Duration::seconds(300),
            )
            .await
            .unwrap();

        let diagnostic = runtime
            .read_account_pool_diagnostic("team-main", Some("acct-1"))
            .await
            .unwrap();

        assert_eq!(diagnostic.pool_id, "team-main");
        assert_eq!(diagnostic.next_eligible_at, Some(lease.expires_at));
        assert_eq!(diagnostic.accounts.len(), 2);
        assert_eq!(
            diagnostic.accounts[0].eligibility,
            crate::AccountStartupEligibility::PreferredAccountBusy
        );
        assert_eq!(
            diagnostic.accounts[0].next_eligible_at,
            Some(lease.expires_at)
        );
        assert_eq!(
            diagnostic.accounts[1].eligibility,
            crate::AccountStartupEligibility::AutomaticAccountSelected
        );
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
    async fn init_does_not_relabel_pre_0028_manual_legacy_default_account_as_migrated() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let state_path = super::state_db_path(codex_home.as_path());
        let old_state_migrator = Migrator {
            migrations: Cow::Owned(
                STATE_MIGRATOR
                    .migrations
                    .iter()
                    .filter(|migration| {
                        migration.version < ACCOUNT_REGISTRY_SOURCE_MIGRATION_VERSION
                    })
                    .cloned()
                    .collect(),
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
            .expect("apply pre-0028 state schema");
        sqlx::query(
            r#"
INSERT INTO account_registry (
    account_id,
    pool_id,
    position,
    account_kind,
    backend_family,
    workspace_id,
    enabled,
    healthy,
    created_at,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind("acct-local")
        .bind("legacy-default")
        .bind(0_i64)
        .bind("chatgpt")
        .bind("chatgpt")
        .bind("workspace-main")
        .bind(1_i64)
        .bind(1_i64)
        .bind(1_i64)
        .bind(1_i64)
        .execute(&pool)
        .await
        .expect("insert pre-0028 manual legacy-default account");
        pool.close().await;

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");

        assert_eq!(
            runtime
                .read_account_pool_membership("acct-local")
                .await
                .expect("read account pool membership"),
            Some(AccountPoolMembership {
                account_id: "acct-local".to_string(),
                pool_id: "legacy-default".to_string(),
                source: None,
                enabled: true,
                healthy: true,
            })
        );

        drop(runtime);
        let _ = tokio::fs::remove_dir_all(codex_home).await;
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
                STATE_MIGRATOR
                    .migrations
                    .iter()
                    .filter(|migration| {
                        migration.version < super::ACTIVE_HOLDER_INDEX_MIGRATION_VERSION
                    })
                    .cloned()
                    .collect(),
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
    async fn init_accepts_databases_with_historical_modified_0026_already_applied() {
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
        historical_modified_0026_migrator()
            .run(&pool)
            .await
            .expect("apply historical modified 0026 state schema");
        pool.close().await;

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");
        let persisted_checksum: Vec<u8> = sqlx::query_scalar(
            r#"
SELECT checksum
FROM _sqlx_migrations
WHERE version = 26
            "#,
        )
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("load persisted checksum");

        assert_eq!(
            persisted_checksum,
            STATE_MIGRATOR
                .migrations
                .iter()
                .find(|migration| migration.version == 26)
                .expect("current 0026 migration")
                .checksum
                .as_ref()
        );

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
        let lease = runtime
            .acquire_account_lease("pool-main", "inst-a", chrono::Duration::seconds(300))
            .await;

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
            .acquire_account_lease("pool-main", "inst-a", chrono::Duration::seconds(300))
            .await
            .unwrap();
        let renewal = runtime
            .renew_account_lease(
                &lease.lease_key(),
                lease.expires_at + chrono::Duration::seconds(1),
                chrono::Duration::seconds(300),
            )
            .await
            .unwrap();

        assert_eq!(renewal, crate::LeaseRenewal::Missing);
    }

    #[tokio::test]
    async fn release_account_lease_allows_immediate_reacquisition() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;

        let first = runtime
            .acquire_account_lease("pool-main", "inst-a", chrono::Duration::seconds(30))
            .await
            .unwrap();
        runtime
            .release_account_lease(&first.lease_key(), test_timestamp(2))
            .await
            .unwrap();
        let second = runtime
            .acquire_account_lease("pool-main", "inst-b", chrono::Duration::seconds(30))
            .await
            .unwrap();

        assert_eq!(second.account_id, "acct-1");
        assert_ne!(second.lease_id, first.lease_id);
    }

    #[tokio::test]
    async fn read_account_health_event_sequence_returns_persisted_sequence() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;

        runtime
            .record_account_health_event(AccountHealthEvent {
                account_id: "acct-1".to_string(),
                pool_id: "pool-main".to_string(),
                health_state: AccountHealthState::RateLimited,
                sequence_number: 7,
                observed_at: test_timestamp(7),
            })
            .await
            .unwrap();

        assert_eq!(
            runtime
                .read_account_health_event_sequence("acct-1")
                .await
                .unwrap(),
            Some(7)
        );
    }

    #[tokio::test]
    async fn record_account_health_event_retries_when_database_is_locked() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;

        let mut lock_conn = runtime
            .pool
            .acquire()
            .await
            .expect("acquire lock connection");
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *lock_conn)
            .await
            .expect("begin immediate lock");

        let runtime_for_record = Arc::clone(&runtime);
        let record_task = tokio::spawn(async move {
            runtime_for_record
                .record_account_health_event(AccountHealthEvent {
                    account_id: "acct-1".to_string(),
                    pool_id: "pool-main".to_string(),
                    health_state: AccountHealthState::RateLimited,
                    sequence_number: 1,
                    observed_at: test_timestamp(1),
                })
                .await
        });

        tokio::time::sleep(std::time::Duration::from_secs(6)).await;
        sqlx::query("COMMIT")
            .execute(&mut *lock_conn)
            .await
            .expect("release immediate lock");

        record_task
            .await
            .expect("record task join")
            .expect("record health event should retry after lock release");

        assert_eq!(
            runtime
                .read_account_health_event_sequence("acct-1")
                .await
                .unwrap(),
            Some(1)
        );
    }

    #[tokio::test]
    async fn acquire_and_renew_account_lease_use_requested_ttl() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;

        let lease = runtime
            .acquire_account_lease("pool-main", "inst-a", chrono::Duration::seconds(30))
            .await
            .unwrap();
        let renew_at = lease.acquired_at + chrono::Duration::seconds(15);
        let renewal = runtime
            .renew_account_lease(&lease.lease_key(), renew_at, chrono::Duration::seconds(30))
            .await
            .unwrap();

        let LeaseRenewal::Renewed(renewed) = renewal else {
            panic!("expected renewed lease");
        };

        assert_eq!(
            lease.expires_at - lease.acquired_at,
            chrono::Duration::seconds(30)
        );
        assert_eq!(renewed.expires_at - renew_at, chrono::Duration::seconds(30));
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
            runtime_a.acquire_account_lease("pool-main", "inst-a", chrono::Duration::seconds(300)),
            runtime_b.acquire_account_lease("pool-main", "inst-b", chrono::Duration::seconds(300))
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
    async fn newer_health_event_keeps_registry_and_runtime_pool_ids_in_sync() {
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

        let health = runtime
            .read_account_health_state("acct-1")
            .await
            .unwrap()
            .expect("persisted health state");
        let registry_pool_id: String = sqlx::query_scalar(
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

        assert_eq!(health.pool_id, "pool-new");
        assert_eq!(registry_pool_id, "pool-new");
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
        let lease = runtime
            .acquire_account_lease("pool-main", "inst-a", chrono::Duration::seconds(300))
            .await;

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
    async fn import_legacy_default_account_fills_existing_empty_startup_selection_row() {
        let runtime = test_runtime().await;

        runtime
            .write_account_startup_selection(crate::AccountStartupSelectionUpdate::default())
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
                default_pool_id: Some("legacy-default".to_string()),
                preferred_account_id: Some("acct-legacy".to_string()),
                suppressed: false,
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
        assert_eq!(
            runtime
                .read_account_pool_membership("acct-1")
                .await
                .unwrap()
                .and_then(|membership| membership.source),
            Some(AccountSource::Migrated)
        );
    }

    #[tokio::test]
    async fn import_legacy_default_account_preserves_ambiguous_legacy_default_membership_source() {
        let runtime = test_runtime().await;
        seed_account_in_pool(
            runtime.as_ref(),
            "acct-local",
            super::LEGACY_DEFAULT_POOL_ID,
            0,
            true,
        )
        .await;

        runtime
            .import_legacy_default_account(LegacyAccountImport {
                account_id: "acct-local".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(
            runtime
                .read_account_pool_membership("acct-local")
                .await
                .unwrap(),
            Some(AccountPoolMembership {
                account_id: "acct-local".to_string(),
                pool_id: super::LEGACY_DEFAULT_POOL_ID.to_string(),
                source: None,
                enabled: true,
                healthy: true,
            })
        );
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

    #[tokio::test]
    async fn migrated_account_source_survives_reassignment() {
        let runtime = test_runtime().await;

        runtime
            .import_legacy_default_account(LegacyAccountImport {
                account_id: "acct-legacy".to_string(),
            })
            .await
            .unwrap();
        runtime
            .assign_account_pool("acct-legacy", "pool-user")
            .await
            .unwrap();

        assert_eq!(
            runtime
                .read_account_pool_membership("acct-legacy")
                .await
                .unwrap(),
            Some(AccountPoolMembership {
                account_id: "acct-legacy".to_string(),
                pool_id: "pool-user".to_string(),
                source: Some(AccountSource::Migrated),
                enabled: true,
                healthy: true,
            })
        );
    }

    #[tokio::test]
    async fn assigning_local_account_to_legacy_default_pool_does_not_mark_it_migrated() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-local").await;

        runtime
            .assign_account_pool("acct-local", "legacy-default")
            .await
            .unwrap();

        assert_eq!(
            runtime
                .read_account_pool_membership("acct-local")
                .await
                .unwrap(),
            Some(AccountPoolMembership {
                account_id: "acct-local".to_string(),
                pool_id: "legacy-default".to_string(),
                source: None,
                enabled: true,
                healthy: true,
            })
        );
    }

    #[tokio::test]
    async fn preview_startup_selection_reports_suppressed_runtime_selection() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;
        runtime
            .write_account_startup_selection(crate::AccountStartupSelectionUpdate {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-1".to_string()),
                suppressed: true,
            })
            .await
            .unwrap();

        let preview = runtime
            .preview_account_startup_selection(Some("pool-main"))
            .await
            .unwrap();

        assert_eq!(
            preview,
            crate::AccountStartupSelectionPreview {
                effective_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-1".to_string()),
                suppressed: true,
                predicted_account_id: None,
                eligibility: crate::AccountStartupEligibility::Suppressed,
            }
        );
    }

    #[tokio::test]
    async fn preview_startup_selection_reports_preferred_account_in_wrong_pool() {
        let runtime = test_runtime().await;
        seed_account_in_pool(runtime.as_ref(), "acct-main", "pool-main", 0, true).await;
        seed_account_in_pool(runtime.as_ref(), "acct-other", "pool-other", 0, true).await;
        runtime
            .write_account_startup_selection(crate::AccountStartupSelectionUpdate {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-other".to_string()),
                suppressed: false,
            })
            .await
            .unwrap();

        let preview = runtime
            .preview_account_startup_selection(Some("pool-main"))
            .await
            .unwrap();

        assert_eq!(
            preview,
            crate::AccountStartupSelectionPreview {
                effective_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-other".to_string()),
                suppressed: false,
                predicted_account_id: None,
                eligibility: crate::AccountStartupEligibility::PreferredAccountInOtherPool {
                    actual_pool_id: "pool-other".to_string(),
                },
            }
        );
    }

    #[tokio::test]
    async fn preview_startup_selection_reports_disabled_preferred_account() {
        let runtime = test_runtime().await;
        seed_account_in_pool(runtime.as_ref(), "acct-1", "pool-main", 0, true).await;
        runtime
            .write_account_startup_selection(crate::AccountStartupSelectionUpdate {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-1".to_string()),
                suppressed: false,
            })
            .await
            .unwrap();
        runtime.set_account_enabled("acct-1", false).await.unwrap();

        let preview = runtime
            .preview_account_startup_selection(Some("pool-main"))
            .await
            .unwrap();

        assert_eq!(
            preview,
            crate::AccountStartupSelectionPreview {
                effective_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-1".to_string()),
                suppressed: false,
                predicted_account_id: None,
                eligibility: crate::AccountStartupEligibility::PreferredAccountDisabled,
            }
        );
    }

    #[tokio::test]
    async fn preview_startup_selection_skips_disabled_accounts_for_automatic_selection() {
        let runtime = test_runtime().await;
        seed_account_in_pool(runtime.as_ref(), "acct-1", "pool-main", 0, true).await;
        seed_account_in_pool(runtime.as_ref(), "acct-2", "pool-main", 1, true).await;
        runtime.set_account_enabled("acct-1", false).await.unwrap();

        let preview = runtime
            .preview_account_startup_selection(Some("pool-main"))
            .await
            .unwrap();

        assert_eq!(
            preview,
            crate::AccountStartupSelectionPreview {
                effective_pool_id: Some("pool-main".to_string()),
                preferred_account_id: None,
                suppressed: false,
                predicted_account_id: Some("acct-2".to_string()),
                eligibility: crate::AccountStartupEligibility::AutomaticAccountSelected,
            }
        );
    }

    #[tokio::test]
    async fn disabling_account_releases_active_lease_and_prevents_renewal() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;

        let lease = runtime
            .acquire_account_lease("pool-main", "inst-a", chrono::Duration::seconds(300))
            .await
            .unwrap();

        runtime.set_account_enabled("acct-1", false).await.unwrap();

        let renewal = runtime
            .renew_account_lease(
                &lease.lease_key(),
                lease.acquired_at + chrono::Duration::seconds(30),
                chrono::Duration::seconds(300),
            )
            .await
            .unwrap();

        assert_eq!(renewal, LeaseRenewal::Missing);
        assert_eq!(
            runtime.read_active_holder_lease("inst-a").await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn assigning_account_pool_releases_active_lease_and_keeps_runtime_state_in_sync() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;
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
        let lease = runtime
            .acquire_account_lease("pool-main", "inst-a", chrono::Duration::seconds(300))
            .await
            .unwrap();

        runtime
            .assign_account_pool("acct-1", "pool-other")
            .await
            .unwrap();

        let renewal = runtime
            .renew_account_lease(
                &lease.lease_key(),
                lease.acquired_at + chrono::Duration::seconds(30),
                chrono::Duration::seconds(300),
            )
            .await
            .unwrap();
        let reassigned = runtime
            .acquire_account_lease("pool-other", "inst-b", chrono::Duration::seconds(300))
            .await
            .unwrap();
        let health = runtime
            .read_account_health_state("acct-1")
            .await
            .unwrap()
            .expect("persisted health state");

        assert_eq!(renewal, LeaseRenewal::Missing);
        assert_eq!(
            runtime.read_active_holder_lease("inst-a").await.unwrap(),
            None
        );
        assert_eq!(reassigned.account_id, "acct-1");
        assert_eq!(reassigned.pool_id, "pool-other");
        assert_eq!(health.pool_id, "pool-other");
    }

    #[tokio::test]
    async fn upsert_account_registry_entry_updates_runtime_pool_and_invalidates_active_lease() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;
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
        let lease = runtime
            .acquire_account_lease("pool-main", "inst-a", chrono::Duration::seconds(300))
            .await
            .unwrap();

        runtime
            .upsert_account_registry_entry(AccountRegistryEntryUpdate {
                account_id: "acct-1".to_string(),
                pool_id: "pool-other".to_string(),
                position: 0,
                account_kind: "chatgpt".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some("workspace-main".to_string()),
                enabled: false,
                healthy: false,
            })
            .await
            .unwrap();

        let renewal = runtime
            .renew_account_lease(
                &lease.lease_key(),
                lease.acquired_at + chrono::Duration::seconds(30),
                chrono::Duration::seconds(300),
            )
            .await
            .unwrap();

        assert_eq!(renewal, LeaseRenewal::Missing);
        assert_eq!(
            runtime.read_account_health_state("acct-1").await.unwrap(),
            None
        );
        assert_eq!(
            runtime.read_active_holder_lease("inst-a").await.unwrap(),
            None
        );
        assert_eq!(
            runtime
                .read_account_pool_membership("acct-1")
                .await
                .unwrap(),
            Some(AccountPoolMembership {
                account_id: "acct-1".to_string(),
                pool_id: "pool-other".to_string(),
                source: None,
                enabled: false,
                healthy: false,
            })
        );
    }

    #[tokio::test]
    async fn upsert_account_registry_entry_changing_healthy_clears_stale_runtime_health() {
        let runtime = test_runtime().await;
        seed_account(runtime.as_ref(), "acct-1").await;
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
        runtime
            .write_account_startup_selection(crate::AccountStartupSelectionUpdate {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-1".to_string()),
                suppressed: false,
            })
            .await
            .unwrap();

        runtime
            .upsert_account_registry_entry(AccountRegistryEntryUpdate {
                account_id: "acct-1".to_string(),
                pool_id: "pool-main".to_string(),
                position: 0,
                account_kind: "chatgpt".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some("workspace-main".to_string()),
                enabled: true,
                healthy: false,
            })
            .await
            .unwrap();

        let diagnostic = runtime
            .read_account_pool_diagnostic("pool-main", Some("acct-1"))
            .await
            .unwrap();
        let preview = runtime
            .preview_account_startup_selection(Some("pool-main"))
            .await
            .unwrap();

        assert_eq!(
            runtime.read_account_health_state("acct-1").await.unwrap(),
            None
        );
        assert_eq!(
            diagnostic.accounts,
            vec![AccountPoolAccountDiagnostic {
                account_id: "acct-1".to_string(),
                pool_id: "pool-main".to_string(),
                source: None,
                enabled: true,
                healthy: false,
                active_lease: None,
                health_state: None,
                eligibility: AccountStartupEligibility::PreferredAccountUnhealthy,
                next_eligible_at: None,
            }]
        );
        assert_eq!(
            preview,
            AccountStartupSelectionPreview {
                effective_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-1".to_string()),
                suppressed: false,
                predicted_account_id: None,
                eligibility: AccountStartupEligibility::PreferredAccountUnhealthy,
            }
        );
    }

    #[tokio::test]
    async fn list_memberships_reflects_assignment_and_enabled_state() {
        let runtime = test_runtime().await;
        seed_account_in_pool(runtime.as_ref(), "acct-1", "pool-main", 0, true).await;
        seed_account_in_pool(runtime.as_ref(), "acct-2", "pool-main", 1, true).await;

        runtime
            .assign_account_pool("acct-2", "pool-other")
            .await
            .unwrap();
        runtime.set_account_enabled("acct-1", false).await.unwrap();

        assert_eq!(
            runtime
                .list_account_pool_memberships(Some("pool-main"))
                .await
                .unwrap(),
            vec![crate::AccountPoolMembership {
                account_id: "acct-1".to_string(),
                pool_id: "pool-main".to_string(),
                source: None,
                healthy: true,
                enabled: false,
            }]
        );
        assert_eq!(
            runtime
                .list_account_pool_memberships(Some("pool-other"))
                .await
                .unwrap(),
            vec![crate::AccountPoolMembership {
                account_id: "acct-2".to_string(),
                pool_id: "pool-other".to_string(),
                source: None,
                healthy: true,
                enabled: true,
            }]
        );
    }

    #[tokio::test]
    async fn membership_backfill_reads_new_membership_table_and_keeps_compat_columns_synced() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let state_path = super::state_db_path(codex_home.as_path());
        let old_state_migrator = Migrator {
            migrations: Cow::Owned(
                STATE_MIGRATOR
                    .migrations
                    .iter()
                    .filter(|migration| migration.version < 29)
                    .cloned()
                    .collect(),
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
        .expect("open pre-0029 state db");
        old_state_migrator
            .run(&pool)
            .await
            .expect("apply pre-0029 state schema");
        sqlx::query(
            r#"
INSERT INTO account_registry (
    account_id,
    pool_id,
    position,
    account_kind,
    backend_family,
    workspace_id,
    enabled,
    healthy,
    created_at,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind("acct-1")
        .bind("team-main")
        .bind(0_i64)
        .bind("chatgpt")
        .bind("chatgpt")
        .bind("workspace-main")
        .bind(1_i64)
        .bind(1_i64)
        .bind(1_i64)
        .bind(1_i64)
        .execute(&pool)
        .await
        .expect("insert pre-0029 registry row");
        pool.close().await;

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initialize runtime");
        let membership = runtime
            .read_account_pool_membership("acct-1")
            .await
            .expect("read account pool membership")
            .expect("membership");
        let compat_columns: (String, i64) = sqlx::query_as(
            r#"
SELECT pool_id, position
FROM account_registry
WHERE account_id = ?
            "#,
        )
        .bind("acct-1")
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("read compat columns");

        assert_eq!(
            membership,
            AccountPoolMembership {
                account_id: "acct-1".to_string(),
                pool_id: "team-main".to_string(),
                source: None,
                enabled: true,
                healthy: true,
            }
        );
        assert_eq!(compat_columns, ("team-main".to_string(), 0));

        runtime
            .assign_account_pool_membership("acct-1", "team-other")
            .await
            .expect("reassign membership");

        let updated_membership = runtime
            .read_account_pool_membership("acct-1")
            .await
            .expect("read updated membership");
        let updated_compat_columns: (String, i64) = sqlx::query_as(
            r#"
SELECT pool_id, position
FROM account_registry
WHERE account_id = ?
            "#,
        )
        .bind("acct-1")
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("read updated compat columns");

        assert_eq!(
            updated_membership,
            Some(AccountPoolMembership {
                account_id: "acct-1".to_string(),
                pool_id: "team-other".to_string(),
                source: None,
                enabled: true,
                healthy: true,
            })
        );
        assert_eq!(updated_compat_columns, ("team-other".to_string(), 0));
    }

    #[tokio::test]
    async fn pending_registration_round_trip_is_keyed_by_idempotency_key() {
        let runtime = test_runtime().await;

        runtime
            .create_pending_account_registration(NewPendingAccountRegistration {
                idempotency_key: "idem-1".to_string(),
                backend_id: "local".to_string(),
                provider_kind: "chatgpt".to_string(),
                target_pool_id: Some("team-main".to_string()),
                backend_account_handle: None,
                account_id: None,
            })
            .await
            .expect("create pending registration");

        assert_eq!(
            runtime
                .read_pending_account_registration("idem-1")
                .await
                .expect("read pending registration"),
            Some(crate::PendingAccountRegistration {
                idempotency_key: "idem-1".to_string(),
                backend_id: "local".to_string(),
                provider_kind: "chatgpt".to_string(),
                target_pool_id: Some("team-main".to_string()),
                backend_account_handle: None,
                account_id: None,
                completed_at: None,
            })
        );
    }

    #[tokio::test]
    async fn finalized_pending_registration_is_not_listed_as_active_and_remains_reconcilable() {
        let runtime = test_runtime().await;

        runtime
            .create_pending_account_registration(NewPendingAccountRegistration {
                idempotency_key: "idem-1".to_string(),
                backend_id: "local".to_string(),
                provider_kind: "chatgpt".to_string(),
                target_pool_id: Some("team-main".to_string()),
                backend_account_handle: None,
                account_id: None,
            })
            .await
            .expect("create pending registration");
        runtime
            .finalize_pending_account_registration("idem-1", "handle-1", "acct-1")
            .await
            .expect("finalize pending registration");
        runtime
            .create_pending_account_registration(NewPendingAccountRegistration {
                idempotency_key: "idem-1".to_string(),
                backend_id: "local".to_string(),
                provider_kind: "chatgpt".to_string(),
                target_pool_id: Some("team-other".to_string()),
                backend_account_handle: None,
                account_id: None,
            })
            .await
            .expect("do not reopen finalized pending registration");

        assert_eq!(
            runtime
                .list_pending_account_registrations()
                .await
                .expect("list active pending registrations"),
            Vec::<crate::PendingAccountRegistration>::new()
        );
        let finalized = runtime
            .read_pending_account_registration("idem-1")
            .await
            .expect("read finalized pending registration")
            .expect("finalized pending registration");
        let finalized_again = runtime
            .finalize_pending_account_registration("idem-1", "handle-2", "acct-2")
            .await
            .expect("do not rewrite finalized pending registration");

        assert_eq!(finalized.idempotency_key, "idem-1");
        assert_eq!(finalized.backend_id, "local");
        assert_eq!(finalized.provider_kind, "chatgpt");
        assert_eq!(finalized.target_pool_id.as_deref(), Some("team-main"));
        assert_eq!(
            finalized.backend_account_handle.as_deref(),
            Some("handle-1")
        );
        assert_eq!(finalized.account_id.as_deref(), Some("acct-1"));
        assert!(finalized.completed_at.is_some());
        assert!(!finalized_again);
    }

    #[tokio::test]
    async fn upgraded_registration_rows_backfill_required_identifiers() {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let state_path = super::state_db_path(codex_home.as_path());
        let old_state_migrator = Migrator {
            migrations: Cow::Owned(
                STATE_MIGRATOR
                    .migrations
                    .iter()
                    .filter(|migration| migration.version < 29)
                    .cloned()
                    .collect(),
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
        .expect("open pre-0029 state db");
        old_state_migrator
            .run(&pool)
            .await
            .expect("apply pre-0029 state schema");
        sqlx::query(
            r#"
INSERT INTO account_registry (
    account_id,
    pool_id,
    position,
    account_kind,
    backend_family,
    workspace_id,
    enabled,
    healthy,
    created_at,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind("acct-1")
        .bind("team-main")
        .bind(0_i64)
        .bind("chatgpt")
        .bind("chatgpt")
        .bind("workspace-main")
        .bind(1_i64)
        .bind(1_i64)
        .bind(1_i64)
        .bind(1_i64)
        .execute(&pool)
        .await
        .expect("insert pre-0029 registry row");
        pool.close().await;

        let runtime = StateRuntime::init(codex_home, "test-provider".to_string())
            .await
            .expect("initialize runtime");
        let committed_identifiers: (String, String, String, String, Option<String>) =
            sqlx::query_as(
                r#"
SELECT backend_id, backend_account_handle, provider_fingerprint, backend_family, workspace_id
FROM account_registry
WHERE account_id = ?
            "#,
            )
            .bind("acct-1")
            .fetch_one(runtime.pool.as_ref())
            .await
            .expect("read committed identifiers");

        assert_eq!(committed_identifiers.0, "local".to_string());
        assert_eq!(committed_identifiers.1, "acct-1".to_string());
        assert_eq!(
            committed_identifiers.2,
            "legacy:chatgpt:workspace-main:acct-1".to_string()
        );
        assert_eq!(committed_identifiers.3, "chatgpt".to_string());
        assert_eq!(committed_identifiers.4, Some("workspace-main".to_string()));
    }

    #[tokio::test]
    async fn upsert_registered_account_persists_backend_family_and_required_identifiers() {
        let runtime = test_runtime().await;

        runtime
            .upsert_registered_account(RegisteredAccountUpsert {
                account_id: "acct-1".to_string(),
                backend_id: "local".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some("workspace-main".to_string()),
                backend_account_handle: "handle-1".to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: "fingerprint-1".to_string(),
                display_name: Some("Primary".to_string()),
                source: None,
                enabled: true,
                healthy: true,
                membership: Some(RegisteredAccountMembership {
                    pool_id: "team-main".to_string(),
                    position: 0,
                }),
            })
            .await
            .expect("upsert registered account");

        assert_eq!(
            runtime
                .read_registered_account("acct-1")
                .await
                .expect("read registered account"),
            Some(RegisteredAccountRecord {
                account_id: "acct-1".to_string(),
                backend_id: "local".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some("workspace-main".to_string()),
                backend_account_handle: "handle-1".to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: "fingerprint-1".to_string(),
                display_name: Some("Primary".to_string()),
                source: None,
                enabled: true,
                healthy: true,
            })
        );

        runtime
            .upsert_registered_account(RegisteredAccountUpsert {
                account_id: "acct-1".to_string(),
                backend_id: "remote".to_string(),
                backend_family: "responses".to_string(),
                workspace_id: None,
                backend_account_handle: "handle-2".to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: "fingerprint-2".to_string(),
                display_name: Some("Primary".to_string()),
                source: None,
                enabled: true,
                healthy: true,
                membership: Some(RegisteredAccountMembership {
                    pool_id: "team-main".to_string(),
                    position: 0,
                }),
            })
            .await
            .expect("update registered account");

        assert_eq!(
            runtime
                .read_registered_account("acct-1")
                .await
                .expect("read updated registered account"),
            Some(RegisteredAccountRecord {
                account_id: "acct-1".to_string(),
                backend_id: "remote".to_string(),
                backend_family: "responses".to_string(),
                workspace_id: Some("workspace-main".to_string()),
                backend_account_handle: "handle-2".to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: "fingerprint-2".to_string(),
                display_name: Some("Primary".to_string()),
                source: None,
                enabled: true,
                healthy: true,
            })
        );
    }

    #[tokio::test]
    async fn upsert_registered_account_allows_unassigned_catalog_entries() {
        let runtime = test_runtime().await;

        runtime
            .upsert_registered_account(RegisteredAccountUpsert {
                account_id: "acct-unassigned".to_string(),
                backend_id: "local".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some("workspace-main".to_string()),
                backend_account_handle: "handle-unassigned".to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: "fingerprint-unassigned".to_string(),
                display_name: Some("Holding".to_string()),
                source: None,
                enabled: true,
                healthy: true,
                membership: None,
            })
            .await
            .expect("upsert unassigned registered account");

        assert_eq!(
            runtime
                .read_registered_account("acct-unassigned")
                .await
                .expect("read unassigned registered account"),
            Some(RegisteredAccountRecord {
                account_id: "acct-unassigned".to_string(),
                backend_id: "local".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some("workspace-main".to_string()),
                backend_account_handle: "handle-unassigned".to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: "fingerprint-unassigned".to_string(),
                display_name: Some("Holding".to_string()),
                source: None,
                enabled: true,
                healthy: true,
            })
        );
        assert_eq!(
            runtime
                .read_account_pool_membership("acct-unassigned")
                .await
                .expect("read membership"),
            None
        );
        assert_eq!(
            runtime
                .list_account_pool_memberships(None)
                .await
                .expect("list memberships"),
            Vec::<AccountPoolMembership>::new()
        );
        let compat_columns: (String, i64) = sqlx::query_as(
            r#"
SELECT pool_id, position
FROM account_registry
WHERE account_id = ?
            "#,
        )
        .bind("acct-unassigned")
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("read compat columns");

        assert_eq!(compat_columns, ("".to_string(), 0));
    }

    #[tokio::test]
    async fn health_events_do_not_reassign_unassigned_registered_accounts() {
        let runtime = test_runtime().await;

        runtime
            .upsert_registered_account(RegisteredAccountUpsert {
                account_id: "acct-unassigned".to_string(),
                backend_id: "local".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some("workspace-main".to_string()),
                backend_account_handle: "handle-unassigned".to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: "fingerprint-unassigned".to_string(),
                display_name: Some("Holding".to_string()),
                source: None,
                enabled: true,
                healthy: true,
                membership: None,
            })
            .await
            .expect("upsert unassigned registered account");
        runtime
            .record_account_health_event(AccountHealthEvent {
                account_id: "acct-unassigned".to_string(),
                pool_id: "team-main".to_string(),
                sequence_number: 1,
                health_state: AccountHealthState::RateLimited,
                observed_at: test_timestamp(1),
            })
            .await
            .expect("record health event");

        assert_eq!(
            runtime
                .read_account_pool_membership("acct-unassigned")
                .await
                .expect("read membership"),
            None
        );
        let compat_columns: (String, i64, bool) = sqlx::query_as(
            r#"
SELECT pool_id, position, healthy
FROM account_registry
WHERE account_id = ?
            "#,
        )
        .bind("acct-unassigned")
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("read compat columns");

        assert_eq!(compat_columns, ("".to_string(), 0, false));
    }

    #[tokio::test]
    async fn registered_account_rows_reject_null_required_identifiers() {
        let runtime = test_runtime().await;

        let result = sqlx::query(
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
    enabled,
    healthy,
    created_at,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind("acct-1")
        .bind("team-main")
        .bind(0_i64)
        .bind("chatgpt")
        .bind("chatgpt")
        .bind(Option::<String>::None)
        .bind("local")
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .bind(1_i64)
        .bind(1_i64)
        .bind(1_i64)
        .bind(1_i64)
        .execute(runtime.pool.as_ref())
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn upsert_registered_account_reuses_existing_backend_identity() {
        let runtime = test_runtime().await;

        let initial_account_id = runtime
            .upsert_registered_account(RegisteredAccountUpsert {
                account_id: "acct-1".to_string(),
                backend_id: "local".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some("workspace-main".to_string()),
                backend_account_handle: "handle-1".to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: "fingerprint-1".to_string(),
                display_name: Some("Primary".to_string()),
                source: None,
                enabled: true,
                healthy: true,
                membership: Some(RegisteredAccountMembership {
                    pool_id: "team-main".to_string(),
                    position: 0,
                }),
            })
            .await
            .expect("create canonical registered account");
        let replay_account_id = runtime
            .upsert_registered_account(RegisteredAccountUpsert {
                account_id: "acct-2".to_string(),
                backend_id: "local".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some("workspace-main".to_string()),
                backend_account_handle: "handle-1".to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: "fingerprint-1".to_string(),
                display_name: Some("Primary Replay".to_string()),
                source: None,
                enabled: true,
                healthy: true,
                membership: Some(RegisteredAccountMembership {
                    pool_id: "team-main".to_string(),
                    position: 1,
                }),
            })
            .await
            .expect("reuse canonical registered account");

        assert_eq!(initial_account_id, "acct-1".to_string());
        assert_eq!(replay_account_id, "acct-1".to_string());
        assert_eq!(
            runtime
                .read_registered_account("acct-2")
                .await
                .expect("read replay alias account"),
            None
        );
        assert_eq!(
            runtime
                .read_registered_account("acct-1")
                .await
                .expect("read canonical registered account"),
            Some(RegisteredAccountRecord {
                account_id: "acct-1".to_string(),
                backend_id: "local".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some("workspace-main".to_string()),
                backend_account_handle: "handle-1".to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: "fingerprint-1".to_string(),
                display_name: Some("Primary Replay".to_string()),
                source: None,
                enabled: true,
                healthy: true,
            })
        );
        assert_eq!(
            runtime
                .read_account_pool_membership("acct-1")
                .await
                .expect("read canonical membership"),
            Some(AccountPoolMembership {
                account_id: "acct-1".to_string(),
                pool_id: "team-main".to_string(),
                source: None,
                enabled: true,
                healthy: true,
            })
        );
        let account_count: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM account_registry
            "#,
        )
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("count registered accounts");
        assert_eq!(account_count, 1);
    }

    #[tokio::test]
    async fn upsert_registered_account_preserves_existing_source_when_replayed_without_one() {
        let runtime = test_runtime().await;

        runtime
            .upsert_registered_account(RegisteredAccountUpsert {
                account_id: "acct-1".to_string(),
                backend_id: "local".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some("workspace-main".to_string()),
                backend_account_handle: "handle-1".to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: "fingerprint-1".to_string(),
                display_name: Some("Primary".to_string()),
                source: Some(AccountSource::Migrated),
                enabled: true,
                healthy: true,
                membership: Some(RegisteredAccountMembership {
                    pool_id: "team-main".to_string(),
                    position: 0,
                }),
            })
            .await
            .expect("create migrated registered account");
        runtime
            .upsert_registered_account(RegisteredAccountUpsert {
                account_id: "acct-2".to_string(),
                backend_id: "local".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some("workspace-main".to_string()),
                backend_account_handle: "handle-1".to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: "fingerprint-1".to_string(),
                display_name: Some("Primary Replay".to_string()),
                source: None,
                enabled: true,
                healthy: true,
                membership: Some(RegisteredAccountMembership {
                    pool_id: "team-main".to_string(),
                    position: 0,
                }),
            })
            .await
            .expect("replay registered account without source");

        assert_eq!(
            runtime
                .read_registered_account("acct-1")
                .await
                .expect("read canonical registered account"),
            Some(RegisteredAccountRecord {
                account_id: "acct-1".to_string(),
                backend_id: "local".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some("workspace-main".to_string()),
                backend_account_handle: "handle-1".to_string(),
                account_kind: "chatgpt".to_string(),
                provider_fingerprint: "fingerprint-1".to_string(),
                display_name: Some("Primary Replay".to_string()),
                source: Some(AccountSource::Migrated),
                enabled: true,
                healthy: true,
            })
        );
    }

    #[tokio::test]
    async fn upsert_account_registry_entry_writes_and_updates_membership() {
        let runtime = test_runtime().await;

        runtime
            .upsert_account_registry_entry(AccountRegistryEntryUpdate {
                account_id: "acct-1".to_string(),
                pool_id: "pool-main".to_string(),
                position: 0,
                account_kind: "chatgpt".to_string(),
                backend_family: "chatgpt".to_string(),
                workspace_id: Some("workspace-main".to_string()),
                enabled: true,
                healthy: true,
            })
            .await
            .unwrap();

        assert_eq!(
            runtime
                .read_account_pool_membership("acct-1")
                .await
                .unwrap(),
            Some(AccountPoolMembership {
                account_id: "acct-1".to_string(),
                pool_id: "pool-main".to_string(),
                source: None,
                enabled: true,
                healthy: true,
            })
        );

        runtime
            .upsert_account_registry_entry(AccountRegistryEntryUpdate {
                account_id: "acct-1".to_string(),
                pool_id: "pool-other".to_string(),
                position: 2,
                account_kind: "api_key".to_string(),
                backend_family: "responses".to_string(),
                workspace_id: None,
                enabled: false,
                healthy: false,
            })
            .await
            .unwrap();

        assert_eq!(
            runtime
                .list_account_pool_memberships(Some("pool-main"))
                .await
                .unwrap(),
            Vec::<AccountPoolMembership>::new()
        );
        assert_eq!(
            runtime
                .list_account_pool_memberships(Some("pool-other"))
                .await
                .unwrap(),
            vec![AccountPoolMembership {
                account_id: "acct-1".to_string(),
                pool_id: "pool-other".to_string(),
                source: None,
                enabled: false,
                healthy: false,
            }]
        );
    }

    #[tokio::test]
    async fn removing_account_registry_entry_revokes_active_lease_and_clears_runtime_health() {
        let runtime = test_runtime().await;
        seed_account_in_pool(runtime.as_ref(), "acct-1", "pool-main", 0, true).await;
        seed_account_in_pool(runtime.as_ref(), "acct-2", "pool-main", 1, true).await;
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
        let lease = runtime
            .acquire_account_lease("pool-main", "inst-a", chrono::Duration::seconds(300))
            .await
            .unwrap();

        assert_eq!(
            runtime
                .remove_account_registry_entry("acct-1")
                .await
                .unwrap(),
            true
        );
        assert_eq!(
            runtime
                .renew_account_lease(
                    &lease.lease_key(),
                    lease.acquired_at + chrono::Duration::seconds(30),
                    chrono::Duration::seconds(300),
                )
                .await
                .unwrap(),
            LeaseRenewal::Missing
        );
        assert_eq!(
            runtime.read_account_health_state("acct-1").await.unwrap(),
            None
        );
        assert_eq!(
            runtime.read_active_holder_lease("inst-a").await.unwrap(),
            None
        );

        assert_eq!(
            runtime
                .list_account_pool_memberships(Some("pool-main"))
                .await
                .unwrap(),
            vec![crate::AccountPoolMembership {
                account_id: "acct-2".to_string(),
                pool_id: "pool-main".to_string(),
                source: None,
                healthy: true,
                enabled: true,
            }]
        );
        assert_eq!(
            runtime
                .read_account_pool_membership("acct-1")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn removing_preferred_account_clears_startup_override_and_falls_back() {
        let runtime = test_runtime().await;
        seed_account_in_pool(runtime.as_ref(), "acct-1", "pool-main", 0, true).await;
        seed_account_in_pool(runtime.as_ref(), "acct-2", "pool-main", 1, true).await;
        runtime
            .write_account_startup_selection(crate::AccountStartupSelectionUpdate {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: Some("acct-1".to_string()),
                suppressed: false,
            })
            .await
            .unwrap();

        assert_eq!(
            runtime
                .remove_account_registry_entry("acct-1")
                .await
                .unwrap(),
            true
        );
        assert_eq!(
            runtime.read_account_startup_selection().await.unwrap(),
            crate::AccountStartupSelectionState {
                default_pool_id: Some("pool-main".to_string()),
                preferred_account_id: None,
                suppressed: false,
            }
        );
        assert_eq!(
            runtime
                .preview_account_startup_selection(None)
                .await
                .unwrap(),
            crate::AccountStartupSelectionPreview {
                effective_pool_id: Some("pool-main".to_string()),
                preferred_account_id: None,
                suppressed: false,
                predicted_account_id: Some("acct-2".to_string()),
                eligibility: crate::AccountStartupEligibility::AutomaticAccountSelected,
            }
        );
    }

    #[tokio::test]
    async fn acquire_preferred_account_lease_claims_requested_account() {
        let runtime = test_runtime().await;
        seed_account_in_pool(runtime.as_ref(), "acct-1", "pool-main", 1, true).await;
        seed_account_in_pool(runtime.as_ref(), "acct-2", "pool-main", 0, true).await;

        let lease = runtime
            .acquire_preferred_account_lease(
                "pool-main",
                "acct-1",
                "inst-a",
                chrono::Duration::seconds(300),
            )
            .await
            .unwrap();

        assert_eq!(lease.account_id, "acct-1");
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
        seed_account_in_pool(runtime, account_id, "pool-main", 0, true).await;
    }

    async fn seed_account_in_pool(
        runtime: &StateRuntime,
        account_id: &str,
        pool_id: &str,
        position: i64,
        healthy: bool,
    ) {
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
    enabled,
    healthy,
    created_at,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(account_id)
        .bind(pool_id)
        .bind(position)
        .bind("chatgpt")
        .bind("chatgpt")
        .bind("workspace-main")
        .bind("local")
        .bind(super::legacy_backend_account_handle(account_id))
        .bind(super::legacy_provider_fingerprint(
            "chatgpt",
            Some("workspace-main"),
            account_id,
        ))
        .bind(1_i64)
        .bind(i64::from(healthy))
        .bind(1_i64)
        .bind(1_i64)
        .execute(runtime.pool.as_ref())
        .await
        .expect("seed account");
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
        .execute(runtime.pool.as_ref())
        .await
        .expect("seed account membership");
    }

    fn test_timestamp(seconds: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(seconds, 0).expect("timestamp")
    }
}
