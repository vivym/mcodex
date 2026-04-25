use super::StateRuntime;
use crate::model::AccountPoolAccountRecord;
use crate::model::AccountPoolAccountsListQuery;
use crate::model::AccountPoolAccountsPage;
use crate::model::AccountPoolDiagnosticsRecord;
use crate::model::AccountPoolEventRecord;
use crate::model::AccountPoolEventsCursor;
use crate::model::AccountPoolEventsListQuery;
use crate::model::AccountPoolEventsPage;
use crate::model::AccountPoolIssueRecord;
use crate::model::AccountPoolLeaseRecord;
use crate::model::AccountPoolQuotaFamilyRecord;
use crate::model::AccountPoolQuotaRecord;
use crate::model::AccountPoolQuotaWindowRecord;
use crate::model::AccountPoolSelectionRecord;
use crate::model::AccountPoolSnapshotRecord;
use crate::model::AccountPoolSummaryRecord;
use crate::model::account_datetime_to_epoch_seconds;
use crate::model::account_epoch_nanos_to_datetime;
use crate::model::account_epoch_seconds_to_datetime;
use chrono::Utc;
use serde_json::Value;
use sqlx::Executor;
use sqlx::QueryBuilder;
use sqlx::Row;
use sqlx::Sqlite;
use std::collections::HashMap;

const DEFAULT_ACCOUNT_PAGE_LIMIT: u32 = 50;
const MAX_ACCOUNT_PAGE_LIMIT: u32 = 200;
const DEFAULT_EVENT_PAGE_LIMIT: u32 = 50;
const MAX_EVENT_PAGE_LIMIT: u32 = 200;

impl StateRuntime {
    pub async fn append_account_pool_event(
        &self,
        event: AccountPoolEventRecord,
    ) -> anyhow::Result<()> {
        append_account_pool_event_tx(self.pool.as_ref(), &event).await
    }

    pub async fn read_account_pool_snapshot(
        &self,
        pool_id: &str,
    ) -> anyhow::Result<AccountPoolSnapshotRecord> {
        let refreshed_at = Utc::now();
        let quota_joins = super::account_pool_quota::selection_quota_state_joins_sql(
            "selection_quota_state",
            "fallback_quota_state",
            "membership.account_id",
            "account_registry.backend_family",
        );
        let quota_exhausted_windows = super::account_pool_quota::selection_quota_state_field_sql(
            "selection_quota_state",
            "fallback_quota_state",
            "exhausted_windows",
        );
        let row = sqlx::query(&format!(
            r#"
SELECT
    COUNT(*) AS total_accounts,
    COALESCE(SUM(CASE WHEN active_lease.lease_id IS NOT NULL THEN 1 ELSE 0 END), 0) AS active_leases,
    COALESCE(SUM(CASE
        WHEN account_registry.enabled = 1
          AND COALESCE(account_runtime_state.health_state, '') != 'unauthorized'
          AND COALESCE({quota_exhausted_windows}, 'none') = 'none'
          AND active_lease.lease_id IS NULL
            THEN 1
        ELSE 0
    END), 0) AS available_accounts,
    COALESCE(SUM(CASE WHEN active_lease.lease_id IS NOT NULL THEN 1 ELSE 0 END), 0) AS leased_accounts
FROM account_pool_membership AS membership
JOIN account_registry
  ON account_registry.account_id = membership.account_id
LEFT JOIN account_runtime_state
  ON account_runtime_state.account_id = membership.account_id
{quota_joins}
LEFT JOIN account_leases AS active_lease
  ON active_lease.account_id = membership.account_id
 AND active_lease.pool_id = membership.pool_id
 AND active_lease.released_at IS NULL
 AND active_lease.expires_at > ?
WHERE membership.pool_id = ?
            "#,
        ))
        .bind(account_datetime_to_epoch_seconds(refreshed_at))
        .bind(pool_id)
        .fetch_one(self.pool.as_ref())
        .await?;

        Ok(AccountPoolSnapshotRecord {
            pool_id: pool_id.to_string(),
            summary: AccountPoolSummaryRecord {
                total_accounts: row.try_get::<i64, _>("total_accounts")? as u32,
                active_leases: row.try_get::<i64, _>("active_leases")? as u32,
                available_accounts: Some(row.try_get::<i64, _>("available_accounts")? as u32),
                leased_accounts: Some(row.try_get::<i64, _>("leased_accounts")? as u32),
                paused_accounts: None,
                draining_accounts: None,
                near_exhausted_accounts: None,
                exhausted_accounts: None,
                error_accounts: None,
            },
            refreshed_at,
        })
    }

