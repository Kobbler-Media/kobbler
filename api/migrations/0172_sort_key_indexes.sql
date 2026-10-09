-- Index the expression the alphabetical lists actually sort and paginate on.
--
-- 0026/0049/0050 indexed the bare sort_title column, but every list and letter-jump
-- query orders by COALESCE(sort_title, title) with (sort_key, id) keyset cursors —
-- an expression no plain column index can serve, so each page was a full scan + sort.
-- Books joined that scheme in this change; movies and shows have been on it since 0171.

CREATE INDEX IF NOT EXISTS idx_books_sort_key
    ON books ((COALESCE(sort_title, title)), id);

CREATE INDEX IF NOT EXISTS idx_movies_sort_key
    ON movies ((COALESCE(sort_title, title)), id);

CREATE INDEX IF NOT EXISTS idx_tv_shows_sort_key
    ON tv_shows ((COALESCE(sort_title, title)), id);
