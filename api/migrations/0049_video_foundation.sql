-- Video foundation: tables, indexes, seed data, and ALTERs for movie support.
-- Creates people, movies, movie_files, movie_cast, movie_crew,
-- movie_watch_states, video_metadata_providers, video_sync_providers.
-- Alters wanted_attempts (nullable book_id, movie_id FK, entity CHECK, movie upsert index)
-- and watch_directories (media_type discriminator).

--------------------------------------------------------------------------------
-- people
--------------------------------------------------------------------------------

CREATE TABLE people (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    sort_name TEXT,
    identifiers JSONB NOT NULL DEFAULT '{}',
    images JSONB NOT NULL DEFAULT '{}'::jsonb,
    bio TEXT,
    birth_date DATE,
    death_date DATE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX idx_people_identifiers_tmdb ON people ((identifiers->>'tmdb_id'));
CREATE INDEX idx_people_sort_name ON people (sort_name);

--------------------------------------------------------------------------------
-- movies
--------------------------------------------------------------------------------

CREATE TABLE movies (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    title TEXT NOT NULL,
    sort_title TEXT,
    original_title TEXT,
    year INTEGER,
    runtime INTEGER,
    description TEXT,
    tagline TEXT,
    images JSONB NOT NULL DEFAULT '{}'::jsonb,
    identifiers JSONB NOT NULL DEFAULT '{}',
    genres TEXT[],
    rating DOUBLE PRECISION,
    rating_count INTEGER,
    content_rating TEXT,
    is_in_library BOOLEAN NOT NULL DEFAULT FALSE,
    want_status TEXT NOT NULL DEFAULT 'none',
    release_date DATE,
    is_released BOOLEAN DEFAULT TRUE,
    quality_profile_id UUID,
    user_edited_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX idx_movies_identifiers_tmdb ON movies ((identifiers->>'tmdb_id'));
CREATE INDEX idx_movies_identifiers_imdb ON movies ((identifiers->>'imdb_id'));
CREATE INDEX idx_movies_want_status ON movies (want_status) WHERE want_status != 'none';
CREATE INDEX idx_movies_sort_title ON movies (sort_title);

--------------------------------------------------------------------------------
-- movie_files
--------------------------------------------------------------------------------

CREATE TABLE movie_files (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    movie_id UUID NOT NULL REFERENCES movies(id) ON DELETE CASCADE,
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

--------------------------------------------------------------------------------
-- movie_cast (surrogate PK, NOT composite)
--------------------------------------------------------------------------------

CREATE TABLE movie_cast (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    movie_id UUID NOT NULL REFERENCES movies(id) ON DELETE CASCADE,
    person_id UUID NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    character_name TEXT,
    cast_order INTEGER
);
CREATE INDEX idx_movie_cast_movie ON movie_cast (movie_id, cast_order);
CREATE INDEX idx_movie_cast_person ON movie_cast (person_id);

--------------------------------------------------------------------------------
-- movie_crew (composite PK)
--------------------------------------------------------------------------------

CREATE TABLE movie_crew (
    movie_id UUID NOT NULL REFERENCES movies(id) ON DELETE CASCADE,
    person_id UUID NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    department TEXT NOT NULL,
    job TEXT NOT NULL,
    PRIMARY KEY (movie_id, person_id, department, job)
);
CREATE INDEX idx_movie_crew_job ON movie_crew (job) WHERE job IN ('Director', 'Writer');

--------------------------------------------------------------------------------
-- movie_watch_states
--------------------------------------------------------------------------------

CREATE TABLE movie_watch_states (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    movie_id UUID NOT NULL REFERENCES movies(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    watched BOOLEAN NOT NULL DEFAULT FALSE,
    watched_at TIMESTAMPTZ,
    position_seconds INTEGER,
    rating DOUBLE PRECISION,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (movie_id, user_id)
);

--------------------------------------------------------------------------------
-- video_metadata_providers + seed
--------------------------------------------------------------------------------

CREATE TABLE video_metadata_providers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider_type TEXT NOT NULL UNIQUE,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    priority INTEGER NOT NULL DEFAULT 0,
    config JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
INSERT INTO video_metadata_providers (provider_type, enabled, priority) VALUES
    ('tmdb', true, 1),
    ('tvmaze', false, 2),
    ('omdb', false, 3)
ON CONFLICT (provider_type) DO NOTHING;

--------------------------------------------------------------------------------
-- video_sync_providers + seed
--------------------------------------------------------------------------------

CREATE TABLE video_sync_providers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider_type TEXT NOT NULL UNIQUE,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    config JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
INSERT INTO video_sync_providers (provider_type, enabled) VALUES
    ('trakt', false),
    ('simkl', false),
    ('letterboxd', false)
ON CONFLICT (provider_type) DO NOTHING;

--------------------------------------------------------------------------------
-- ALTER existing tables
--------------------------------------------------------------------------------

-- wanted_attempts: relax book_id to nullable (movie attempts won't have a book_id)
ALTER TABLE wanted_attempts ALTER COLUMN book_id DROP NOT NULL;

-- wanted_attempts: allow linking to movies
ALTER TABLE wanted_attempts ADD COLUMN movie_id UUID REFERENCES movies(id) ON DELETE SET NULL;

-- wanted_attempts: prevent orphaned rows (at least one entity FK must be set)
ALTER TABLE wanted_attempts ADD CONSTRAINT wanted_attempts_entity_check
    CHECK (num_nonnulls(book_id, movie_id) >= 1);

-- wanted_attempts: unique index for movie attempt upserts (mirrors book index from 0032)
CREATE UNIQUE INDEX wanted_attempts_movie_source_external_idx
    ON wanted_attempts (movie_id, source, external_id)
    WHERE movie_id IS NOT NULL AND external_id IS NOT NULL;

-- watch_directories: discriminate media type
ALTER TABLE watch_directories ADD COLUMN media_type TEXT NOT NULL DEFAULT 'book'
    CHECK (media_type IN ('book', 'tv', 'movie'));
