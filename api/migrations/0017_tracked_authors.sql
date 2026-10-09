-- Tracked authors for monitoring and auto-want functionality
CREATE TABLE IF NOT EXISTS tracked_authors (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    provider TEXT NOT NULL DEFAULT 'hardcover',
    provider_id TEXT NOT NULL,             -- Hardcover author ID

    name TEXT NOT NULL,
    image_url TEXT,
    metadata JSONB DEFAULT '{}',           -- cached: bio, book_count, etc.

    -- Tracking type
    is_implicit BOOLEAN NOT NULL DEFAULT FALSE,  -- TRUE = auto-created from library, hidden from UI
    enabled BOOLEAN NOT NULL DEFAULT TRUE,

    -- Want behavior (only applies when is_implicit = FALSE and monitoring enabled)
    auto_want_mode TEXT NOT NULL DEFAULT 'none',  -- 'all' | 'new_only' | 'none'
    monitoring_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    tracking_started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Update scheduling
    last_checked_at TIMESTAMPTZ,
    next_check_at TIMESTAMPTZ,
    check_interval_hours INT NOT NULL DEFAULT 24,

    known_book_count INT DEFAULT 0,

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE(provider, provider_id)
);

CREATE INDEX IF NOT EXISTS idx_tracked_authors_next_check
    ON tracked_authors(next_check_at) WHERE enabled = TRUE;
CREATE INDEX IF NOT EXISTS idx_tracked_authors_implicit
    ON tracked_authors(is_implicit) WHERE is_implicit = FALSE;
CREATE INDEX IF NOT EXISTS idx_tracked_authors_name
    ON tracked_authors(name);

-- Books discovered for each tracked author
CREATE TABLE IF NOT EXISTS tracked_author_books (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tracked_author_id UUID NOT NULL REFERENCES tracked_authors(id) ON DELETE CASCADE,

    provider_book_id TEXT NOT NULL,        -- Hardcover book ID
    title TEXT NOT NULL,
    cover_url TEXT,

    -- Series info (metadata, not a separate tracking entity)
    series_name TEXT,
    series_id TEXT,                        -- Hardcover series ID
    series_index REAL,

    release_date DATE,
    is_released BOOLEAN NOT NULL DEFAULT TRUE,

    -- Linking
    wanted_item_id UUID REFERENCES wanted_items(id) ON DELETE SET NULL,
    book_id UUID REFERENCES books(id) ON DELETE SET NULL,  -- if in library

    -- Status
    want_status TEXT NOT NULL DEFAULT 'pending',  -- 'pending' | 'wanted' | 'skipped' | 'owned'

    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE(tracked_author_id, provider_book_id)
);

CREATE INDEX IF NOT EXISTS idx_tracked_author_books_author
    ON tracked_author_books(tracked_author_id);
CREATE INDEX IF NOT EXISTS idx_tracked_author_books_series
    ON tracked_author_books(series_id);
CREATE INDEX IF NOT EXISTS idx_tracked_author_books_status
    ON tracked_author_books(want_status);
CREATE INDEX IF NOT EXISTS idx_tracked_author_books_wanted
    ON tracked_author_books(wanted_item_id) WHERE wanted_item_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_tracked_author_books_book
    ON tracked_author_books(book_id) WHERE book_id IS NOT NULL;
