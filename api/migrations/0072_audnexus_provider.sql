-- Add Audnexus metadata provider for audiobook enrichment
INSERT INTO metadata_providers (provider_type, priority, enabled, config)
VALUES ('audnexus', 40, TRUE, '{}')
ON CONFLICT (provider_type) DO NOTHING;
