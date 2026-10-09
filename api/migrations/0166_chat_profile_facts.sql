-- Tier 3 of the conversation store: durable, per-user, reviewable profile facts.
--
-- Not chat history. Small enough to fit in every prompt rather than needing a
-- lookup, which is the whole reason it beats retrieval over session summaries.
-- `source_id` points at the session the fact was extracted from so a wrong fact
-- can be traced back to what was said; it is nulled rather than cascaded when
-- that session is swept, because the fact outlives the conversation.

CREATE TABLE chat_profile_facts (
    id         UUID PRIMARY KEY,
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL CHECK (kind IN ('prefers', 'likes', 'avoids')),
    value      TEXT NOT NULL,
    source_id  UUID REFERENCES chat_sessions(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX chat_profile_facts_user ON chat_profile_facts (user_id, created_at DESC);

-- The same fact re-extracted from a later session is the common case, and a
-- profile that repeats itself is a log. Case-insensitive so "70s horror" and
-- "70s Horror" are one fact.
CREATE UNIQUE INDEX chat_profile_facts_unique
    ON chat_profile_facts (user_id, kind, LOWER(value));
