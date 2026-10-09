-- Migration: Add file deduplication support
-- Tracks source file metadata to prevent duplicate ingestion

-- Track original source for deduplication
ALTER TABLE book_files ADD COLUMN source_path TEXT;
ALTER TABLE book_files ADD COLUMN source_mtime TIMESTAMPTZ;
ALTER TABLE book_files ADD COLUMN file_hash TEXT;

CREATE INDEX idx_book_files_source_path ON book_files(source_path) WHERE source_path IS NOT NULL;
CREATE INDEX idx_book_files_hash ON book_files(file_hash) WHERE file_hash IS NOT NULL;

-- Track mtime in queue for change detection
ALTER TABLE ingest_queue ADD COLUMN source_mtime TIMESTAMPTZ;

-- Flag for update vs new ingest
ALTER TABLE ingest_queue ADD COLUMN update_target_book_id UUID REFERENCES books(id) ON DELETE SET NULL;
