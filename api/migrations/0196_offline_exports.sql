-- Single-file MP4 renditions a client downloads for offline playback. Regenerable cache:
-- kept out of backups, and swept by offline_export_gc once nobody has asked for one in a while.
CREATE TABLE offline_exports (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    media_type TEXT NOT NULL CHECK (media_type IN ('movie', 'episode')),
    file_id UUID NOT NULL,
    quality TEXT NOT NULL,
    audio_stream_index INT NOT NULL,
    state TEXT NOT NULL DEFAULT 'queued' CHECK (state IN ('queued', 'running', 'ready', 'failed')),
    progress DOUBLE PRECISION NOT NULL DEFAULT 0,
    size_bytes BIGINT,
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_accessed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (file_id, quality, audio_stream_index)
);

CREATE INDEX idx_offline_exports_last_accessed ON offline_exports (last_accessed_at);
