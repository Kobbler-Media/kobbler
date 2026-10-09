-- Add pending_verifier column for PKCE OAuth flow.
-- Stores the PKCE verifier between the two OAuth steps.

ALTER TABLE kindle_accounts ADD COLUMN pending_verifier TEXT;