    pub async fn list_account_pool_accounts(
        &self,
        request: AccountPoolAccountsListQuery,
    ) -> anyhow::Result<AccountPoolAccountsPage> {
        let selection = self.read_account_startup_selection().await?;
        let now = Utc::now();
        let limit = normalize_page_limit(
            request.limit,
            DEFAULT_ACCOUNT_PAGE_LIMIT,
            MAX_ACCOUNT_PAGE_LIMIT,
        );
        let states = request.states.filter(|states| !states.is_empty());
        let cursor = request
            .cursor
            .as_deref()
            .map(decode_account_cursor)
            .transpose()?;

        let quota_joins = super::account_pool_quota::selection_quota_state_joins_sql(
            "selection_quota_state",
            "fallback_quota_state",
            "membership.account_id",
            "account_registry.backend_family",
        );
        let quota_exhausted_windows = super::account_pool_quota::selection_quota_state_field_sql(
            "selection_quota_state",
            "fallback_quota_state",
            "exhausted_windows",
        );
        let quota_predicted_blocked_until =
            super::account_pool_quota::selection_quota_state_field_sql(
                "selection_quota_state",
                "fallback_quota_state",
                "predicted_blocked_until",
            );
        let quota_next_probe_after = super::account_pool_quota::selection_quota_state_field_sql(
            "selection_quota_state",
            "fallback_quota_state",
            "next_probe_after",
        );
        let quota_updated_at = super::account_pool_quota::selection_quota_state_field_sql(
            "selection_quota_state",
            "fallback_quota_state",
            "updated_at",
        );
        let mut builder = QueryBuilder::<Sqlite>::new(&format!(
            r#"
SELECT
    membership.account_id,
    membership.position,
    account_registry.backend_account_handle,
    account_registry.account_kind,
    account_registry.enabled,
    account_registry.healthy,
    account_registry.updated_at AS registry_updated_at,
    account_runtime_state.health_state,
    account_runtime_state.updated_at AS health_updated_at,
    {quota_exhausted_windows} AS quota_exhausted_windows,
    {quota_predicted_blocked_until} AS quota_predicted_blocked_until,
    {quota_next_probe_after} AS quota_next_probe_after,
    {quota_updated_at} AS quota_updated_at,
    active_lease.lease_id,
    active_lease.lease_epoch,
    active_lease.holder_instance_id,
    active_lease.acquired_at,
    active_lease.renewed_at,
    active_lease.expires_at
FROM account_pool_membership AS membership
JOIN account_registry
  ON account_registry.account_id = membership.account_id
LEFT JOIN account_runtime_state
  ON account_runtime_state.account_id = membership.account_id
{quota_joins}
LEFT JOIN account_leases AS active_lease
  ON active_lease.account_id = membership.account_id
 AND active_lease.pool_id = membership.pool_id
 AND active_lease.released_at IS NULL
 AND active_lease.expires_at > 
            "#,
        ));
        builder.push_bind(account_datetime_to_epoch_seconds(now));
        builder.push(" WHERE membership.pool_id = ");
        builder.push_bind(&request.pool_id);

        if let Some(account_id) = request.account_id.as_ref() {
            builder.push(" AND membership.account_id = ");
            builder.push_bind(account_id);
        }

        if let Some(account_kinds) = request.account_kinds.as_ref()
            && !account_kinds.is_empty()
        {
            builder.push(" AND account_registry.account_kind IN (");
            let mut separated = builder.separated(", ");
            for account_kind in account_kinds {
                separated.push_bind(account_kind);
            }
            builder.push(")");
        }

        if request.account_id.is_none()
            && let Some((position, account_id)) = cursor.as_ref()
        {
            builder.push(" AND (membership.position > ");
            builder.push_bind(*position);
            builder.push(" OR (membership.position = ");
            builder.push_bind(*position);
            builder.push(" AND membership.account_id > ");
            builder.push_bind(account_id);
            builder.push("))");
        }

        builder.push(" ORDER BY membership.position ASC, membership.account_id ASC");
        if states.is_none() && request.account_id.is_none() {
            builder.push(" LIMIT ");
            builder.push_bind(i64::from(limit) + 1);
        }

        let rows = builder.build().fetch_all(self.pool.as_ref()).await?;
        let mut entries = Vec::with_capacity(rows.len().min(limit as usize + 1));
        for row in &rows {
            let position: i64 = row.try_get("position")?;
            let account_id: String = row.try_get("account_id")?;
            let enabled = row.try_get::<i64, _>("enabled")? != 0;
            let healthy = row.try_get::<i64, _>("healthy")? != 0;
            let legacy_health_state = row.try_get::<Option<String>, _>("health_state")?;
            let quota_exhausted = quota_exhausted_windows_is_exhausted(
                row.try_get::<Option<String>, _>("quota_exhausted_windows")?
                    .as_deref(),
            );
            let selector_auth_eligible =
                selector_auth_eligible(healthy, legacy_health_state.as_deref());
            let health_state =
                derive_account_compat_health_state(legacy_health_state.as_deref(), quota_exhausted)
                    .map(ToOwned::to_owned);
            let current_lease = match row.try_get::<Option<String>, _>("lease_id")? {
                Some(lease_id) => Some(AccountPoolLeaseRecord {
                    lease_id,
                    lease_epoch: row.try_get::<i64, _>("lease_epoch")? as u64,
                    holder_instance_id: row.try_get("holder_instance_id")?,
                    acquired_at: account_epoch_seconds_to_datetime(row.try_get("acquired_at")?)?,
                    renewed_at: account_epoch_seconds_to_datetime(row.try_get("renewed_at")?)?,
                    expires_at: account_epoch_seconds_to_datetime(row.try_get("expires_at")?)?,
                }),
                None => None,
            };
            let quota_next_eligible_at = if quota_exhausted {
                row.try_get::<Option<i64>, _>("quota_predicted_blocked_until")?
                    .or(row.try_get::<Option<i64>, _>("quota_next_probe_after")?)
                    .map(account_epoch_seconds_to_datetime)
                    .transpose()?
            } else {
                None
            };
            let operational_state = derive_account_operational_state(
                enabled,
                health_state.as_deref(),
                quota_exhausted,
                current_lease.is_some(),
            )
            .map(ToOwned::to_owned);
            if let Some(states) = states.as_ref()
                && !matches_account_state_filter(operational_state.as_deref(), states)
            {
                continue;
            }
            let next_eligible_at = if enabled && selector_auth_eligible {
                match (
                    current_lease.as_ref().map(|lease| lease.expires_at),
                    quota_next_eligible_at,
                ) {
                    (Some(lease_expires_at), Some(quota_next_eligible_at)) => {
                        Some(lease_expires_at.max(quota_next_eligible_at))
                    }
                    (Some(lease_expires_at), None) => Some(lease_expires_at),
                    (None, quota_next_eligible_at) => quota_next_eligible_at,
                }
            } else {
                None
            };
            entries.push((
                position,
                account_id.clone(),
                AccountPoolAccountRecord {
                    account_id: account_id.clone(),
                    backend_account_ref: row
                        .try_get::<String, _>("backend_account_handle")
                        .ok()
                        .filter(|value| !value.is_empty()),
                    account_kind: row.try_get("account_kind")?,
                    enabled,
                    health_state,
                    operational_state,
                    allocatable: None,
                    status_reason_code: None,
                    status_message: None,
                    current_lease,
                    quota: None::<AccountPoolQuotaRecord>,
                    quotas: Vec::new(),
                    selection: Some(AccountPoolSelectionRecord {
                        eligible: !selection.suppressed
                            && enabled
                            && selector_auth_eligible
                            && !quota_exhausted
                            && next_eligible_at.is_none(),
                        next_eligible_at,
                        preferred: selection.preferred_account_id.as_deref()
                            == Some(account_id.as_str()),
                        suppressed: selection.suppressed,
                    }),
                    updated_at: account_epoch_seconds_to_datetime(
                        row.try_get::<i64, _>("quota_updated_at")
                            .or_else(|_| row.try_get::<i64, _>("health_updated_at"))
                            .or_else(|_| row.try_get::<i64, _>("registry_updated_at"))?,
                    )?,
                },
            ));
        }

        let next_cursor = if request.account_id.is_none() && entries.len() > limit as usize {
            entries
                .get(limit as usize - 1)
                .map(|(position, account_id, _)| {
                    encode_account_cursor(*position, account_id.clone())
                })
        } else {
            None
        };
        let mut data: Vec<_> = entries
            .into_iter()
            .take(limit as usize)
            .map(|(_, _, record)| record)
            .collect();
        let visible_account_ids: Vec<_> = data
            .iter()
            .map(|record| record.account_id.clone())
            .collect();
        let mut quota_families_by_account =
            load_account_pool_quota_families(self.pool.as_ref(), &visible_account_ids).await?;
        for record in &mut data {
            record.quotas = quota_families_by_account
                .remove(&record.account_id)
                .unwrap_or_default();
            record.quota = record
                .quotas
                .iter()
                .find(|quota| quota.limit_id == "codex")
                .map(derive_legacy_quota_record);
        }

        Ok(AccountPoolAccountsPage { data, next_cursor })
    }

