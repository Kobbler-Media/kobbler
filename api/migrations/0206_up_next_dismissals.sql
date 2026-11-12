-- target is a movie or show UUID (as text) or a Stremio id. A row hides the item only while
-- dismissed_at is at least the user's latest activity on it, so watching again brings it back.
CREATE TABLE up_next_dismissals (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('movie', 'show', 'stremio')),
    target TEXT NOT NULL,
    dismissed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, kind, target)
);
