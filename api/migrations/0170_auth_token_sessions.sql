-- Session metadata for auth tokens so devices can be listed and revoked by name,
-- and so an admin-minted token records who provisioned it.
ALTER TABLE auth_tokens ADD COLUMN device_name TEXT;
ALTER TABLE auth_tokens ADD COLUMN client_kind TEXT;
ALTER TABLE auth_tokens ADD COLUMN last_used_at TIMESTAMPTZ;
ALTER TABLE auth_tokens ADD COLUMN created_by UUID REFERENCES users(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_auth_tokens_user_active
    ON auth_tokens (user_id, created_at DESC)
    WHERE revoked = FALSE;
