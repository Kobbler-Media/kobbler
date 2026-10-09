-- Bump updated_at on entitled books that have a cover source URL so Kobo
-- devices re-fetch covers now that the cover handler has a full fallback chain.
UPDATE books
SET updated_at = NOW()
WHERE EXISTS (
    SELECT 1 FROM entitlements e
    WHERE e.book_id = books.id AND NOT e.is_removed
)
AND images->'cover'->>'source_url' IS NOT NULL;
