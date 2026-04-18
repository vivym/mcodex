ALTER TABLE account_quota_state
ADD COLUMN observed_at_nanos INTEGER;

UPDATE account_quota_state
SET observed_at_nanos = observed_at * 1000000000
WHERE observed_at_nanos IS NULL;
