-- Kobbler Database Schema
-- Consolidated from 24 incremental migrations for the stable release.

CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

--------------------------------------------------------------------------------
-- Core Tables (no dependencies)
--------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS users (
    id UUID PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    is_admin BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS authors (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS series (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name TEXT NOT NULL,
    sort_key TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS ingest_tokens (
    id UUID PRIMARY KEY,
    token TEXT NOT NULL UNIQUE,
    revoked BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS metadata_providers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider_type TEXT NOT NULL UNIQUE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    priority INTEGER NOT NULL DEFAULT 100,
    api_key TEXT,
    rate_limit_per_minute INTEGER NOT NULL DEFAULT 30,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Insert default metadata providers
INSERT INTO metadata_providers (provider_type, priority) VALUES
    ('openlibrary', 10),
    ('googlebooks', 20),
    ('hardcover', 30)
ON CONFLICT (provider_type) DO NOTHING;

CREATE TABLE IF NOT EXISTS system_config (
    id INTEGER PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    public_base_url TEXT NOT NULL DEFAULT 'http://localhost:8080',
    public_proxy_url TEXT NOT NULL DEFAULT '',
    kobo_storeapi_base_url TEXT NOT NULL DEFAULT 'https://storeapi.kobo.com',
    kobo_mode TEXT NOT NULL DEFAULT 'merge',
    proxy_logging_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    proxy_log_retention_days INTEGER NOT NULL DEFAULT 7,
    blob_capture_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    kobo_sync_batch_size BIGINT NOT NULL DEFAULT 100,
    activity_limit BIGINT NOT NULL DEFAULT 200,
    default_page_size BIGINT NOT NULL DEFAULT 50,
    max_page_size BIGINT NOT NULL DEFAULT 200,
    default_entitlement_mode TEXT NOT NULL DEFAULT 'all_devices',
    watcher_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    watcher_scan_interval_seconds INTEGER NOT NULL DEFAULT 60,
    metadata_providers_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    metadata_cache_days INTEGER NOT NULL DEFAULT 30,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Insert default system configuration row
INSERT INTO system_config (id) VALUES (1) ON CONFLICT (id) DO NOTHING;

CREATE TABLE IF NOT EXISTS watch_directories (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    path TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    ingest_mode TEXT NOT NULL DEFAULT 'manual',
    file_mode TEXT NOT NULL DEFAULT 'copy',
    default_entitlement_scope TEXT NOT NULL DEFAULT 'all_users',
    last_scan_at TIMESTAMPTZ,
    scan_status TEXT NOT NULL DEFAULT 'idle',
    scan_error TEXT,
    error_count INTEGER NOT NULL DEFAULT 0,
    next_retry_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS integrations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    integration_type TEXT NOT NULL,
    source_type TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    poll_url TEXT,
    poll_interval_seconds INTEGER DEFAULT 300,
    last_poll_at TIMESTAMPTZ,
    poll_cursor TEXT,
    auth_header TEXT,
    webhook_secret TEXT,
    ingest_mode TEXT NOT NULL DEFAULT 'manual',
    default_entitlement_scope TEXT NOT NULL DEFAULT 'all_users',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS jobs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    job_type TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    payload JSONB NOT NULL DEFAULT '{}',
    result JSONB,
    error_message TEXT,
    progress INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ
);

COMMENT ON TABLE jobs IS 'Generic job queue for background tasks (metadata writes, cover fetches, etc.)';
COMMENT ON COLUMN jobs.job_type IS 'Type of job: metadata_write, cover_fetch, etc.';
COMMENT ON COLUMN jobs.status IS 'Job status: pending, running, completed, failed';
COMMENT ON COLUMN jobs.payload IS 'Job-specific input data as JSON';
COMMENT ON COLUMN jobs.result IS 'Job-specific output/result as JSON';
COMMENT ON COLUMN jobs.progress IS 'Optional progress indicator 0-100';

CREATE TABLE IF NOT EXISTS metadata_cache (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider_type TEXT NOT NULL,
    query_key TEXT NOT NULL,
    response JSONB NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (provider_type, query_key)
);

--------------------------------------------------------------------------------
-- Tables depending on users
--------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS devices (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token TEXT NOT NULL UNIQUE,
    name TEXT,
    kobo_api_url TEXT DEFAULT 'https://storeapi.kobo.com',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS auth_tokens (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token TEXT NOT NULL UNIQUE,
    scope TEXT NOT NULL DEFAULT 'admin',
    revoked BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS shelves (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS shelf_archives (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT,
    last_modified TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

--------------------------------------------------------------------------------
-- Tables depending on series
--------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS books (
    id UUID PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT,
    language TEXT,
    publisher TEXT,
    pubdate TIMESTAMPTZ,
    series_id UUID REFERENCES series(id),
    series_index INTEGER,
    series_index_float DOUBLE PRECISION,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_books_series_id ON books(series_id);

--------------------------------------------------------------------------------
-- Tables depending on books
--------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS book_authors (
    book_id UUID NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    author_id UUID NOT NULL REFERENCES authors(id) ON DELETE CASCADE,
    role TEXT,
    PRIMARY KEY (book_id, author_id)
);

CREATE TABLE IF NOT EXISTS book_files (
    id UUID PRIMARY KEY,
    book_id UUID NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    format TEXT NOT NULL,
    size_bytes BIGINT NOT NULL,
    path TEXT NOT NULL,
    available BOOLEAN NOT NULL DEFAULT TRUE,
    last_checked_at TIMESTAMPTZ,
    unavailable_since TIMESTAMPTZ,
    watch_directory_id UUID REFERENCES watch_directories(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (book_id, format)
);

CREATE INDEX IF NOT EXISTS idx_book_files_available ON book_files(available) WHERE NOT available;
CREATE INDEX IF NOT EXISTS idx_book_files_watch_directory ON book_files(watch_directory_id) WHERE watch_directory_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS entitlements (
    id UUID PRIMARY KEY,
    book_id UUID NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    scope_type TEXT NOT NULL,
    scope_id UUID,
    is_removed BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS shelf_items (
    shelf_id UUID NOT NULL REFERENCES shelves(id) ON DELETE CASCADE,
    book_id UUID NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    added_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (shelf_id, book_id)
);

CREATE INDEX IF NOT EXISTS shelf_items_book_id_idx ON shelf_items(book_id);

--------------------------------------------------------------------------------
-- Tables depending on devices
--------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS device_sync_state (
    device_id UUID PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
    last_sync_at TIMESTAMPTZ,
    last_sync_token TEXT
);

--------------------------------------------------------------------------------
-- Reading state tables (depend on books, users, devices)
--------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS reading_states (
    id UUID PRIMARY KEY,
    book_id UUID NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    priority_timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (book_id, user_id, device_id)
);

CREATE TABLE IF NOT EXISTS reading_status (
    reading_state_id UUID PRIMARY KEY REFERENCES reading_states(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'ReadyToRead',
    times_started_reading INTEGER NOT NULL DEFAULT 0,
    last_time_started_reading TIMESTAMPTZ,
    last_modified_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS reading_statistics (
    reading_state_id UUID PRIMARY KEY REFERENCES reading_states(id) ON DELETE CASCADE,
    spent_reading_minutes INTEGER,
    remaining_time_minutes INTEGER,
    last_modified_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS reading_bookmark (
    reading_state_id UUID PRIMARY KEY REFERENCES reading_states(id) ON DELETE CASCADE,
    progress_percent DOUBLE PRECISION,
    content_source_progress_percent DOUBLE PRECISION,
    location_value TEXT,
    location_type TEXT,
    location_source TEXT,
    last_modified_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

--------------------------------------------------------------------------------
-- Ingest tables
--------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS ingest_queue (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    watch_directory_id UUID REFERENCES watch_directories(id) ON DELETE CASCADE,
    file_path TEXT NOT NULL,
    file_name TEXT NOT NULL,
    file_size BIGINT NOT NULL,
    file_hash TEXT UNIQUE,
    source_format TEXT NOT NULL,
    extracted_title TEXT,
    extracted_authors TEXT[],
    extracted_isbn TEXT,
    extracted_description TEXT,
    extracted_cover BYTEA,
    selected_metadata JSONB,
    status TEXT NOT NULL DEFAULT 'pending',
    conversion_error TEXT,
    book_id UUID REFERENCES books(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_ingest_queue_status ON ingest_queue(status);
CREATE INDEX IF NOT EXISTS idx_ingest_queue_watch_directory ON ingest_queue(watch_directory_id);

CREATE TABLE IF NOT EXISTS ingest_events (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    source TEXT NOT NULL,
    book_id UUID REFERENCES books(id) ON DELETE SET NULL,
    device_id UUID REFERENCES devices(id) ON DELETE SET NULL,
    status TEXT NOT NULL,
    message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS ingest_events_created_at_idx ON ingest_events(created_at DESC);
CREATE INDEX IF NOT EXISTS ingest_events_device_id_idx ON ingest_events(device_id);

--------------------------------------------------------------------------------
-- Proxy logs
--------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS proxy_logs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    device_id UUID REFERENCES devices(id) ON DELETE SET NULL,
    request_method TEXT NOT NULL,
    request_path TEXT NOT NULL,
    request_query TEXT,
    request_headers JSONB,
    request_body TEXT,
    request_body_size BIGINT NOT NULL DEFAULT 0,
    request_blob_path TEXT,
    response_status SMALLINT,
    response_headers JSONB,
    response_body TEXT,
    response_body_size BIGINT NOT NULL DEFAULT 0,
    response_blob_path TEXT,
    original_response_headers JSONB,
    decision TEXT NOT NULL,
    upstream_url TEXT,
    upstream_status SMALLINT,
    upstream_headers JSONB,
    upstream_body TEXT,
    upstream_body_size BIGINT DEFAULT 0,
    upstream_duration_ms INTEGER,
    total_duration_ms INTEGER,
    severity TEXT NOT NULL DEFAULT 'info',
    status TEXT NOT NULL DEFAULT 'complete',
    notes TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS proxy_logs_created_at_idx ON proxy_logs(created_at DESC);
CREATE INDEX IF NOT EXISTS proxy_logs_device_id_idx ON proxy_logs(device_id);
CREATE INDEX IF NOT EXISTS proxy_logs_request_path_idx ON proxy_logs(request_path);
CREATE INDEX IF NOT EXISTS proxy_logs_severity_idx ON proxy_logs(severity) WHERE severity != 'info';
CREATE INDEX IF NOT EXISTS idx_proxy_logs_status ON proxy_logs(status) WHERE status = 'in_progress';

--------------------------------------------------------------------------------
-- Jobs indexes
--------------------------------------------------------------------------------

CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs(status);
CREATE INDEX IF NOT EXISTS idx_jobs_type_status ON jobs(job_type, status);
CREATE INDEX IF NOT EXISTS idx_jobs_created ON jobs(created_at DESC);

--------------------------------------------------------------------------------
-- Metadata cache indexes
--------------------------------------------------------------------------------

CREATE INDEX IF NOT EXISTS idx_metadata_cache_expires ON metadata_cache(expires_at);
