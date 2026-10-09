-- Backfill sort_title for TV shows and movies, and keep it maintained.
--
-- 0026 added sort_title with backfill + triggers for books/series/authors, but the
-- video tables (0049/0050) only ever got the column. Nothing populated it, so every
-- list/letter-jump query fell through `COALESCE(sort_title, title)` and filed
-- "The Leftovers" under T.
--
-- Two changes here:
--   1. compute_sort_title() now trims and uses a single case-insensitive regex, and
--      refuses to return an empty string (a show literally titled "The").
--   2. A shared trigger function that only *defaults* sort_title: a hand-entered
--      value survives, and an auto-derived one follows the title when it changes.
--      Books/series move onto it too — their 0026 triggers overwrote unconditionally,
--      so editing a book title silently discarded a manual sort title.

CREATE OR REPLACE FUNCTION compute_sort_title(title TEXT) RETURNS TEXT AS $$
DECLARE
    trimmed TEXT := btrim(COALESCE(title, ''));
    stripped TEXT;
BEGIN
    IF trimmed = '' THEN
        RETURN NULLIF(btrim(COALESCE(title, '')), '');
    END IF;

    stripped := btrim(regexp_replace(trimmed, '^(the|a|an)\s+', '', 'i'));

    -- Never strip a title down to nothing.
    IF stripped = '' THEN
        RETURN trimmed;
    END IF;

    RETURN stripped;
END;
$$ LANGUAGE plpgsql IMMUTABLE;

-- Shared BEFORE INSERT/UPDATE trigger. TG_ARGV[0] names the source column
-- ('title' by default, 'name' for series).
CREATE OR REPLACE FUNCTION set_sort_title_default() RETURNS TRIGGER AS $$
DECLARE
    src_col TEXT := COALESCE(TG_ARGV[0], 'title');
    new_src TEXT := to_jsonb(NEW) ->> src_col;
    old_src TEXT;
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.sort_title IS NULL OR btrim(NEW.sort_title) = '' THEN
            NEW.sort_title := compute_sort_title(new_src);
        END IF;
        RETURN NEW;
    END IF;

    old_src := to_jsonb(OLD) ->> src_col;

    -- Recompute when the column is empty, or when the stored value was itself
    -- derived from the old title (i.e. nobody has overridden it by hand).
    IF NEW.sort_title IS NULL
       OR btrim(NEW.sort_title) = ''
       OR (NEW.sort_title IS NOT DISTINCT FROM OLD.sort_title
           AND OLD.sort_title IS NOT DISTINCT FROM compute_sort_title(old_src))
    THEN
        NEW.sort_title := compute_sort_title(new_src);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- --------------------------------------------------------------------------
-- Triggers
-- --------------------------------------------------------------------------

DROP TRIGGER IF EXISTS tv_shows_sort_title_trigger ON tv_shows;
CREATE TRIGGER tv_shows_sort_title_trigger
    BEFORE INSERT OR UPDATE OF title, sort_title ON tv_shows
    FOR EACH ROW EXECUTE FUNCTION set_sort_title_default('title');

DROP TRIGGER IF EXISTS movies_sort_title_trigger ON movies;
CREATE TRIGGER movies_sort_title_trigger
    BEFORE INSERT OR UPDATE OF title, sort_title ON movies
    FOR EACH ROW EXECUTE FUNCTION set_sort_title_default('title');

-- Replace the unconditional 0026 triggers with the defaulting one.
DROP TRIGGER IF EXISTS books_sort_title_trigger ON books;
CREATE TRIGGER books_sort_title_trigger
    BEFORE INSERT OR UPDATE OF title, sort_title ON books
    FOR EACH ROW EXECUTE FUNCTION set_sort_title_default('title');

DROP TRIGGER IF EXISTS series_sort_title_trigger ON series;
CREATE TRIGGER series_sort_title_trigger
    BEFORE INSERT OR UPDATE OF name, sort_title ON series
    FOR EACH ROW EXECUTE FUNCTION set_sort_title_default('name');

DROP FUNCTION IF EXISTS books_set_sort_title();
DROP FUNCTION IF EXISTS series_set_sort_title();

-- --------------------------------------------------------------------------
-- Backfill
-- --------------------------------------------------------------------------

UPDATE tv_shows SET sort_title = compute_sort_title(title)
 WHERE sort_title IS NULL OR btrim(sort_title) = '';

UPDATE movies SET sort_title = compute_sort_title(title)
 WHERE sort_title IS NULL OR btrim(sort_title) = '';

-- Repair rows whose sort_title is just a copy of the title and still leads with an
-- article. A deliberate override would not be byte-identical to the title.
UPDATE books SET sort_title = compute_sort_title(title)
 WHERE (sort_title IS NULL OR btrim(sort_title) = '' OR sort_title = title)
   AND title ~* '^\s*(the|a|an)\s+';

UPDATE series SET sort_title = compute_sort_title(name)
 WHERE (sort_title IS NULL OR btrim(sort_title) = '' OR sort_title = name)
   AND name ~* '^\s*(the|a|an)\s+';