    pub async fn list_account_pool_events(
        &self,
        request: AccountPoolEventsListQuery,
    ) -> anyhow::Result<AccountPoolEventsPage> {
        let limit = normalize_page_limit(
            request.limit,
            DEFAULT_EVENT_PAGE_LIMIT,
            MAX_EVENT_PAGE_LIMIT,
        );
        let cursor = request
            .cursor
            .as_deref()
            .map(decode_event_cursor)
            .transpose()?;

        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
SELECT
    event_id,
    occurred_at,
    pool_id,
    account_id,
    lease_id,
    holder_instance_id,
    event_type,
    reason_code,
    message,
    details_json
FROM account_pool_events
WHERE pool_id = 
            "#,
        );
        builder.push_bind(&request.pool_id);

        if let Some(account_id) = request.account_id.as_ref() {
            builder.push(" AND account_id = ");
            builder.push_bind(account_id);
        }

        if let Some(types) = request.types.as_ref()
            && !types.is_empty()
        {
            builder.push(" AND event_type IN (");
            let mut separated = builder.separated(", ");
            for event_type in types {
                separated.push_bind(event_type);
            }
            builder.push(")");
        }

        if let Some(cursor) = cursor.as_ref() {
            builder.push(" AND (occurred_at < ");
            builder.push_bind(cursor.occurred_at);
            builder.push(" OR (occurred_at = ");
            builder.push_bind(cursor.occurred_at);
            builder.push(" AND event_id < ");
            builder.push_bind(&cursor.event_id);
            builder.push("))");
        }

        builder.push(" ORDER BY occurred_at DESC, event_id DESC LIMIT ");
        builder.push_bind(i64::from(limit) + 1);

        let rows = builder.build().fetch_all(self.pool.as_ref()).await?;
        let mut data = Vec::with_capacity(rows.len().min(limit as usize));
        for row in rows.iter().take(limit as usize) {
            let details_json = row
                .try_get::<Option<String>, _>("details_json")?
                .map(|json| serde_json::from_str::<Value>(&json))
                .transpose()?;
            data.push(AccountPoolEventRecord {
                event_id: row.try_get("event_id")?,
                occurred_at: account_epoch_seconds_to_datetime(row.try_get("occurred_at")?)?,
                pool_id: row.try_get("pool_id")?,
                account_id: row.try_get("account_id")?,
                lease_id: row.try_get("lease_id")?,
                holder_instance_id: row.try_get("holder_instance_id")?,
                event_type: row.try_get("event_type")?,
                reason_code: row.try_get("reason_code")?,
                message: row.try_get("message")?,
                details_json,
            });
        }

        let next_cursor = if rows.len() > limit as usize {
            match rows.get(limit as usize - 1) {
                Some(row) => Some(encode_event_cursor(AccountPoolEventsCursor {
                    occurred_at: row.try_get("occurred_at")?,
                    event_id: row.try_get("event_id")?,
                })),
                None => None,
            }
        } else {
            None
        };

