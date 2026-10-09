-- Add role column to users table for ACL support.
-- Existing users default to 'admin' to preserve current access.
ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'admin';
