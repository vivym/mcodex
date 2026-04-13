ALTER TABLE account_registry
ADD COLUMN backend_id TEXT NOT NULL DEFAULT 'local';

ALTER TABLE account_registry
ADD COLUMN backend_account_handle TEXT;

ALTER TABLE account_registry
ADD COLUMN provider_fingerprint TEXT;

ALTER TABLE account_registry
ADD COLUMN display_name TEXT;

CREATE TABLE account_pool_membership (
    account_id TEXT PRIMARY KEY,
    pool_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    assigned_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(account_id) REFERENCES account_registry(account_id) ON DELETE CASCADE
);

INSERT INTO account_pool_membership (
    account_id,
    pool_id,
    position,
    assigned_at,
    updated_at
)
SELECT
    account_id,
    pool_id,
    position,
    created_at,
    updated_at
FROM account_registry;

CREATE TABLE pending_account_registration (
    idempotency_key TEXT PRIMARY KEY,
    backend_id TEXT NOT NULL,
    provider_kind TEXT NOT NULL,
    target_pool_id TEXT,
    backend_account_handle TEXT,
    account_id TEXT,
    started_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE account_compat_migration_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    legacy_import_completed INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

INSERT INTO account_compat_migration_state (
    singleton,
    legacy_import_completed,
    updated_at
) VALUES (1, 0, unixepoch('now'));

CREATE UNIQUE INDEX account_registry_backend_fingerprint_idx
ON account_registry(backend_id, provider_fingerprint);

CREATE UNIQUE INDEX account_registry_backend_handle_idx
ON account_registry(backend_id, backend_account_handle);
