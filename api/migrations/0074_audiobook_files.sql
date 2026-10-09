-- Dedicated audiobook_files table for multi-file audio support.
-- Replaces the use of book_files for audio content, which was limited to
-- one file per format per book due to UNIQUE(book_id, format).

CREATE TABLE audiobook_files (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    book_id UUID NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    file_index INTEGER NOT NULL DEFAULT 0,
    format TEXT NOT NULL,
    file_path TEXT NOT NULL,
    file_size BIGINT NOT NULL DEFAULT 0,
    file_hash TEXT,
    duration_seconds DOUBLE PRECISION,
    bitrate INTEGER,
    codec TEXT,
    channels INTEGER,
    sample_rate INTEGER,
    track_number INTEGER,
    disc_number INTEGER,
    source_path TEXT,
    source_mtime TIMESTAMPTZ,
    source_type TEXT,
    source_metadata JSONB DEFAULT '{}',
    available BOOLEAN NOT NULL DEFAULT TRUE,
    last_checked_at TIMESTAMPTZ,
    unavailable_since TIMESTAMPTZ,
    added_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (book_id, file_index),
    UNIQUE (book_id, file_path)
);

CREATE INDEX idx_audiobook_files_book ON audiobook_files(book_id);
CREATE INDEX idx_audiobook_files_available ON audiobook_files(available) WHERE NOT available;
CREATE INDEX idx_audiobook_files_source_path ON audiobook_files(source_path) WHERE source_path IS NOT NULL;

-- Migrate existing audio records from book_files.
-- Only 1 file per format per book exists (due to old UNIQUE constraint),
-- so file_index=0 is safe for all migrated records.
INSERT INTO audiobook_files (
    id, book_id, file_index, format, file_path, file_size, file_hash,
    source_path, source_mtime, source_type, source_metadata,
    available, last_checked_at, unavailable_since, added_at
)
SELECT
    bf.id, bf.book_id, 0, bf.format, bf.path, bf.size_bytes, bf.file_hash,
    bf.source_path, bf.source_mtime, bf.source_type,
    COALESCE(bf.source_metadata, '{}'),
    bf.available, bf.last_checked_at, bf.unavailable_since, bf.added_at
FROM book_files bf
WHERE LOWER(bf.format) IN ('m4b', 'm4a', 'mp3', 'flac', 'ogg', 'opus');

-- Re-point audiobook_chapters FK from book_files to audiobook_files.
ALTER TABLE audiobook_chapters DROP CONSTRAINT audiobook_chapters_book_file_id_fkey;
DROP INDEX IF EXISTS idx_audiobook_chapters_file;
ALTER TABLE audiobook_chapters RENAME COLUMN book_file_id TO audiobook_file_id;
ALTER TABLE audiobook_chapters
    ADD CONSTRAINT audiobook_chapters_audiobook_file_id_fkey
    FOREIGN KEY (audiobook_file_id) REFERENCES audiobook_files(id) ON DELETE CASCADE;
ALTER TABLE audiobook_chapters
    DROP CONSTRAINT audiobook_chapters_book_file_id_chapter_index_key;
ALTER TABLE audiobook_chapters
    ADD CONSTRAINT audiobook_chapters_audiobook_file_id_chapter_index_key
    UNIQUE (audiobook_file_id, chapter_index);
CREATE INDEX idx_audiobook_chapters_file ON audiobook_chapters(audiobook_file_id);

-- Remove migrated audio records from book_files.
DELETE FROM book_files WHERE LOWER(format) IN ('m4b', 'm4a', 'mp3', 'flac', 'ogg', 'opus');
