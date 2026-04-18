use super::*;
use crate::AccountHealthState;
use crate::AccountQuotaStateRecord;
use crate::QuotaExhaustedWindows;
use crate::QuotaProbeResult;
use crate::model::account_datetime_to_epoch_nanos;
use crate::model::account_datetime_to_epoch_seconds;
use crate::model::account_epoch_nanos_to_datetime;
use crate::model::account_epoch_seconds_to_datetime;
use sqlx::Executor;
use sqlx::Row;
use sqlx::Sqlite;

pub(super) const COMPAT_QUOTA_LIMIT_ID: &str = "codex";

fn normalized_selection_family_sql(selection_family_expr: &str) -> String {
    format!("COALESCE(NULLIF({selection_family_expr}, ''), '{COMPAT_QUOTA_LIMIT_ID}')")
}

pub(super) fn selection_quota_state_joins_sql(
    selection_quota_alias: &str,
    fallback_quota_alias: &str,
    account_id_expr: &str,
    selection_family_expr: &str,
) -> String {
    let normalized_selection_family_expr = normalized_selection_family_sql(selection_family_expr);
    format!(
        r#"
LEFT JOIN account_quota_state AS {selection_quota_alias}
  ON {selection_quota_alias}.account_id = {account_id_expr}
 AND {selection_quota_alias}.limit_id = {normalized_selection_family_expr}
LEFT JOIN account_quota_state AS {fallback_quota_alias}
  ON {fallback_quota_alias}.account_id = {account_id_expr}
 AND {fallback_quota_alias}.limit_id = '{COMPAT_QUOTA_LIMIT_ID}'
 AND {selection_quota_alias}.account_id IS NULL
        "#,
    )
}

pub(super) fn selection_quota_state_field_sql(
    selection_quota_alias: &str,
    fallback_quota_alias: &str,
    field_name: &str,
) -> String {
    format!("COALESCE({selection_quota_alias}.{field_name}, {fallback_quota_alias}.{field_name})")
}

pub(super) async fn read_account_selection_family<'e, E>(
    executor: E,
    account_id: &str,
) -> anyhow::Result<String>
where
    E: Executor<'e, Database = Sqlite>,
{
    Ok(sqlx::query_scalar::<_, Option<String>>(
        r#"
SELECT NULLIF(backend_family, '')
FROM account_registry
WHERE account_id = ?
        "#,
    )
    .bind(account_id)
    .fetch_optional(executor)
    .await?
    .flatten()
    .unwrap_or_else(|| COMPAT_QUOTA_LIMIT_ID.to_string()))
}

