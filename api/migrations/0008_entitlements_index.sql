-- Add index on entitlements.book_id for faster joins and lookups
-- This column is heavily used in:
--   - JOIN entitlements e ON e.book_id = b.id
--   - WHERE book_id = $1 AND scope_type = ...
CREATE INDEX IF NOT EXISTS idx_entitlements_book_id ON entitlements(book_id);

-- Composite index for common query pattern: looking up entitlements by book and scope
CREATE INDEX IF NOT EXISTS idx_entitlements_book_scope ON entitlements(book_id, scope_type);
