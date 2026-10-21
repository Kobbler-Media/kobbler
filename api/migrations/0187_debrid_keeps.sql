-- "Keep in library" for a debrid stream: the job that finishes the download and imports
-- the file, and how far it has got. One per session; a failed keep is retried in place.
CREATE TABLE debrid_keeps (
    session_id UUID PRIMARY KEY REFERENCES debrid_stream_sessions(id) ON DELETE CASCADE,
    job_id UUID,
    state TEXT NOT NULL DEFAULT 'queued'
        CHECK (state IN ('queued', 'downloading', 'importing', 'done', 'failed')),
    progress DOUBLE PRECISION NOT NULL DEFAULT 0,
    error TEXT,
    file_path TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
