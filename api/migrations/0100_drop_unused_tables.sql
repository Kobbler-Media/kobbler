-- Drop legacy tables that are no longer used by any application code.
-- `jobs` was superseded by `job_status` (0018) + NATS JetStream.
-- `integrations` was an early concept replaced by `user_sync_providers` + `device_sync_providers`.
DROP TABLE IF EXISTS jobs;
DROP TABLE IF EXISTS integrations;
