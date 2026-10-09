-- Add 'sport' to media_roots.media_type CHECK constraint
ALTER TABLE media_roots DROP CONSTRAINT media_roots_media_type_check;
ALTER TABLE media_roots ADD CONSTRAINT media_roots_media_type_check
    CHECK (media_type IN ('tv', 'movie', 'book', 'sport'));
