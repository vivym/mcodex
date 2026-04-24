CREATE TABLE thread_config_baselines (
    thread_id TEXT PRIMARY KEY REFERENCES threads(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    model_provider_id TEXT NOT NULL,
    service_tier TEXT,
    approval_policy TEXT NOT NULL,
    approvals_reviewer TEXT NOT NULL,
    sandbox_policy TEXT NOT NULL,
    cwd TEXT NOT NULL,
    reasoning_effort TEXT,
    personality TEXT,
    base_instructions TEXT,
    developer_instructions TEXT
);
