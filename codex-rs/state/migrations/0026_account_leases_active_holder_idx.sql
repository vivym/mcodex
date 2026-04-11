WITH ranked_active_leases AS (
    SELECT
        lease_id,
        ROW_NUMBER() OVER (
            PARTITION BY holder_instance_id
            ORDER BY acquired_at DESC, lease_id DESC
        ) AS row_num
    FROM account_leases
    WHERE released_at IS NULL
)
UPDATE account_leases
SET released_at = unixepoch('now')
WHERE lease_id IN (
    SELECT lease_id
    FROM ranked_active_leases
    WHERE row_num > 1
);

CREATE UNIQUE INDEX account_leases_active_holder_idx
ON account_leases(holder_instance_id)
WHERE released_at IS NULL;
