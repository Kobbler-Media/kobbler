-- Conversation store for the chatbot loop (Tier 1 of the three-tier design).
--
-- Keyed for two front ends at once. Google Chat has a real conversation address
-- — `(space, thread)` — so those rows are unique on it. The Command Palette has
-- no such address: its "session" is the browser's conversation, which the client
-- resets on idle, so a web row carries a NULL space/thread and is identified by
-- `user_id` plus being the most recently updated open row. The partial unique
-- index therefore only constrains the transports that can actually key.
--
-- `closed_at`/`summary` belong to Tier 2 (session summaries) and are unused for
-- now. They are here because adding them later is a migration for two columns
-- that cost nothing empty.

CREATE TABLE chat_sessions (
    id         UUID PRIMARY KEY,
    transport  TEXT NOT NULL,
    space      TEXT,
    thread     TEXT,
    user_id    UUID REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    closed_at  TIMESTAMPTZ,
    summary    TEXT
);

CREATE UNIQUE INDEX chat_sessions_address
    ON chat_sessions (transport, space, thread)
    WHERE space IS NOT NULL AND thread IS NOT NULL;

CREATE INDEX chat_sessions_open_for_user
    ON chat_sessions (transport, user_id, updated_at DESC)
    WHERE closed_at IS NULL;

-- `steps` holds the durable per-tool record of a turn: one entry per tool call
-- with its arguments summary, state and observation. `content` is the narration
-- the user saw. Tool observations live in `steps`, never in `content`, so a
-- replay can drop the observations without losing the reply.

CREATE TABLE chat_messages (
    id         UUID PRIMARY KEY,
    session_id UUID NOT NULL REFERENCES chat_sessions(id) ON DELETE CASCADE,
    role       TEXT NOT NULL,
    content    TEXT NOT NULL DEFAULT '',
    steps      JSONB NOT NULL DEFAULT '[]'::jsonb,
    status     TEXT NOT NULL DEFAULT 'complete',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX chat_messages_session ON chat_messages (session_id, created_at);
