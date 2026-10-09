-- Index for JOIN on book_authors(author_id) - speeds up /library/authors significantly
CREATE INDEX IF NOT EXISTS idx_book_authors_author_id ON book_authors(author_id);

-- Index for JOIN on shelf_items(shelf_id) - speeds up /library/shelves
CREATE INDEX IF NOT EXISTS idx_shelf_items_shelf_id ON shelf_items(shelf_id);
