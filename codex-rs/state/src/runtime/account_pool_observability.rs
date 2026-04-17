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
use crate::model::AccountPoolQuotaRecord;
use crate::model::AccountPoolSelectionRecord;
use crate::model::AccountPoolSnapshotRecord;
use crate::model::AccountPoolSummaryRecord;
use crate::model::account_datetime_to_epoch_seconds;
use crate::model::account_epoch_seconds_to_datetime;
use chrono::Utc;
use serde_json::Value;
use sqlx::Executor;
use sqlx::QueryBuilder;
use sqlx::Row;
use sqlx::Sqlite;

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
        let row = sqlx::query(
            r#"
SELECT
    COUNT(*) AS total_accounts,
    COALESCE(SUM(CASE WHEN active_lease.lease_id IS NOT NULL THEN 1 ELSE 0 END), 0) AS active_leases,
    COALESCE(SUM(CASE
        WHEN account_registry.enabled = 1
          AND account_registry.healthy = 1
          AND active_lease.lease_id IS NULL
            THEN 1
        ELSE 0
    END), 0) AS available_accounts,
    COALESCE(SUM(CASE WHEN active_lease.lease_id IS NOT NULL THEN 1 ELSE 0 END), 0) AS leased_accounts
FROM account_pool_membership AS membership
JOIN account_registry
  ON account_registry.account_id = membership.account_id
LEFT JOIN account_leases AS active_lease
  ON active_lease.account_id = membership.account_id
 AND active_lease.pool_id = membership.pool_id
 AND active_lease.released_at IS NULL
 AND active_lease.expires_at > ?
WHERE membership.pool_id = ?
            "#,
        )
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
        if request
            .states
            .as_ref()
            .is_some_and(|states| !states.is_empty())
        {
            return Ok(AccountPoolAccountsPage {
                data: Vec::new(),
                next_cursor: None,
            });
        }

        let selection = self.read_account_startup_selection().await?;
        let now = Utc::now();
        let limit = normalize_page_limit(
            request.limit,
            DEFAULT_ACCOUNT_PAGE_LIMIT,
            MAX_ACCOUNT_PAGE_LIMIT,
        );
        let cursor = request
            .cursor
            .as_deref()
            .map(decode_account_cursor)
            .transpose()?;

        let mut builder = QueryBuilder::<Sqlite>::new(
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
LEFT JOIN account_leases AS active_lease
  ON active_lease.account_id = membership.account_id
 AND active_lease.pool_id = membership.pool_id
 AND active_lease.released_at IS NULL
 AND active_lease.expires_at > 
            "#,
        );
        builder.push_bind(account_datetime_to_epoch_seconds(now));
        builder.push(" WHERE membership.pool_id = ");
        builder.push_bind(&request.pool_id);

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

        if let Some((position, account_id)) = cursor.as_ref() {
            builder.push(" AND (membership.position > ");
            builder.push_bind(*position);
            builder.push(" OR (membership.position = ");
            builder.push_bind(*position);
            builder.push(" AND membership.account_id > ");
            builder.push_bind(account_id);
            builder.push("))");
        }

        builder.push(" ORDER BY membership.position ASC, membership.account_id ASC LIMIT ");
        builder.push_bind(i64::from(limit) + 1);

        let rows = builder.build().fetch_all(self.pool.as_ref()).await?;
        let mut data = Vec::with_capacity(rows.len().min(limit as usize));
        for row in rows.iter().take(limit as usize) {
            let account_id: String = row.try_get("account_id")?;
            let enabled = row.try_get::<i64, _>("enabled")? != 0;
            let healthy = row.try_get::<i64, _>("healthy")? != 0;
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
            let next_eligible_at = current_lease.as_ref().map(|lease| lease.expires_at);
            data.push(AccountPoolAccountRecord {
                account_id: account_id.clone(),
                backend_account_ref: row
                    .try_get::<String, _>("backend_account_handle")
                    .ok()
                    .filter(|value| !value.is_empty()),
                account_kind: row.try_get("account_kind")?,
                enabled,
                health_state: row.try_get("health_state")?,
                operational_state: None,
                allocatable: None,
                status_reason_code: None,
                status_message: None,
                current_lease,
                quota: None::<AccountPoolQuotaRecord>,
                selection: Some(AccountPoolSelectionRecord {
                    eligible: !selection.suppressed
                        && enabled
                        && healthy
                        && next_eligible_at.is_none(),
                    next_eligible_at,
                    preferred: selection.preferred_account_id.as_deref()
                        == Some(account_id.as_str()),
                    suppressed: selection.suppressed,
                }),
                updated_at: account_epoch_seconds_to_datetime(
                    row.try_get::<i64, _>("health_updated_at")
                        .or_else(|_| row.try_get::<i64, _>("registry_updated_at"))?,
                )?,
            });
        }

        let next_cursor = if rows.len() > limit as usize {
            match rows.get(limit as usize - 1) {
                Some(row) => Some(encode_account_cursor(
                    row.try_get("position")?,
                    row.try_get::<String, _>("account_id")?,
                )),
                None => None,
            }
        } else {
            None
        };

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
        let rows = sqlx::query(
            r#"
SELECT
    membership.account_id,
    account_registry.enabled,
    account_registry.healthy,
    account_runtime_state.health_state,
    active_lease.holder_instance_id,
    active_lease.expires_at
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
ORDER BY membership.position ASC, membership.account_id ASC
            "#,
        )
        .bind(account_datetime_to_epoch_seconds(generated_at))
        .bind(pool_id)
        .fetch_all(self.pool.as_ref())
        .await?;

        let mut issues = Vec::new();
        let mut eligible_accounts = 0_u32;
        let mut next_relevant_at: Option<chrono::DateTime<Utc>> = None;
        let mut preferred_in_pool = false;
        for row in rows {
            let enabled = row.try_get::<i64, _>("enabled")? != 0;
            let healthy = row.try_get::<i64, _>("healthy")? != 0;
            let expires_at = row
                .try_get::<Option<i64>, _>("expires_at")?
                .map(account_epoch_seconds_to_datetime)
                .transpose()?;

            if enabled && healthy && expires_at.is_none() && !selection.suppressed {
                eligible_accounts += 1;
            }

            if let Some(expires_at) = expires_at {
                next_relevant_at = match next_relevant_at {
                    Some(current) => Some(current.min(expires_at)),
                    None => Some(expires_at),
                };
            }

            let account_id: String = row.try_get("account_id")?;
            if selection.preferred_account_id.as_deref() == Some(account_id.as_str()) {
                preferred_in_pool = true;
            }
            match row.try_get::<Option<String>, _>("health_state")?.as_deref() {
                Some("rate_limited") => issues.push(AccountPoolIssueRecord {
                    severity: "warning".to_string(),
                    reason_code: "cooldownActive".to_string(),
                    message: format!("account {account_id} is in cooldown"),
                    account_id: Some(account_id.clone()),
                    holder_instance_id: row.try_get("holder_instance_id")?,
                    next_relevant_at: expires_at,
                }),
                Some("unauthorized") => issues.push(AccountPoolIssueRecord {
                    severity: "critical".to_string(),
                    reason_code: "authFailure".to_string(),
                    message: format!("account {account_id} is unauthorized"),
                    account_id: Some(account_id.clone()),
                    holder_instance_id: row.try_get("holder_instance_id")?,
                    next_relevant_at: None,
                }),
                _ => {}
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

        if eligible_accounts == 0 {
            let lease_failure = recent_events
                .data
                .iter()
                .find(|event| event.event_type == "leaseAcquireFailed");
            issues.push(AccountPoolIssueRecord {
                severity: "critical".to_string(),
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

        let status = if issues.iter().any(|issue| issue.severity == "critical") {
            "unavailable"
        } else if issues.is_empty() {
            "healthy"
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

fn normalize_page_limit(limit: Option<u32>, default_limit: u32, max_limit: u32) -> u32 {
    limit.unwrap_or(default_limit).max(1).min(max_limit)
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
