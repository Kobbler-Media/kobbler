-- Add TheTVDB as a video metadata provider (disabled by default, needs API key)
INSERT INTO video_metadata_providers (provider_type, enabled, priority)
VALUES ('tvdb', false, 2)
ON CONFLICT (provider_type) DO NOTHING;
