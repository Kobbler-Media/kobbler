-- Stores kobload ingest payloads that are waiting for a media root to become
-- available. Processed automatically when the media root recovers.
CREATE TABLE deferred_ingests (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    download_id UUID NOT NULL,
    media_root_id UUID NOT NULL REFERENCES media_roots(id) ON DELETE CASCADE,
    payload JSONB NOT NULL,
    response_mode TEXT NOT NULL DEFAULT 'nats',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(download_id)
);

CREATE INDEX idx_deferred_ingests_media_root_id ON deferred_ingests(media_root_id);
