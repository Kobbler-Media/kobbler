-- Track when content was first streamed via debrid (enables "streamed but not in library" filtering)
ALTER TABLE movies ADD COLUMN streamed_at TIMESTAMPTZ;
ALTER TABLE tv_shows ADD COLUMN streamed_at TIMESTAMPTZ;
