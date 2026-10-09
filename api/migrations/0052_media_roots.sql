-- Media roots: permanent library directories for TV, movies, and books
-- Unlike watch_directories (staging areas for incoming files), media roots are
-- where media permanently lives on disk.

-- Media root directories
CREATE TABLE media_roots (
    id          UUID PRIMARY KEY,
    path        TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL DEFAULT '',
    media_type  TEXT NOT NULL CHECK (media_type IN ('tv', 'movie', 'book')),
    enabled     BOOLEAN NOT NULL DEFAULT TRUE,
    auto_add    BOOLEAN NOT NULL DEFAULT TRUE,

    -- Default entitlement scope for books (books need entitlements for Kobo sync)
    default_entitlement_scope TEXT NOT NULL DEFAULT 'all_users',

    -- Scan state
    scan_status TEXT NOT NULL DEFAULT 'idle' CHECK (scan_status IN ('idle', 'scanning', 'error')),
    last_scan_at TIMESTAMPTZ,
    scan_error  TEXT,
    error_count INTEGER NOT NULL DEFAULT 0,
    next_retry_at TIMESTAMPTZ,

    -- Naming template (NULL = use default for media_type)
    naming_template TEXT,

    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Files discovered in media roots
CREATE TABLE media_root_files (
    id              UUID PRIMARY KEY,
    media_root_id   UUID NOT NULL REFERENCES media_roots(id) ON DELETE CASCADE,
    file_path       TEXT NOT NULL UNIQUE,
    file_name       TEXT NOT NULL,
    file_size       BIGINT,
    file_mtime      TIMESTAMPTZ,

    -- Match state machine: pending → identified → matched | ambiguous | unmatched | ignored
    match_status    TEXT NOT NULL DEFAULT 'pending'
                    CHECK (match_status IN ('pending', 'identified', 'matched', 'ambiguous', 'unmatched', 'ignored')),

    -- Polymorphic link to matched entity
    entity_type     TEXT CHECK (entity_type IN ('movie', 'tv_episode', 'book')),
    entity_id       UUID,

    -- Raw parsed release info from interpreter
    interpretation  JSONB,
    -- TMDB candidates when ambiguous
    match_candidates JSONB,

    -- File still exists on disk
    available       BOOLEAN NOT NULL DEFAULT TRUE,

    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_media_root_files_root_id ON media_root_files(media_root_id);
CREATE INDEX idx_media_root_files_match_status ON media_root_files(media_root_id, match_status);
CREATE INDEX idx_media_root_files_entity ON media_root_files(entity_type, entity_id) WHERE entity_id IS NOT NULL;

-- Link tv_shows, movies, and books to their media root
ALTER TABLE tv_shows ADD COLUMN media_root_id UUID REFERENCES media_roots(id) ON DELETE SET NULL;
ALTER TABLE movies ADD COLUMN media_root_id UUID REFERENCES media_roots(id) ON DELETE SET NULL;
ALTER TABLE books ADD COLUMN media_root_id UUID REFERENCES media_roots(id) ON DELETE SET NULL;
