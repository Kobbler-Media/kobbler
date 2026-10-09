CREATE TABLE chat_watches (
    id           UUID PRIMARY KEY,
    transport    TEXT NOT NULL,
    target       JSONB NOT NULL,
    message_id   TEXT,
    entity_kind  TEXT NOT NULL,
    entity_id    UUID NOT NULL,
    label        TEXT NOT NULL,
    user_id      UUID REFERENCES users(id) ON DELETE SET NULL,
    last_state   TEXT,
    last_edit_at TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at   TIMESTAMPTZ NOT NULL,
    CHECK (entity_kind IN ('episode', 'movie', 'book', 'wanted'))
);

-- One watch per conversation per entity: asking twice refreshes the existing row
-- rather than making the same download report itself twice into one thread.
CREATE UNIQUE INDEX chat_watches_conversation_entity ON chat_watches (
    transport,
    (target ->> 'space'),
    COALESCE(target ->> 'thread', ''),
    entity_id
);
CREATE INDEX chat_watches_entity ON chat_watches (entity_id, expires_at);
CREATE INDEX chat_watches_expires_at ON chat_watches (expires_at);
CREATE INDEX chat_watches_user_id ON chat_watches (user_id) WHERE user_id IS NOT NULL;
