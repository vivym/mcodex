ALTER TABLE thread_config_baselines
ADD COLUMN personality_overrides_rollout INTEGER NOT NULL DEFAULT 0;

ALTER TABLE thread_config_baselines
ADD COLUMN developer_instructions_overrides_rollout INTEGER NOT NULL DEFAULT 0;