        Ok(AccountPoolEventsPage { data, next_cursor })
    }

    pub async fn read_account_pool_diagnostics(
        &self,
        pool_id: &str,
    ) -> anyhow::Result<AccountPoolDiagnosticsRecord> {
        let generated_at = Utc::now();
        let selection = self.read_account_startup_selection().await?;
        let recent_events = self
            .list_account_pool_events(AccountPoolEventsListQuery {
                pool_id: pool_id.to_string(),
                account_id: None,
                types: None,
                cursor: None,
                limit: Some(5),
            })
            .await?;
        let quota_joins = super::account_pool_quota::selection_quota_state_joins_sql(
            "selection_quota_state",
            "fallback_quota_state",
            "membership.account_id",
            "account_registry.backend_family",
        );
        let quota_exhausted_windows = super::account_pool_quota::selection_quota_state_field_sql(
            "selection_quota_state",
            "fallback_quota_state",
            "exhausted_windows",
        );
        let quota_predicted_blocked_until =
            super::account_pool_quota::selection_quota_state_field_sql(
                "selection_quota_state",
                "fallback_quota_state",
                "predicted_blocked_until",
            );
        let quota_next_probe_after = super::account_pool_quota::selection_quota_state_field_sql(
            "selection_quota_state",
            "fallback_quota_state",
            "next_probe_after",
        );
        let rows = sqlx::query(&format!(
            r#"
SELECT
    membership.account_id,
    account_registry.enabled,
    account_registry.healthy,
    account_runtime_state.health_state,
    {quota_exhausted_windows} AS quota_exhausted_windows,
    {quota_predicted_blocked_until} AS quota_predicted_blocked_until,
    {quota_next_probe_after} AS quota_next_probe_after,
    active_lease.holder_instance_id,
    active_lease.expires_at
FROM account_pool_membership AS membership
JOIN account_registry
  ON account_registry.account_id = membership.account_id
LEFT JOIN account_runtime_state
  ON account_runtime_state.account_id = membership.account_id
{quota_joins}
LEFT JOIN account_leases AS active_lease
  ON active_lease.account_id = membership.account_id
 AND active_lease.pool_id = membership.pool_id
 AND active_lease.released_at IS NULL
 AND active_lease.expires_at > ?
WHERE membership.pool_id = ?
ORDER BY membership.position ASC, membership.account_id ASC
            "#,
        ))
        .bind(account_datetime_to_epoch_seconds(generated_at))
        .bind(pool_id)
        .fetch_all(self.pool.as_ref())
        .await?;

        let mut issues = Vec::new();
        let mut allocatable_accounts = 0_u32;
        let mut viable_active_leases = 0_u32;
        let mut next_relevant_at: Option<chrono::DateTime<Utc>> = None;
        let mut preferred_in_pool = false;
        for row in rows {
            let enabled = row.try_get::<i64, _>("enabled")? != 0;
            let healthy = row.try_get::<i64, _>("healthy")? != 0;
            let health_state = row.try_get::<Option<String>, _>("health_state")?;
            let selector_auth_eligible = selector_auth_eligible(healthy, health_state.as_deref());
            let quota_exhausted = quota_exhausted_windows_is_exhausted(
                row.try_get::<Option<String>, _>("quota_exhausted_windows")?
                    .as_deref(),
            );
            let expires_at = row
                .try_get::<Option<i64>, _>("expires_at")?
                .map(account_epoch_seconds_to_datetime)
                .transpose()?;

            if enabled && selector_auth_eligible && !quota_exhausted && expires_at.is_none() {
                allocatable_accounts += 1;
            }

            if let Some(expires_at) = expires_at {
                if health_state.as_deref() != Some("unauthorized") {
                    viable_active_leases += 1;
                }
                next_relevant_at = match next_relevant_at {
                    Some(current) => Some(current.min(expires_at)),
                    None => Some(expires_at),
                };
            }

            let account_id: String = row.try_get("account_id")?;
            if selection.preferred_account_id.as_deref() == Some(account_id.as_str()) {
                preferred_in_pool = true;
            }
            if quota_exhausted {
                let quota_next_relevant_at = row
                    .try_get::<Option<i64>, _>("quota_predicted_blocked_until")?
                    .or(row.try_get::<Option<i64>, _>("quota_next_probe_after")?)
                    .map(account_epoch_seconds_to_datetime)
                    .transpose()?;
                next_relevant_at = match (next_relevant_at, quota_next_relevant_at) {
                    (Some(current), Some(next)) => Some(current.min(next)),
                    (None, Some(next)) => Some(next),
                    (current, None) => current,
                };
                issues.push(AccountPoolIssueRecord {
                    severity: "warning".to_string(),
                    reason_code: "cooldownActive".to_string(),
                    message: format!("account {account_id} is in cooldown"),
                    account_id: Some(account_id.clone()),
                    holder_instance_id: row.try_get("holder_instance_id")?,
                    next_relevant_at: quota_next_relevant_at,
                });
            }
            if let Some("unauthorized") = health_state.as_deref() {
                issues.push(AccountPoolIssueRecord {
                    severity: "error".to_string(),
                    reason_code: "authFailure".to_string(),
                    message: format!("account {account_id} is unauthorized"),
                    account_id: Some(account_id.clone()),
                    holder_instance_id: row.try_get("holder_instance_id")?,
                    next_relevant_at: None,
                })
            }
        }

        if selection.suppressed
            && (selection.default_pool_id.as_deref() == Some(pool_id) || preferred_in_pool)
        {
            issues.push(AccountPoolIssueRecord {
                severity: "warning".to_string(),
                reason_code: "durablySuppressed".to_string(),
                message: "startup selection is durably suppressed".to_string(),
                account_id: selection.preferred_account_id.clone(),
                holder_instance_id: None,
                next_relevant_at: None,
            });
        }

        if allocatable_accounts == 0 && viable_active_leases == 0 {
            let lease_failure = recent_events
                .data
                .iter()
                .find(|event| event.event_type == "leaseAcquireFailed");
            issues.push(AccountPoolIssueRecord {
                severity: "error".to_string(),
                reason_code: lease_failure
                    .and_then(|event| event.reason_code.clone())
                    .unwrap_or_else(|| "noEligibleAccount".to_string()),
                message: lease_failure
                    .map(|event| event.message.clone())
                    .unwrap_or_else(|| "no eligible account is currently available".to_string()),
                account_id: lease_failure.and_then(|event| event.account_id.clone()),
                holder_instance_id: lease_failure
                    .and_then(|event| event.holder_instance_id.clone()),
                next_relevant_at,
            });
        }

        let status = if issues.is_empty() {
            "healthy"
        } else if viable_active_leases == 0 && allocatable_accounts == 0 {
            "blocked"
        } else {
            "degraded"
        };

        Ok(AccountPoolDiagnosticsRecord {
            pool_id: pool_id.to_string(),
            generated_at,
            status: status.to_string(),
            issues,
        })
    }
}

pub(super) async fn append_account_pool_event_tx<'e, E>(
    executor: E,
    event: &AccountPoolEventRecord,
) -> anyhow::Result<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    let details_json = event
        .details_json
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    sqlx::query(
        r#"
INSERT INTO account_pool_events (
    event_id,
    occurred_at,
    pool_id,
    account_id,
    lease_id,
    holder_instance_id,
    event_type,
    reason_code,
    message,
    details_json
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&event.event_id)
    .bind(account_datetime_to_epoch_seconds(event.occurred_at))
    .bind(&event.pool_id)
    .bind(&event.account_id)
    .bind(&event.lease_id)
    .bind(&event.holder_instance_id)
    .bind(&event.event_type)
    .bind(&event.reason_code)
    .bind(&event.message)
    .bind(details_json)
    .execute(executor)
    .await?;
    Ok(())
}

