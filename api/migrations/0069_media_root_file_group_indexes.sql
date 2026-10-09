-- Expression indexes for server-side entity grouping of media root files.
-- These support GROUP BY on JSONB-extracted fields used by the entity-groups endpoint.

CREATE INDEX idx_mrf_tv_show_name
    ON media_root_files (media_root_id, (LOWER(interpretation->>'show_name')))
    WHERE available = TRUE AND interpretation->>'show_name' IS NOT NULL;

CREATE INDEX idx_mrf_movie_title
    ON media_root_files (media_root_id, (LOWER(interpretation->>'movie_title')))
    WHERE available = TRUE AND interpretation->>'movie_title' IS NOT NULL;

CREATE INDEX idx_mrf_book_series
    ON media_root_files (media_root_id, (LOWER(interpretation->>'series')))
    WHERE available = TRUE AND interpretation->>'series' IS NOT NULL;
