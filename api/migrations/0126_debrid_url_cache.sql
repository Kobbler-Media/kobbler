-- Cache resolved debrid URL and its expiry on the session to avoid re-resolving on every play.
ALTER TABLE debrid_stream_sessions
    ADD COLUMN debrid_url TEXT,
    ADD COLUMN debrid_url_expires_at TIMESTAMPTZ;
