CREATE TABLE account_quota_state (
    account_id TEXT NOT NULL,
    limit_id TEXT NOT NULL,
    primary_used_percent REAL,
    primary_resets_at INTEGER,
    secondary_used_percent REAL,
    secondary_resets_at INTEGER,
    observed_at INTEGER NOT NULL,
    exhausted_windows TEXT NOT NULL,
    predicted_blocked_until INTEGER,
    next_probe_after INTEGER,
    probe_backoff_level INTEGER NOT NULL,
    last_probe_result TEXT,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(account_id, limit_id),
    FOREIGN KEY(account_id) REFERENCES account_registry(account_id) ON DELETE CASCADE
);
