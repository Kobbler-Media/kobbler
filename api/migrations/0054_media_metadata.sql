-- Media metadata: video metadata columns on file tables, track tables,
-- external subtitles, and subtitle provider configuration.

--------------------------------------------------------------------------------
-- Video metadata columns on movie_files
--------------------------------------------------------------------------------

ALTER TABLE movie_files
    ADD COLUMN video_codec TEXT,
    ADD COLUMN video_width INTEGER,
    ADD COLUMN video_height INTEGER,
    ADD COLUMN video_bitrate_kbps INTEGER,
    ADD COLUMN framerate DOUBLE PRECISION,
    ADD COLUMN bit_depth SMALLINT,
    ADD COLUMN is_hdr BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN hdr_format TEXT,
    ADD COLUMN is_interlaced BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN duration_seconds DOUBLE PRECISION,
    ADD COLUMN container TEXT,
    ADD COLUMN overall_bitrate_kbps INTEGER;

CREATE INDEX idx_movie_files_video_codec ON movie_files (video_codec) WHERE video_codec IS NOT NULL;
CREATE INDEX idx_movie_files_is_hdr ON movie_files (is_hdr) WHERE is_hdr = TRUE;

--------------------------------------------------------------------------------
-- Video metadata columns on tv_episode_files
--------------------------------------------------------------------------------

ALTER TABLE tv_episode_files
    ADD COLUMN video_codec TEXT,
    ADD COLUMN video_width INTEGER,
    ADD COLUMN video_height INTEGER,
    ADD COLUMN video_bitrate_kbps INTEGER,
    ADD COLUMN framerate DOUBLE PRECISION,
    ADD COLUMN bit_depth SMALLINT,
    ADD COLUMN is_hdr BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN hdr_format TEXT,
    ADD COLUMN is_interlaced BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN duration_seconds DOUBLE PRECISION,
    ADD COLUMN container TEXT,
    ADD COLUMN overall_bitrate_kbps INTEGER;

CREATE INDEX idx_episode_files_video_codec ON tv_episode_files (video_codec) WHERE video_codec IS NOT NULL;
CREATE INDEX idx_episode_files_is_hdr ON tv_episode_files (is_hdr) WHERE is_hdr = TRUE;

--------------------------------------------------------------------------------
-- audio_tracks - Embedded audio tracks from container metadata
--------------------------------------------------------------------------------

CREATE TABLE audio_tracks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    movie_file_id UUID REFERENCES movie_files(id) ON DELETE CASCADE,
    episode_file_id UUID REFERENCES tv_episode_files(id) ON DELETE CASCADE,
    track_index INTEGER NOT NULL,
    codec TEXT NOT NULL,
    channels SMALLINT NOT NULL,
    bitrate_kbps INTEGER,
    sample_rate INTEGER,
    language TEXT,
    title TEXT,
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    CONSTRAINT audio_tracks_exactly_one_parent CHECK (
        (movie_file_id IS NOT NULL AND episode_file_id IS NULL) OR
        (movie_file_id IS NULL AND episode_file_id IS NOT NULL)
    )
);

CREATE INDEX idx_audio_tracks_movie_file ON audio_tracks (movie_file_id) WHERE movie_file_id IS NOT NULL;
CREATE INDEX idx_audio_tracks_episode_file ON audio_tracks (episode_file_id) WHERE episode_file_id IS NOT NULL;

--------------------------------------------------------------------------------
-- subtitle_tracks - Embedded subtitle tracks from container metadata
--------------------------------------------------------------------------------

CREATE TABLE subtitle_tracks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    movie_file_id UUID REFERENCES movie_files(id) ON DELETE CASCADE,
    episode_file_id UUID REFERENCES tv_episode_files(id) ON DELETE CASCADE,
    track_index INTEGER NOT NULL,
    codec TEXT NOT NULL,
    language TEXT,
    title TEXT,
    is_forced BOOLEAN NOT NULL DEFAULT FALSE,
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    is_hearing_impaired BOOLEAN NOT NULL DEFAULT FALSE,
    CONSTRAINT subtitle_tracks_exactly_one_parent CHECK (
        (movie_file_id IS NOT NULL AND episode_file_id IS NULL) OR
        (movie_file_id IS NULL AND episode_file_id IS NOT NULL)
    )
);

CREATE INDEX idx_subtitle_tracks_movie_file ON subtitle_tracks (movie_file_id) WHERE movie_file_id IS NOT NULL;
CREATE INDEX idx_subtitle_tracks_episode_file ON subtitle_tracks (episode_file_id) WHERE episode_file_id IS NOT NULL;
CREATE INDEX idx_subtitle_tracks_forced ON subtitle_tracks (is_forced) WHERE is_forced = TRUE;

--------------------------------------------------------------------------------
-- external_subtitles - Sidecar subtitle files (on-disk or provider-downloaded)
--------------------------------------------------------------------------------

CREATE TABLE external_subtitles (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    movie_file_id UUID REFERENCES movie_files(id) ON DELETE CASCADE,
    episode_file_id UUID REFERENCES tv_episode_files(id) ON DELETE CASCADE,
    file_path TEXT NOT NULL,
    file_size BIGINT,
    format TEXT NOT NULL,
    language TEXT,
    title TEXT,
    is_forced BOOLEAN NOT NULL DEFAULT FALSE,
    is_hearing_impaired BOOLEAN NOT NULL DEFAULT FALSE,
    source TEXT NOT NULL DEFAULT 'local',
    source_id TEXT,
    source_metadata JSONB,
    available BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT external_subtitles_exactly_one_parent CHECK (
        (movie_file_id IS NOT NULL AND episode_file_id IS NULL) OR
        (movie_file_id IS NULL AND episode_file_id IS NOT NULL)
    )
);

CREATE INDEX idx_external_subtitles_movie_file ON external_subtitles (movie_file_id) WHERE movie_file_id IS NOT NULL;
CREATE INDEX idx_external_subtitles_episode_file ON external_subtitles (episode_file_id) WHERE episode_file_id IS NOT NULL;
CREATE UNIQUE INDEX idx_external_subtitles_path ON external_subtitles (file_path);

--------------------------------------------------------------------------------
-- subtitle_providers - Provider configuration (mirrors video_metadata_providers)
--------------------------------------------------------------------------------

CREATE TABLE subtitle_providers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider_type TEXT NOT NULL UNIQUE,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    priority INTEGER NOT NULL DEFAULT 100,
    config JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Seed OpenSubtitles provider (disabled by default)
INSERT INTO subtitle_providers (provider_type, enabled, priority, config)
VALUES ('opensubtitles', FALSE, 100, '{}');
