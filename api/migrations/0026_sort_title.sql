-- Add sort_title columns for proper alphabetical ordering
-- Books and series: Strip articles (The, A, An) from the beginning
-- Authors: Last name first (e.g., "Brandon Sanderson" → "Sanderson, Brandon")

-- Books sort title
ALTER TABLE books ADD COLUMN IF NOT EXISTS sort_title TEXT;

-- Series sort title
ALTER TABLE series ADD COLUMN IF NOT EXISTS sort_title TEXT;

-- Authors sort name (last name first)
ALTER TABLE authors ADD COLUMN IF NOT EXISTS sort_name TEXT;

-- Helper function to compute sort title (strip articles)
CREATE OR REPLACE FUNCTION compute_sort_title(title TEXT) RETURNS TEXT AS $$
BEGIN
    -- Strip leading articles (case-insensitive)
    IF UPPER(LEFT(title, 4)) = 'THE ' THEN
        RETURN SUBSTRING(title FROM 5);
    ELSIF UPPER(LEFT(title, 2)) = 'A ' THEN
        RETURN SUBSTRING(title FROM 3);
    ELSIF UPPER(LEFT(title, 3)) = 'AN ' THEN
        RETURN SUBSTRING(title FROM 4);
    ELSE
        RETURN title;
    END IF;
END;
$$ LANGUAGE plpgsql IMMUTABLE;

-- Helper function to compute author sort name (last name first)
CREATE OR REPLACE FUNCTION compute_author_sort_name(name TEXT) RETURNS TEXT AS $$
DECLARE
    trimmed TEXT;
    space_pos INT;
BEGIN
    trimmed := TRIM(name);
    -- Find last space
    space_pos := LENGTH(trimmed) - POSITION(' ' IN REVERSE(trimmed)) + 1;

    -- If no space found, return as-is
    IF POSITION(' ' IN trimmed) = 0 THEN
        RETURN trimmed;
    END IF;

    -- Return "LastName, FirstName MiddleName..."
    RETURN SUBSTRING(trimmed FROM space_pos + 1) || ', ' || SUBSTRING(trimmed FOR space_pos - 1);
END;
$$ LANGUAGE plpgsql IMMUTABLE;

-- Backfill existing data
UPDATE books SET sort_title = compute_sort_title(title) WHERE sort_title IS NULL;
UPDATE series SET sort_title = compute_sort_title(name) WHERE sort_title IS NULL;
UPDATE authors SET sort_name = compute_author_sort_name(name) WHERE sort_name IS NULL;

-- Create indexes for efficient ordering
CREATE INDEX IF NOT EXISTS idx_books_sort_title ON books (sort_title);
CREATE INDEX IF NOT EXISTS idx_series_sort_title ON series (sort_title);
CREATE INDEX IF NOT EXISTS idx_authors_sort_name ON authors (sort_name);

-- Triggers to auto-compute sort_title/sort_name on insert/update

CREATE OR REPLACE FUNCTION books_set_sort_title() RETURNS TRIGGER AS $$
BEGIN
    NEW.sort_title := compute_sort_title(NEW.title);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION series_set_sort_title() RETURNS TRIGGER AS $$
BEGIN
    NEW.sort_title := compute_sort_title(NEW.name);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION authors_set_sort_name() RETURNS TRIGGER AS $$
BEGIN
    NEW.sort_name := compute_author_sort_name(NEW.name);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS books_sort_title_trigger ON books;
CREATE TRIGGER books_sort_title_trigger
    BEFORE INSERT OR UPDATE OF title ON books
    FOR EACH ROW EXECUTE FUNCTION books_set_sort_title();

DROP TRIGGER IF EXISTS series_sort_title_trigger ON series;
CREATE TRIGGER series_sort_title_trigger
    BEFORE INSERT OR UPDATE OF name ON series
    FOR EACH ROW EXECUTE FUNCTION series_set_sort_title();

DROP TRIGGER IF EXISTS authors_sort_name_trigger ON authors;
CREATE TRIGGER authors_sort_name_trigger
    BEFORE INSERT OR UPDATE OF name ON authors
    FOR EACH ROW EXECUTE FUNCTION authors_set_sort_name();
