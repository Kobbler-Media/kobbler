CREATE TABLE tv_shows (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    title TEXT NOT NULL,
    sort_title TEXT,
    original_title TEXT,
    year INTEGER,
    status TEXT,  -- continuing, ended, canceled, upcoming
    description TEXT,
    images JSONB NOT NULL DEFAULT '{}'::jsonb,
    identifiers JSONB NOT NULL DEFAULT '{}',
    genres TEXT[],
    rating DOUBLE PRECISION,
    rating_count INTEGER,
    content_rating TEXT,
    network TEXT,
    is_monitored BOOLEAN NOT NULL DEFAULT FALSE,
    auto_want_mode TEXT NOT NULL DEFAULT 'none' CHECK (auto_want_mode IN ('none', 'new', 'all')),
    quality_profile_id UUID,
    first_aired DATE,
    last_aired DATE,
    next_airing DATE,
    runtime INTEGER,
    total_episode_count INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX idx_tv_shows_identifiers_tmdb ON tv_shows ((identifiers->>'tmdb_id'));
CREATE INDEX idx_tv_shows_identifiers_imdb ON tv_shows ((identifiers->>'imdb_id'));
CREATE INDEX idx_tv_shows_monitored ON tv_shows (is_monitored) WHERE is_monitored = TRUE;
CREATE INDEX idx_tv_shows_sort_title ON tv_shows (sort_title);

CREATE TABLE tv_seasons (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    show_id UUID NOT NULL REFERENCES tv_shows(id) ON DELETE CASCADE,
    season_identifier TEXT NOT NULL,
    season_sort_order INTEGER NOT NULL,
    title TEXT,
    description TEXT,
    images JSONB NOT NULL DEFAULT '{}'::jsonb,
    air_date DATE,
    episode_count INTEGER,
    is_monitored BOOLEAN NOT NULL DEFAULT TRUE,
    is_special BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (show_id, season_identifier)
);
CREATE INDEX idx_tv_seasons_show ON tv_seasons (show_id, season_sort_order);

CREATE TABLE tv_episodes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    season_id UUID NOT NULL REFERENCES tv_seasons(id) ON DELETE CASCADE,
    episode_identifier TEXT NOT NULL,
    episode_sort_order INTEGER NOT NULL,
    title TEXT,
    description TEXT,
    images JSONB NOT NULL DEFAULT '{}'::jsonb,
    air_date DATE,
    runtime INTEGER,
    is_in_library BOOLEAN NOT NULL DEFAULT FALSE,
    want_status TEXT NOT NULL DEFAULT 'none',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (season_id, episode_identifier)
);
CREATE INDEX idx_tv_episodes_season ON tv_episodes (season_id, episode_sort_order);
CREATE INDEX idx_tv_episodes_air_date ON tv_episodes (air_date) WHERE air_date IS NOT NULL;
CREATE INDEX idx_tv_episodes_want ON tv_episodes (want_status) WHERE want_status != 'none';

CREATE TABLE tv_episode_files (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    episode_id UUID NOT NULL REFERENCES tv_episodes(id) ON DELETE CASCADE,
    file_path TEXT NOT NULL,
    file_size BIGINT,
    format TEXT NOT NULL,
    quality JSONB,
    languages TEXT[],
    subtitles TEXT[],
    source_type TEXT,
    source_metadata JSONB,
    available BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- TV cast (show-level, surrogate PK for multi-character actors)
CREATE TABLE tv_cast (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    show_id UUID NOT NULL REFERENCES tv_shows(id) ON DELETE CASCADE,
    person_id UUID NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    character_name TEXT,
    cast_order INTEGER
);
CREATE INDEX idx_tv_cast_show ON tv_cast (show_id, cast_order);
CREATE INDEX idx_tv_cast_person ON tv_cast (person_id);

-- TV crew (show-level, composite PK)
CREATE TABLE tv_crew (
    show_id UUID NOT NULL REFERENCES tv_shows(id) ON DELETE CASCADE,
    person_id UUID NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    department TEXT NOT NULL,
    job TEXT NOT NULL,
    PRIMARY KEY (show_id, person_id, department, job)
);

-- Episode watch state
CREATE TABLE tv_watch_states (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    episode_id UUID NOT NULL REFERENCES tv_episodes(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    watched BOOLEAN NOT NULL DEFAULT FALSE,
    watched_at TIMESTAMPTZ,
    position_seconds INTEGER,
    rating DOUBLE PRECISION,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (episode_id, user_id)
);

-- Show-level user ratings (separate from per-episode watch state)
CREATE TABLE tv_show_ratings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    show_id UUID NOT NULL REFERENCES tv_shows(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    rating DOUBLE PRECISION NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (show_id, user_id)
);

-- Wanted attempts: add episode_id column
ALTER TABLE wanted_attempts ADD COLUMN episode_id UUID REFERENCES tv_episodes(id) ON DELETE SET NULL;

-- Update entity check to include episode_id
ALTER TABLE wanted_attempts DROP CONSTRAINT wanted_attempts_entity_check;
ALTER TABLE wanted_attempts ADD CONSTRAINT wanted_attempts_entity_check
    CHECK (num_nonnulls(book_id, movie_id, episode_id) >= 1);

-- Episode attempt upsert index
CREATE UNIQUE INDEX wanted_attempts_episode_source_external_idx
    ON wanted_attempts (episode_id, source, external_id)
    WHERE episode_id IS NOT NULL AND external_id IS NOT NULL;
