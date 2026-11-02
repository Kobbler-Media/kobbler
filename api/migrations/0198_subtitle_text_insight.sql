-- When a track's own text was last read to fill in what its tags left out: language,
-- forced, SDH. NULL means not yet; a checked track is not read again.
ALTER TABLE subtitle_tracks
    ADD COLUMN text_checked_at TIMESTAMPTZ;

-- A debrid stream's subtitle tracks, named the same way from the first stretch of the
-- download, for its status stream to send while the rest is still arriving.
ALTER TABLE debrid_stream_sessions
    ADD COLUMN subtitle_tracks JSONB;
