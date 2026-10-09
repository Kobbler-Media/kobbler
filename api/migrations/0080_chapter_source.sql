-- Track the source of chapter data: embedded (from file metadata), filename (auto-generated),
-- or transcript (detected from transcript analysis).
ALTER TABLE audiobook_chapters ADD COLUMN source TEXT NOT NULL DEFAULT 'embedded';
