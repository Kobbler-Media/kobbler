CREATE TABLE activity_log (
    id UUID PRIMARY KEY,
    event_kind TEXT NOT NULL,
    entity_type TEXT,
    entity_id UUID,
    title TEXT NOT NULL,
    subtitle TEXT,
    image_url TEXT,
    url TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_activity_log_created_at ON activity_log (created_at DESC, id DESC);
