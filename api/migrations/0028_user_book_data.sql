-- User book data: ratings and reading status independent of device sync
-- Also adds Hardcover API key fields to users and devices tables

--------------------------------------------------------------------------------
-- User book data table (ratings per user per book)
--------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS user_book_data (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_id UUID NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    -- Rating 1-5 stars
    rating INTEGER CHECK (rating IS NULL OR (rating >= 1 AND rating <= 5)),
    -- Source of rating: 'manual' | 'hardcover' | 'kobo'
    rating_source TEXT,
    rating_updated_at TIMESTAMPTZ,
    -- Reading status for UI: 'want_to_read' | 'reading' | 'finished' | null
    -- This is user's personal status, separate from device sync status
    reading_status TEXT,
    reading_status_updated_at TIMESTAMPTZ,
    -- When the user started/finished reading (for display)
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(user_id, book_id)
);

CREATE INDEX IF NOT EXISTS idx_user_book_data_user ON user_book_data(user_id);
CREATE INDEX IF NOT EXISTS idx_user_book_data_book ON user_book_data(book_id);
CREATE INDEX IF NOT EXISTS idx_user_book_data_status ON user_book_data(reading_status) WHERE reading_status IS NOT NULL;

COMMENT ON TABLE user_book_data IS 'Per-user book data: ratings and personal reading status';
COMMENT ON COLUMN user_book_data.rating IS 'User rating 1-5 stars';
COMMENT ON COLUMN user_book_data.rating_source IS 'Source of rating: manual, hardcover, kobo';
COMMENT ON COLUMN user_book_data.reading_status IS 'Personal reading status for UI display';

--------------------------------------------------------------------------------
-- Add Hardcover API key fields to users table
--------------------------------------------------------------------------------

ALTER TABLE users ADD COLUMN IF NOT EXISTS hardcover_api_key TEXT;
ALTER TABLE users ADD COLUMN IF NOT EXISTS hardcover_enabled BOOLEAN NOT NULL DEFAULT TRUE;

COMMENT ON COLUMN users.hardcover_api_key IS 'User-specific Hardcover API key (overrides system key)';
COMMENT ON COLUMN users.hardcover_enabled IS 'Whether Hardcover sync is enabled for this user';

--------------------------------------------------------------------------------
-- Add Hardcover sync enabled to devices table
--------------------------------------------------------------------------------

ALTER TABLE devices ADD COLUMN IF NOT EXISTS hardcover_sync_enabled BOOLEAN NOT NULL DEFAULT TRUE;

COMMENT ON COLUMN devices.hardcover_sync_enabled IS 'Whether Hardcover sync is enabled for this device';