async fn load_account_pool_quota_families<'e, E>(
    executor: E,
    account_ids: &[String],
) -> anyhow::Result<HashMap<String, Vec<AccountPoolQuotaFamilyRecord>>>
where
    E: Executor<'e, Database = Sqlite>,
{
    if account_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut builder = QueryBuilder::<Sqlite>::new(
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
    next_probe_after
FROM account_quota_state
WHERE account_id IN (
        "#,
    );
    let mut separated = builder.separated(", ");
    for account_id in account_ids {
        separated.push_bind(account_id);
    }
    builder.push(") ORDER BY account_id ASC, limit_id ASC");

    let rows = builder.build().fetch_all(executor).await?;
    let mut by_account: HashMap<String, Vec<AccountPoolQuotaFamilyRecord>> = HashMap::new();
    for row in rows {
        let account_id: String = row.try_get("account_id")?;
        let family = AccountPoolQuotaFamilyRecord {
            limit_id: row.try_get("limit_id")?,
            primary: AccountPoolQuotaWindowRecord {
                used_percent: row.try_get("primary_used_percent")?,
                resets_at: row
                    .try_get::<Option<i64>, _>("primary_resets_at")?
                    .map(account_epoch_seconds_to_datetime)
                    .transpose()?,
            },
            secondary: AccountPoolQuotaWindowRecord {
                used_percent: row.try_get("secondary_used_percent")?,
                resets_at: row
                    .try_get::<Option<i64>, _>("secondary_resets_at")?
                    .map(account_epoch_seconds_to_datetime)
                    .transpose()?,
            },
            exhausted_windows: row.try_get("exhausted_windows")?,
            predicted_blocked_until: row
                .try_get::<Option<i64>, _>("predicted_blocked_until")?
                .map(account_epoch_seconds_to_datetime)
                .transpose()?,
            next_probe_after: row
                .try_get::<Option<i64>, _>("next_probe_after")?
                .map(account_epoch_seconds_to_datetime)
                .transpose()?,
            observed_at: account_epoch_nanos_to_datetime(row.try_get("observed_at_nanos")?)?,
        };
        by_account.entry(account_id).or_default().push(family);
    }

    Ok(by_account)
}

fn derive_legacy_quota_record(family: &AccountPoolQuotaFamilyRecord) -> AccountPoolQuotaRecord {
    let chosen_used_window = [
        (
            family.primary.used_percent,
            family.primary.resets_at,
            QuotaWindowKind::Primary,
        ),
        (
            family.secondary.used_percent,
            family.secondary.resets_at,
            QuotaWindowKind::Secondary,
        ),
    ]
    .into_iter()
    .filter_map(|(used_percent, resets_at, kind)| {
        used_percent.map(|used_percent| {
            let remaining_percent = (100.0 - used_percent).clamp(0.0, 100.0);
            (remaining_percent, resets_at, kind)
        })
    })
    .min_by(|left, right| {
        left.0
            .partial_cmp(&right.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.2.cmp(&right.2))
    });

    match chosen_used_window {
        Some((remaining_percent, resets_at, _)) => AccountPoolQuotaRecord {
            remaining_percent: Some(remaining_percent),
            resets_at,
            observed_at: family.observed_at,
        },
        None => AccountPoolQuotaRecord {
            remaining_percent: None,
            resets_at: match family.exhausted_windows.as_str() {
                "primary" => family.primary.resets_at,
                "secondary" => family.secondary.resets_at,
                "both" => match (family.primary.resets_at, family.secondary.resets_at) {
                    (Some(primary), Some(secondary)) => Some(primary.min(secondary)),
                    (Some(primary), None) => Some(primary),
                    (None, Some(secondary)) => Some(secondary),
                    (None, None) => None,
                },
                "none" | "unknown" => None,
                _ => None,
            },
            observed_at: family.observed_at,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum QuotaWindowKind {
    Primary,
    Secondary,
}

fn normalize_page_limit(limit: Option<u32>, default_limit: u32, max_limit: u32) -> u32 {
    limit.unwrap_or(default_limit).max(1).min(max_limit)
}

fn derive_account_operational_state(
    enabled: bool,
    health_state: Option<&str>,
    quota_exhausted: bool,
    has_active_lease: bool,
) -> Option<&'static str> {
    match health_state {
        Some("unauthorized") => Some("error"),
        _ if quota_exhausted && !has_active_lease => Some("coolingDown"),
        _ if has_active_lease => Some("leased"),
        _ if enabled => Some("available"),
        _ => None,
    }
}

fn derive_account_compat_health_state(
    legacy_health_state: Option<&str>,
    quota_exhausted: bool,
) -> Option<&'static str> {
    if legacy_health_state == Some("unauthorized") {
        Some("unauthorized")
    } else if quota_exhausted {
        Some("rate_limited")
    } else if legacy_health_state == Some("healthy") {
        Some("healthy")
    } else {
        None
    }
}

fn selector_auth_eligible(_healthy: bool, health_state: Option<&str>) -> bool {
    match health_state {
        Some("unauthorized") => false,
        Some("healthy" | "rate_limited") => true,
        _ => true,
    }
}

fn quota_exhausted_windows_is_exhausted(exhausted_windows: Option<&str>) -> bool {
    exhausted_windows.is_some_and(|exhausted_windows| exhausted_windows != "none")
}

fn matches_account_state_filter(operational_state: Option<&str>, states: &[String]) -> bool {
    operational_state
        .is_some_and(|operational_state| states.iter().any(|state| state == operational_state))
}

fn encode_account_cursor(position: i64, account_id: String) -> String {
    format!("{position}:{account_id}")
}

fn decode_account_cursor(cursor: &str) -> anyhow::Result<(i64, String)> {
    let Some((position, account_id)) = cursor.split_once(':') else {
        anyhow::bail!("invalid account cursor");
    };
    Ok((position.parse()?, account_id.to_string()))
}

fn encode_event_cursor(cursor: AccountPoolEventsCursor) -> String {
    format!("{}:{}", cursor.occurred_at, cursor.event_id)
}

fn decode_event_cursor(cursor: &str) -> anyhow::Result<AccountPoolEventsCursor> {
    let Some((occurred_at, event_id)) = cursor.split_once(':') else {
        anyhow::bail!("invalid account pool event cursor");
    };
    Ok(AccountPoolEventsCursor {
        occurred_at: occurred_at.parse()?,
        event_id: event_id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::StateRuntime;
    use super::super::test_support::unique_temp_dir;
    use crate::AccountPoolEventRecord;
    use crate::AccountPoolEventsListQuery;
    use chrono::DateTime;
    use chrono::Utc;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn list_account_pool_events_returns_descending_cursor_page() {
        let runtime = test_runtime().await;
        runtime
            .append_account_pool_event(test_event(
                "evt-1",
                100,
                "team-main",
                Some("acct-1"),
                "leaseAcquired",
            ))
            .await
            .unwrap();
        runtime
            .append_account_pool_event(test_event(
                "evt-2",
                200,
                "team-main",
                Some("acct-1"),
                "leaseReleased",
            ))
            .await
            .unwrap();

        let first = runtime
            .list_account_pool_events(AccountPoolEventsListQuery {
                pool_id: "team-main".to_string(),
                account_id: None,
                types: None,
                cursor: None,
                limit: Some(1),
            })
            .await
            .unwrap();

        assert_eq!(first.data.len(), 1);
        assert_eq!(first.data[0].event_id, "evt-2");
        assert_eq!(first.next_cursor.is_some(), true);

        let second = runtime
            .list_account_pool_events(AccountPoolEventsListQuery {
                pool_id: "team-main".to_string(),
                account_id: None,
                types: None,
                cursor: first.next_cursor,
                limit: Some(1),
            })
            .await
            .unwrap();

        assert_eq!(second.data.len(), 1);
        assert_eq!(second.data[0].event_id, "evt-1");
        assert_eq!(second.next_cursor, None);
    }

    #[tokio::test]
    async fn read_account_pool_snapshot_leaves_unknown_counts_null() {
        let runtime = test_runtime().await;
        seed_account(&runtime, "acct-1", "team-main", 0).await;

        let snapshot = runtime
            .read_account_pool_snapshot("team-main")
            .await
            .unwrap();

        assert_eq!(snapshot.pool_id, "team-main");
        assert_eq!(snapshot.summary.total_accounts, 1);
        assert_eq!(snapshot.summary.active_leases, 0);
        assert_eq!(snapshot.summary.available_accounts, Some(1));
        assert_eq!(snapshot.summary.leased_accounts, Some(0));
        assert_eq!(snapshot.summary.paused_accounts, None);
        assert_eq!(snapshot.summary.draining_accounts, None);
        assert_eq!(snapshot.summary.near_exhausted_accounts, None);
        assert_eq!(snapshot.summary.exhausted_accounts, None);
        assert_eq!(snapshot.summary.error_accounts, None);
    }

    #[tokio::test]
    async fn read_account_pool_diagnostics_uses_error_severity_and_blocked_status() {
        let runtime = test_runtime().await;
        seed_account(&runtime, "acct-1", "team-main", 0).await;
        runtime
            .record_account_health_event(crate::AccountHealthEvent {
                account_id: "acct-1".to_string(),
                pool_id: "team-main".to_string(),
                health_state: crate::AccountHealthState::Unauthorized,
                sequence_number: 1,
                observed_at: timestamp(10),
            })
            .await
            .unwrap();

        let diagnostics = runtime
            .read_account_pool_diagnostics("team-main")
            .await
            .unwrap();

        assert_eq!(diagnostics.status, "blocked");
        assert_eq!(
            diagnostics
                .issues
                .iter()
                .find(|issue| issue.reason_code == "authFailure")
                .map(|issue| issue.severity.as_str()),
            Some("error")
        );
        assert!(
            diagnostics
                .issues
                .iter()
                .all(|issue| matches!(issue.severity.as_str(), "info" | "warning" | "error"))
        );
    }

    #[tokio::test]
    async fn read_account_pool_diagnostics_keeps_active_lease_pool_degraded() {
        let runtime = test_runtime().await;
        seed_account(&runtime, "acct-1", "team-main", 0).await;
        seed_account(&runtime, "acct-2", "team-main", 1).await;
        runtime
            .acquire_account_lease("team-main", "inst-a", chrono::Duration::seconds(300))
            .await
            .unwrap();
        runtime
            .record_account_health_event(crate::AccountHealthEvent {
                account_id: "acct-2".to_string(),
                pool_id: "team-main".to_string(),
                health_state: crate::AccountHealthState::Unauthorized,
                sequence_number: 1,
                observed_at: timestamp(11),
            })
            .await
            .unwrap();

        let diagnostics = runtime
            .read_account_pool_diagnostics("team-main")
            .await
            .unwrap();

        assert_eq!(diagnostics.status, "degraded");
        assert_eq!(
            diagnostics
                .issues
                .iter()
                .find(|issue| issue.reason_code == "authFailure")
                .map(|issue| issue.severity.as_str()),
            Some("error")
        );
    }

    #[tokio::test]
    async fn read_account_pool_diagnostics_keeps_single_healthy_active_lease_pool_healthy() {
        let runtime = test_runtime().await;
        seed_account(&runtime, "acct-1", "team-main", 0).await;
        runtime
            .acquire_account_lease("team-main", "inst-a", chrono::Duration::seconds(300))
            .await
            .unwrap();

        let diagnostics = runtime
            .read_account_pool_diagnostics("team-main")
            .await
            .unwrap();

        assert_eq!(diagnostics.status, "healthy");
        assert!(diagnostics.issues.is_empty());
    }

    #[tokio::test]
    async fn read_account_pool_diagnostics_blocks_unauthorized_only_active_lease() {
        let runtime = test_runtime().await;
        seed_account(&runtime, "acct-1", "team-main", 0).await;
        runtime
            .acquire_account_lease("team-main", "inst-a", chrono::Duration::seconds(300))
            .await
            .unwrap();
        runtime
            .record_account_health_event(crate::AccountHealthEvent {
                account_id: "acct-1".to_string(),
                pool_id: "team-main".to_string(),
                health_state: crate::AccountHealthState::Unauthorized,
                sequence_number: 1,
                observed_at: timestamp(12),
            })
            .await
            .unwrap();

        let diagnostics = runtime
            .read_account_pool_diagnostics("team-main")
            .await
            .unwrap();

        assert_eq!(diagnostics.status, "blocked");
    }

    #[tokio::test]
    async fn read_account_pool_diagnostics_keeps_suppressed_pool_degraded() {
        let runtime = test_runtime().await;
        seed_account(&runtime, "acct-1", "team-main", 0).await;
        runtime
            .write_account_startup_selection(crate::AccountStartupSelectionUpdate {
                default_pool_id: Some("team-main".to_string()),
                preferred_account_id: Some("acct-1".to_string()),
                suppressed: true,
            })
            .await
            .unwrap();

        let diagnostics = runtime
            .read_account_pool_diagnostics("team-main")
            .await
            .unwrap();

        assert_eq!(diagnostics.status, "degraded");
        assert_eq!(
            diagnostics
                .issues
                .iter()
                .find(|issue| issue.reason_code == "durablySuppressed")
                .map(|issue| issue.severity.as_str()),
            Some("warning")
        );
        assert!(
            diagnostics
                .issues
                .iter()
                .all(|issue| issue.reason_code != "noEligibleAccount")
        );
    }

    #[tokio::test]
    async fn list_account_pool_accounts_filters_by_operational_state() {
        let runtime = test_runtime().await;
        seed_account(&runtime, "acct-1", "team-main", 0).await;
        seed_account(&runtime, "acct-2", "team-main", 1).await;
        runtime
            .acquire_account_lease("team-main", "inst-a", chrono::Duration::seconds(300))
            .await
            .unwrap();

        let accounts = runtime
            .list_account_pool_accounts(crate::AccountPoolAccountsListQuery {
                pool_id: "team-main".to_string(),
                account_id: None,
                cursor: None,
                limit: Some(10),
                states: Some(vec!["leased".to_string()]),
                account_kinds: None,
            })
            .await
            .unwrap();

        assert_eq!(accounts.next_cursor, None);
        assert_eq!(accounts.data.len(), 1);
        assert_eq!(accounts.data[0].account_id, "acct-1");
        assert_eq!(
            accounts.data[0].operational_state.as_deref(),
            Some("leased")
        );
    }

    #[tokio::test]
    async fn list_account_pool_accounts_projects_cooling_down_from_quota_rows() {
        let runtime = test_runtime().await;
        seed_account(&runtime, "acct-1", "team-main", 0).await;
        runtime
            .upsert_account_quota_state(crate::AccountQuotaStateRecord {
                account_id: "acct-1".to_string(),
                limit_id: "codex".to_string(),
                primary_used_percent: Some(98.0),
                primary_resets_at: Some(timestamp(120)),
                secondary_used_percent: None,
                secondary_resets_at: None,
                observed_at: timestamp(60),
                exhausted_windows: crate::QuotaExhaustedWindows::Primary,
                predicted_blocked_until: Some(timestamp(120)),
                next_probe_after: Some(timestamp(90)),
                probe_backoff_level: 0,
                last_probe_result: None,
            })
            .await
            .unwrap();

        let accounts = runtime
            .list_account_pool_accounts(crate::AccountPoolAccountsListQuery {
                pool_id: "team-main".to_string(),
                account_id: None,
                cursor: None,
                limit: Some(10),
                states: Some(vec!["coolingDown".to_string()]),
                account_kinds: None,
            })
            .await
            .unwrap();

        assert_eq!(accounts.next_cursor, None);
        assert_eq!(accounts.data.len(), 1);
        assert_eq!(accounts.data[0].account_id, "acct-1");
        assert_eq!(
            accounts.data[0].health_state.as_deref(),
            Some("rate_limited")
        );
        assert_eq!(
            accounts.data[0].operational_state.as_deref(),
            Some("coolingDown")
        );
        assert_eq!(
            accounts.data[0]
                .selection
                .as_ref()
                .map(|selection| selection.eligible),
            Some(false)
        );
        assert_eq!(
            accounts.data[0]
                .selection
                .as_ref()
                .and_then(|selection| selection.next_eligible_at),
            Some(timestamp(120))
        );
    }

    #[tokio::test]
    async fn list_account_pool_accounts_uses_backend_family_quota_rows() {
        let runtime = test_runtime().await;
        seed_account(&runtime, "acct-1", "team-main", 0).await;
        runtime
            .upsert_account_quota_state(crate::AccountQuotaStateRecord {
                account_id: "acct-1".to_string(),
                limit_id: "chatgpt".to_string(),
                primary_used_percent: Some(98.0),
                primary_resets_at: Some(timestamp(120)),
                secondary_used_percent: None,
                secondary_resets_at: None,
                observed_at: timestamp(60),
                exhausted_windows: crate::QuotaExhaustedWindows::Primary,
                predicted_blocked_until: Some(timestamp(120)),
                next_probe_after: Some(timestamp(90)),
                probe_backoff_level: 0,
                last_probe_result: None,
            })
            .await
            .unwrap();

        let accounts = runtime
            .list_account_pool_accounts(crate::AccountPoolAccountsListQuery {
                pool_id: "team-main".to_string(),
                account_id: None,
                cursor: None,
                limit: Some(10),
                states: Some(vec!["coolingDown".to_string()]),
                account_kinds: None,
            })
            .await
            .unwrap();

        assert_eq!(accounts.next_cursor, None);
        assert_eq!(accounts.data.len(), 1);
        assert_eq!(accounts.data[0].account_id, "acct-1");
        assert_eq!(
            accounts.data[0].operational_state.as_deref(),
            Some("coolingDown")
        );
    }

    #[tokio::test]
    async fn list_account_pool_accounts_does_not_splice_codex_timing_into_family_row() {
        let runtime = test_runtime().await;
        seed_account(&runtime, "acct-1", "team-main", 0).await;
        runtime
            .upsert_account_quota_state(crate::AccountQuotaStateRecord {
                account_id: "acct-1".to_string(),
                limit_id: "codex".to_string(),
                primary_used_percent: Some(98.0),
                primary_resets_at: Some(timestamp(180)),
                secondary_used_percent: None,
                secondary_resets_at: None,
                observed_at: timestamp(60),
                exhausted_windows: crate::QuotaExhaustedWindows::Primary,
                predicted_blocked_until: Some(timestamp(180)),
                next_probe_after: Some(timestamp(150)),
                probe_backoff_level: 0,
                last_probe_result: None,
            })
            .await
            .unwrap();
        runtime
            .upsert_account_quota_state(crate::AccountQuotaStateRecord {
                account_id: "acct-1".to_string(),
                limit_id: "chatgpt".to_string(),
                primary_used_percent: Some(96.0),
                primary_resets_at: Some(timestamp(240)),
                secondary_used_percent: None,
                secondary_resets_at: None,
                observed_at: timestamp(120),
                exhausted_windows: crate::QuotaExhaustedWindows::Unknown,
                predicted_blocked_until: None,
                next_probe_after: None,
                probe_backoff_level: 0,
                last_probe_result: None,
            })
            .await
            .unwrap();

        let accounts = runtime
            .list_account_pool_accounts(crate::AccountPoolAccountsListQuery {
                pool_id: "team-main".to_string(),
                account_id: None,
                cursor: None,
                limit: Some(10),
                states: Some(vec!["coolingDown".to_string()]),
                account_kinds: None,
            })
            .await
            .unwrap();

        assert_eq!(accounts.next_cursor, None);
        assert_eq!(accounts.data.len(), 1);
        assert_eq!(accounts.data[0].account_id, "acct-1");
        assert_eq!(
            accounts.data[0]
                .selection
                .as_ref()
                .and_then(|selection| selection.next_eligible_at),
            None
        );
    }

    #[tokio::test]
    async fn diagnostics_and_snapshot_project_cooldown_from_quota_rows() {
        let runtime = test_runtime().await;
        seed_account(&runtime, "acct-1", "team-main", 0).await;
        runtime
            .upsert_account_quota_state(crate::AccountQuotaStateRecord {
                account_id: "acct-1".to_string(),
                limit_id: "codex".to_string(),
                primary_used_percent: Some(99.0),
                primary_resets_at: Some(timestamp(180)),
                secondary_used_percent: None,
                secondary_resets_at: None,
                observed_at: timestamp(100),
                exhausted_windows: crate::QuotaExhaustedWindows::Unknown,
                predicted_blocked_until: Some(timestamp(180)),
                next_probe_after: Some(timestamp(150)),
                probe_backoff_level: 0,
                last_probe_result: Some(crate::QuotaProbeResult::StillBlocked),
            })
            .await
            .unwrap();

        let snapshot = runtime
            .read_account_pool_snapshot("team-main")
            .await
            .unwrap();
        let diagnostics = runtime
            .read_account_pool_diagnostics("team-main")
            .await
            .unwrap();

        assert_eq!(snapshot.summary.available_accounts, Some(0));
        assert_eq!(diagnostics.status, "blocked");
        assert_eq!(
            diagnostics
                .issues
                .iter()
                .find(|issue| issue.reason_code == "cooldownActive")
                .map(|issue| issue.severity.as_str()),
            Some("warning")
        );
    }

    #[tokio::test]
    async fn snapshot_uses_backend_family_quota_rows() {
        let runtime = test_runtime().await;
        seed_account(&runtime, "acct-1", "team-main", 0).await;
        runtime
            .upsert_account_quota_state(crate::AccountQuotaStateRecord {
                account_id: "acct-1".to_string(),
                limit_id: "chatgpt".to_string(),
                primary_used_percent: Some(99.0),
                primary_resets_at: Some(timestamp(180)),
                secondary_used_percent: None,
                secondary_resets_at: None,
                observed_at: timestamp(100),
                exhausted_windows: crate::QuotaExhaustedWindows::Unknown,
                predicted_blocked_until: Some(timestamp(180)),
                next_probe_after: Some(timestamp(150)),
                probe_backoff_level: 0,
                last_probe_result: Some(crate::QuotaProbeResult::StillBlocked),
            })
            .await
            .unwrap();

        let snapshot = runtime
            .read_account_pool_snapshot("team-main")
            .await
            .unwrap();

        assert_eq!(snapshot.summary.available_accounts, Some(0));
    }

    #[tokio::test]
    async fn list_account_pool_accounts_paginates_without_skipping_rows() {
        let runtime = test_runtime().await;
        seed_account(&runtime, "acct-1", "team-main", 0).await;
        seed_account(&runtime, "acct-2", "team-main", 1).await;
        seed_account(&runtime, "acct-3", "team-main", 2).await;

        let first = runtime
            .list_account_pool_accounts(crate::AccountPoolAccountsListQuery {
                pool_id: "team-main".to_string(),
                account_id: None,
                cursor: None,
                limit: Some(1),
                states: None,
                account_kinds: None,
            })
            .await
            .unwrap();
        let second = runtime
            .list_account_pool_accounts(crate::AccountPoolAccountsListQuery {
                pool_id: "team-main".to_string(),
                account_id: None,
                cursor: first.next_cursor.clone(),
                limit: Some(1),
                states: None,
                account_kinds: None,
            })
            .await
            .unwrap();

        assert_eq!(first.data.len(), 1);
        assert_eq!(first.data[0].account_id, "acct-1");
        assert_eq!(second.data.len(), 1);
        assert_eq!(second.data[0].account_id, "acct-2");
        assert_ne!(first.next_cursor, second.next_cursor);
    }

    async fn test_runtime() -> std::sync::Arc<StateRuntime> {
        let codex_home = unique_temp_dir();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        StateRuntime::init(codex_home, "test-provider".to_string())
            .await
            .expect("initialize runtime")
    }

    async fn seed_account(runtime: &StateRuntime, account_id: &str, pool_id: &str, position: i64) {
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
        .bind(account_id)
        .bind(format!("legacy:chatgpt:workspace-main:{account_id}"))
        .bind(1_i64)
        .bind(1_i64)
        .bind(1_i64)
        .bind(1_i64)
        .execute(runtime.pool.as_ref())
        .await
        .expect("seed account registry");

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

    fn test_event(
        event_id: &str,
        occurred_at: i64,
        pool_id: &str,
        account_id: Option<&str>,
        event_type: &str,
    ) -> AccountPoolEventRecord {
        AccountPoolEventRecord {
            event_id: event_id.to_string(),
            occurred_at: timestamp(occurred_at),
            pool_id: pool_id.to_string(),
            account_id: account_id.map(ToOwned::to_owned),
            lease_id: None,
            holder_instance_id: None,
            event_type: event_type.to_string(),
            reason_code: None,
            message: format!("event {event_id}"),
            details_json: None,
        }
    }

    fn timestamp(seconds: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(seconds, 0).expect("timestamp")
    }
}
