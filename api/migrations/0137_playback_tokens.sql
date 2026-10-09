-- Playback tokens for cookie-free authentication (Google Cast, external players).
-- Short-lived tokens that can be appended as query parameters to streaming URLs.
CREATE TABLE playback_tokens (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token      TEXT NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_playback_tokens_token ON playback_tokens (token);
CREATE INDEX idx_playback_tokens_expires ON playback_tokens (expires_at);
