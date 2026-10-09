CREATE TABLE chat_identities (
    id           UUID PRIMARY KEY,
    transport    TEXT NOT NULL,
    address      TEXT,
    external_id  TEXT,
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    max_role     TEXT NOT NULL DEFAULT 'contributor',
    enabled      BOOLEAN NOT NULL DEFAULT TRUE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at TIMESTAMPTZ,
    CHECK (address IS NOT NULL OR external_id IS NOT NULL),
    CHECK (max_role IN ('viewer', 'contributor', 'admin'))
);

CREATE UNIQUE INDEX chat_identities_address ON chat_identities (transport, lower(address))
    WHERE address IS NOT NULL;
CREATE UNIQUE INDEX chat_identities_external ON chat_identities (transport, external_id)
    WHERE external_id IS NOT NULL;
CREATE INDEX chat_identities_user_id ON chat_identities (user_id);

CREATE TABLE chat_pairing_codes (
    id           UUID PRIMARY KEY,
    transport    TEXT NOT NULL,
    external_id  TEXT NOT NULL,
    code         TEXT NOT NULL,
    display_name TEXT,
    space        TEXT,
    thread       TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at   TIMESTAMPTZ NOT NULL,
    consumed_at  TIMESTAMPTZ,
    identity_id  UUID REFERENCES chat_identities(id) ON DELETE SET NULL
);

CREATE UNIQUE INDEX chat_pairing_codes_code ON chat_pairing_codes (transport, code);
CREATE INDEX chat_pairing_codes_external ON chat_pairing_codes (transport, external_id, created_at DESC);
CREATE INDEX chat_pairing_codes_pending ON chat_pairing_codes (created_at DESC) WHERE consumed_at IS NULL;
