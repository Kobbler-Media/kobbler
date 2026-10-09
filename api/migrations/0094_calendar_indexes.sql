-- Indexes for calendar page date-range queries.
CREATE INDEX IF NOT EXISTS idx_movies_release_date ON movies (release_date) WHERE release_date IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_books_pubdate ON books (pubdate) WHERE pubdate IS NOT NULL;
