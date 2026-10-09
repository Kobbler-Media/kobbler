ALTER TABLE media_roots ADD COLUMN is_default BOOLEAN NOT NULL DEFAULT FALSE;

-- At most one default per media_type
CREATE UNIQUE INDEX idx_media_roots_default_per_type
    ON media_roots (media_type) WHERE is_default = TRUE;
