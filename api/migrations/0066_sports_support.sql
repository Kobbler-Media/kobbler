-- Add sports support: content_category discriminator on tv_shows + TheSportsDB provider

-- Discriminate TV shows from sports leagues within the same table
ALTER TABLE tv_shows ADD COLUMN content_category TEXT NOT NULL DEFAULT 'tv';
ALTER TABLE tv_shows ADD COLUMN sport_metadata JSONB;
CREATE INDEX idx_tv_shows_content_category ON tv_shows (content_category);

-- Seed TheSportsDB as a video metadata provider
INSERT INTO video_metadata_providers (provider_type, enabled, priority)
VALUES ('thesportsdb', true, 50)
ON CONFLICT (provider_type) DO NOTHING;
