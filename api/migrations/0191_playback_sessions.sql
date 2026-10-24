-- What it takes to rebuild an in-memory playback session after the API restarts under
-- it: the request that opened it and how far it had got. Rows live as long as the
-- session would have — the reaper deletes them with the session, and a sweep catches
-- the ones a restart orphaned.
CREATE TABLE playback_sessions (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL,
    dir TEXT NOT NULL,
    request JSONB NOT NULL,
    progress JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_activity_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_playback_sessions_last_activity ON playback_sessions (last_activity_at);
