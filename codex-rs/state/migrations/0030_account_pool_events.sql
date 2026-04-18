CREATE TABLE account_pool_events (
    event_id TEXT PRIMARY KEY,
    occurred_at INTEGER NOT NULL,
    pool_id TEXT NOT NULL,
    account_id TEXT,
    lease_id TEXT,
    holder_instance_id TEXT,
    event_type TEXT NOT NULL,
    reason_code TEXT,
    message TEXT NOT NULL,
    details_json TEXT
);

CREATE INDEX account_pool_events_pool_occurred_idx
ON account_pool_events(pool_id, occurred_at DESC, event_id DESC);

CREATE INDEX account_pool_events_account_occurred_idx
ON account_pool_events(account_id, occurred_at DESC, event_id DESC);
