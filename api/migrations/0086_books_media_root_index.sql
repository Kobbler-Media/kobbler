-- Index on books.media_root_id for the orphaned books filter (IS NULL / IS NOT NULL).
CREATE INDEX idx_books_media_root_id ON books (media_root_id);
