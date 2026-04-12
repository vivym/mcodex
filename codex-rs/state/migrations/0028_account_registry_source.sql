ALTER TABLE account_registry
ADD COLUMN source TEXT;

UPDATE account_registry
SET source = 'migrated'
WHERE source IS NULL
  AND pool_id = 'legacy-default';
