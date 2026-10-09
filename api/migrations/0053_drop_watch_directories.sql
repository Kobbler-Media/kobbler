-- Remove watch directory support from kobbler.
-- Watch directories are now managed by kobload instead.

-- Delete ingest queue items that came from watch directories
DELETE FROM books WHERE id IN (
    SELECT book_id FROM book_ingest_source WHERE watch_directory_id IS NOT NULL
) AND ingest_status IS NOT NULL;

-- Drop watch_directory_id FK from book_ingest_source
ALTER TABLE book_ingest_source DROP COLUMN IF EXISTS watch_directory_id;

-- Drop watch_directory_id FK from book_files
ALTER TABLE book_files DROP COLUMN IF EXISTS watch_directory_id;

-- Drop the watch_directories table
DROP TABLE IF EXISTS watch_directories CASCADE;
