CREATE TABLE account_registry (
    account_id TEXT PRIMARY KEY,
    pool_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    account_kind TEXT NOT NULL,
    backend_family TEXT NOT NULL,
    workspace_id TEXT,
    healthy INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE account_runtime_state (
    account_id TEXT PRIMARY KEY,
    pool_id TEXT NOT NULL,
    health_state TEXT NOT NULL,
    last_health_event_sequence INTEGER NOT NULL,
    last_health_event_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(account_id) REFERENCES account_registry(account_id) ON DELETE CASCADE
);

CREATE TABLE account_startup_selection (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    default_pool_id TEXT,
    preferred_account_id TEXT,
    suppressed INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE account_leases (
    lease_id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    pool_id TEXT NOT NULL,
    holder_instance_id TEXT NOT NULL,
    lease_epoch INTEGER NOT NULL,
    acquired_at INTEGER NOT NULL,
    renewed_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    released_at INTEGER,
    FOREIGN KEY(account_id) REFERENCES account_registry(account_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX account_leases_active_account_idx
ON account_leases(account_id)
WHERE released_at IS NULL;
