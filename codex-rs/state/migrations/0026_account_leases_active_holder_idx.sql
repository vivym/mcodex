CREATE UNIQUE INDEX account_leases_active_holder_idx
ON account_leases(holder_instance_id)
WHERE released_at IS NULL;
