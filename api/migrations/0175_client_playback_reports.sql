-- Playback failures reported by a client.
--
-- The player knows things the server cannot see — which URL libVLC was actually
-- handed, what the device is, what the app build is — and until now a hard
-- failure on the client left no trace anywhere. `detail` is the same text the
-- error dialog shows, so a report is exactly what the person was looking at.
CREATE TABLE client_playback_reports (
    id UUID PRIMARY KEY,
    user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    client_kind TEXT NOT NULL,
    app_version TEXT,
    device_label TEXT,
    media_type TEXT,
    media_id TEXT,
    stream_url TEXT,
    session_id TEXT,
    detail TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_client_playback_reports_created_at
    ON client_playback_reports (created_at DESC);
