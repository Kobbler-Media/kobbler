-- Search recents: per-user frecency tracking for command palette
CREATE TABLE search_recents (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    entity_type TEXT NOT NULL,      -- 'book', 'author', 'series', 'page', 'command', 'section'
    entity_id TEXT NOT NULL,        -- UUID-as-text for DB entities, slug for static types
    access_count INTEGER NOT NULL DEFAULT 1,
    last_accessed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(user_id, entity_type, entity_id)
);

CREATE INDEX idx_search_recents_user ON search_recents(user_id);
