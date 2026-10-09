-- Index the letter the alphabet bars group by.
--
-- The bars are served from the search index and fall back to SQL when it is unavailable,
-- and that fallback asks each table for the distinct first letter of its sort key. The
-- sort-key index from 0172 cannot serve it: a btree on COALESCE(sort_title, title) has to
-- be read end to end and sorted to find 26 distinct values.
--
-- Spelled exactly as repositories/letters.rs spells it, including the '#' fold — an
-- expression index only serves a query that asks the same question the same way.

CREATE INDEX IF NOT EXISTS idx_books_first_letter
    ON books ((CASE WHEN COALESCE(sort_title, title) ~ '^[A-Za-z]'
                    THEN UPPER(LEFT(COALESCE(sort_title, title), 1)) ELSE '#' END))
    WHERE ingest_status IS NULL;

CREATE INDEX IF NOT EXISTS idx_movies_first_letter
    ON movies ((CASE WHEN COALESCE(sort_title, title) ~ '^[A-Za-z]'
                     THEN UPPER(LEFT(COALESCE(sort_title, title), 1)) ELSE '#' END));

-- Carries content_category so the TV bar and the sports bar each read only their own
-- rows out of the shared table.
CREATE INDEX IF NOT EXISTS idx_tv_shows_first_letter
    ON tv_shows (content_category,
                 (CASE WHEN COALESCE(sort_title, title) ~ '^[A-Za-z]'
                       THEN UPPER(LEFT(COALESCE(sort_title, title), 1)) ELSE '#' END));