impl StateRuntime {
    pub async fn upsert_account_quota_state(
        &self,
        record: AccountQuotaStateRecord,
    ) -> anyhow::Result<()> {
        let observed_at = account_datetime_to_epoch_seconds(record.observed_at);
        let observed_at_nanos = account_datetime_to_epoch_nanos(record.observed_at);
        let primary_resets_at = record
            .primary_resets_at
            .map(account_datetime_to_epoch_seconds);
        let secondary_resets_at = record
            .secondary_resets_at
            .map(account_datetime_to_epoch_seconds);
        let updated_at = account_datetime_to_epoch_seconds(Utc::now());
        let predicted_blocked_until = if record.exhausted_windows.is_exhausted() {
            record
                .predicted_blocked_until
                .map(account_datetime_to_epoch_seconds)
        } else {
            None
        };
        let next_probe_after = if record.exhausted_windows.is_exhausted() {
            record
                .next_probe_after
                .map(account_datetime_to_epoch_seconds)
        } else {
            None
        };

        sqlx::query(
            r#"
INSERT INTO account_quota_state (
    account_id,
    limit_id,
    primary_used_percent,
    primary_resets_at,
    secondary_used_percent,
    secondary_resets_at,
    observed_at,
    observed_at_nanos,
    exhausted_windows,
    predicted_blocked_until,
    next_probe_after,
    probe_backoff_level,
    last_probe_result,
    updated_at
 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(account_id, limit_id) DO UPDATE SET
    primary_used_percent = excluded.primary_used_percent,
    primary_resets_at = excluded.primary_resets_at,
    secondary_used_percent = excluded.secondary_used_percent,
    secondary_resets_at = excluded.secondary_resets_at,
    observed_at = excluded.observed_at,
    observed_at_nanos = excluded.observed_at_nanos,
    exhausted_windows = excluded.exhausted_windows,
    predicted_blocked_until = excluded.predicted_blocked_until,
    next_probe_after = excluded.next_probe_after,
    probe_backoff_level = excluded.probe_backoff_level,
    last_probe_result = excluded.last_probe_result,
    updated_at = excluded.updated_at
WHERE excluded.observed_at_nanos >= COALESCE(account_quota_state.observed_at_nanos, account_quota_state.observed_at * 1000000000)
            "#,
        )
        .bind(&record.account_id)
        .bind(&record.limit_id)
        .bind(record.primary_used_percent)
        .bind(primary_resets_at)
        .bind(record.secondary_used_percent)
        .bind(secondary_resets_at)
        .bind(observed_at)
        .bind(observed_at_nanos)
        .bind(record.exhausted_windows.as_str())
        .bind(predicted_blocked_until)
        .bind(next_probe_after)
        .bind(0_i64)
        .bind(Option::<String>::None)
        .bind(updated_at)
        .execute(self.pool.as_ref())
        .await?;

        Ok(())
    }

    pub async fn read_account_quota_state(
        &self,
        account_id: &str,
        limit_id: &str,
    ) -> anyhow::Result<Option<AccountQuotaStateRecord>> {
        read_account_quota_state(self.pool.as_ref(), account_id, limit_id).await
    }

    pub async fn read_selection_quota_state(
        &self,
        account_id: &str,
        selection_family: &str,
    ) -> anyhow::Result<Option<AccountQuotaStateRecord>> {
        let selected =
            read_account_quota_state(self.pool.as_ref(), account_id, selection_family).await?;
        if selected.is_some() || selection_family == COMPAT_QUOTA_LIMIT_ID {
            return Ok(selected);
        }

        read_account_quota_state(self.pool.as_ref(), account_id, COMPAT_QUOTA_LIMIT_ID).await
    }

    pub async fn read_registered_account_selection_quota_state(
        &self,
        account_id: &str,
    ) -> anyhow::Result<Option<AccountQuotaStateRecord>> {
        let selection_family =
            read_account_selection_family(self.pool.as_ref(), account_id).await?;
        self.read_selection_quota_state(account_id, &selection_family)
            .await
    }

    pub async fn read_selection_quota_compat_health_state(
        &self,
        account_id: &str,
        selection_family: &str,
    ) -> anyhow::Result<Option<AccountHealthState>> {
        Ok(self
            .read_selection_quota_state(account_id, selection_family)
            .await?
            .map(|record| record.compatibility_health_state()))
    }

    pub async fn reserve_account_quota_probe(
        &self,
        account_id: &str,
        limit_id: &str,
        now: DateTime<Utc>,
        reserved_until: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let updated_at = account_datetime_to_epoch_seconds(Utc::now());
        let result = sqlx::query(
            r#"
UPDATE account_quota_state
SET next_probe_after = ?,
    updated_at = ?
WHERE account_id = ?
  AND limit_id = ?
  AND exhausted_windows != ?
  AND COALESCE(next_probe_after, 0) <= ?
            "#,
        )
        .bind(account_datetime_to_epoch_seconds(reserved_until))
        .bind(updated_at)
        .bind(account_id)
        .bind(limit_id)
        .bind(QuotaExhaustedWindows::None.as_str())
        .bind(account_datetime_to_epoch_seconds(now))
        .execute(self.pool.as_ref())
        .await?;

        Ok(result.rows_affected() == 1)
    }

    pub async fn record_account_quota_probe_success(
        &self,
        account_id: &str,
        limit_id: &str,
        observed_at: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let observed_at_nanos = account_datetime_to_epoch_nanos(observed_at);
        let result = sqlx::query(
            r#"
UPDATE account_quota_state
SET observed_at = ?,
    observed_at_nanos = ?,
    exhausted_windows = ?,
    predicted_blocked_until = NULL,
    next_probe_after = NULL,
    probe_backoff_level = 0,
    last_probe_result = ?,
    updated_at = ?
WHERE account_id = ?
  AND limit_id = ?
  AND COALESCE(observed_at_nanos, observed_at * 1000000000) <= ?
            "#,
        )
        .bind(account_datetime_to_epoch_seconds(observed_at))
        .bind(observed_at_nanos)
        .bind(QuotaExhaustedWindows::None.as_str())
        .bind(QuotaProbeResult::Success.as_str())
        .bind(account_datetime_to_epoch_seconds(Utc::now()))
        .bind(account_id)
        .bind(limit_id)
        .bind(observed_at_nanos)
        .execute(self.pool.as_ref())
        .await?;

        Ok(result.rows_affected() == 1)
    }

    pub async fn record_account_quota_probe_still_blocked(
        &self,
        account_id: &str,
        limit_id: &str,
        observed_at: DateTime<Utc>,
        exhausted_windows: QuotaExhaustedWindows,
        predicted_blocked_until: Option<DateTime<Utc>>,
        next_probe_after: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let exhausted_windows = if exhausted_windows.is_exhausted() {
            exhausted_windows
        } else {
            QuotaExhaustedWindows::Unknown
        };
        let observed_at_nanos = account_datetime_to_epoch_nanos(observed_at);
        let result = sqlx::query(
            r#"
UPDATE account_quota_state
SET observed_at = ?,
    observed_at_nanos = ?,
    exhausted_windows = ?,
    predicted_blocked_until = ?,
    next_probe_after = ?,
    probe_backoff_level = 0,
    last_probe_result = ?,
    updated_at = ?
WHERE account_id = ?
  AND limit_id = ?
  AND COALESCE(observed_at_nanos, observed_at * 1000000000) <= ?
            "#,
        )
        .bind(account_datetime_to_epoch_seconds(observed_at))
        .bind(observed_at_nanos)
        .bind(exhausted_windows.as_str())
        .bind(predicted_blocked_until.map(account_datetime_to_epoch_seconds))
        .bind(account_datetime_to_epoch_seconds(next_probe_after))
        .bind(QuotaProbeResult::StillBlocked.as_str())
        .bind(account_datetime_to_epoch_seconds(Utc::now()))
        .bind(account_id)
        .bind(limit_id)
        .bind(observed_at_nanos)
        .execute(self.pool.as_ref())
        .await?;

        Ok(result.rows_affected() == 1)
    }

    pub async fn record_account_quota_probe_ambiguous(
        &self,
        account_id: &str,
        limit_id: &str,
        observed_at: DateTime<Utc>,
        predicted_blocked_until: DateTime<Utc>,
        next_probe_after: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let observed_at_nanos = account_datetime_to_epoch_nanos(observed_at);
        let result = sqlx::query(
            r#"
UPDATE account_quota_state
SET observed_at = ?,
    observed_at_nanos = ?,
    exhausted_windows = CASE
        WHEN exhausted_windows = 'none' THEN ?
        ELSE exhausted_windows
    END,
    predicted_blocked_until = ?,
    next_probe_after = ?,
    probe_backoff_level = probe_backoff_level + 1,
    last_probe_result = ?,
    updated_at = ?
WHERE account_id = ?
  AND limit_id = ?
  AND COALESCE(observed_at_nanos, observed_at * 1000000000) <= ?
            "#,
        )
        .bind(account_datetime_to_epoch_seconds(observed_at))
        .bind(observed_at_nanos)
        .bind(QuotaExhaustedWindows::Unknown.as_str())
        .bind(account_datetime_to_epoch_seconds(predicted_blocked_until))
        .bind(account_datetime_to_epoch_seconds(next_probe_after))
        .bind(QuotaProbeResult::Ambiguous.as_str())
        .bind(account_datetime_to_epoch_seconds(Utc::now()))
        .bind(account_id)
        .bind(limit_id)
        .bind(observed_at_nanos)
        .execute(self.pool.as_ref())
        .await?;

        Ok(result.rows_affected() == 1)
    }
}

async fn read_account_quota_state<'e, E>(
    executor: E,
    account_id: &str,
    limit_id: &str,
) -> anyhow::Result<Option<AccountQuotaStateRecord>>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let row = sqlx::query(
        r#"
SELECT
    account_id,
    limit_id,
    primary_used_percent,
    primary_resets_at,
    secondary_used_percent,
    secondary_resets_at,
    observed_at,
    COALESCE(observed_at_nanos, observed_at * 1000000000) AS observed_at_nanos,
    exhausted_windows,
    predicted_blocked_until,
    next_probe_after,
    probe_backoff_level,
    last_probe_result
FROM account_quota_state
WHERE account_id = ?
  AND limit_id = ?
            "#,
    )
    .bind(account_id)
    .bind(limit_id)
    .fetch_optional(executor)
    .await?;

    row.map(account_quota_state_record_from_row).transpose()
}

