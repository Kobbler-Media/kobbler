-- Migrate video sync providers from global table to per-user user_sync_providers.
-- Only migrates providers that were actually configured (non-empty config).

INSERT INTO user_sync_providers (user_id, provider_type, enabled, config)
SELECT u.id, vsp.provider_type, vsp.enabled, vsp.config
FROM video_sync_providers vsp
CROSS JOIN users u
WHERE vsp.config != '{}'::jsonb
ON CONFLICT (user_id, provider_type) DO NOTHING;

DROP TABLE video_sync_providers;
