-- Add audiobook support: new columns on books, audiobook_chapters table, media_root type

-- New columns on books for audiobook metadata
ALTER TABLE books ADD COLUMN narrator TEXT;
ALTER TABLE books ADD COLUMN duration_seconds INTEGER;
ALTER TABLE books ADD COLUMN is_abridged BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE books ADD COLUMN want_format TEXT NOT NULL DEFAULT 'ebook';

-- Audiobook chapters table (for both single-file and multi-file audiobooks)
CREATE TABLE audiobook_chapters (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    book_file_id UUID NOT NULL REFERENCES book_files(id) ON DELETE CASCADE,
    chapter_index INTEGER NOT NULL,
    title TEXT,
    start_seconds DOUBLE PRECISION NOT NULL,
    end_seconds DOUBLE PRECISION NOT NULL,
    file_path TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (book_file_id, chapter_index)
);
CREATE INDEX idx_audiobook_chapters_file ON audiobook_chapters(book_file_id);

-- Add 'audiobook' to media_roots.media_type CHECK constraint
ALTER TABLE media_roots DROP CONSTRAINT media_roots_media_type_check;
ALTER TABLE media_roots ADD CONSTRAINT media_roots_media_type_check
    CHECK (media_type IN ('tv', 'movie', 'book', 'sport', 'audiobook'));