fn account_quota_state_record_from_row(
    row: sqlx::sqlite::SqliteRow,
) -> anyhow::Result<AccountQuotaStateRecord> {
    let last_probe_result: Option<String> = row.try_get("last_probe_result")?;

    Ok(AccountQuotaStateRecord {
        account_id: row.try_get("account_id")?,
        limit_id: row.try_get("limit_id")?,
        primary_used_percent: row.try_get("primary_used_percent")?,
        primary_resets_at: row
            .try_get::<Option<i64>, _>("primary_resets_at")?
            .map(account_epoch_seconds_to_datetime)
            .transpose()?,
        secondary_used_percent: row.try_get("secondary_used_percent")?,
        secondary_resets_at: row
            .try_get::<Option<i64>, _>("secondary_resets_at")?
            .map(account_epoch_seconds_to_datetime)
            .transpose()?,
        observed_at: account_epoch_nanos_to_datetime(row.try_get("observed_at_nanos")?)?,
        exhausted_windows: QuotaExhaustedWindows::try_from(
            row.try_get::<String, _>("exhausted_windows")?.as_str(),
        )?,
        predicted_blocked_until: row
            .try_get::<Option<i64>, _>("predicted_blocked_until")?
            .map(account_epoch_seconds_to_datetime)
            .transpose()?,
        next_probe_after: row
            .try_get::<Option<i64>, _>("next_probe_after")?
            .map(account_epoch_seconds_to_datetime)
            .transpose()?,
        probe_backoff_level: row.try_get("probe_backoff_level")?,
        last_probe_result: last_probe_result
            .as_deref()
            .map(QuotaProbeResult::try_from)
            .transpose()?,
    })
}
