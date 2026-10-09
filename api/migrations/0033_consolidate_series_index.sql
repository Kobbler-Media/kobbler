-- Consolidate dual series_index (INTEGER) + series_index_float (DOUBLE PRECISION)
-- into a single series_index (DOUBLE PRECISION) column.
-- The integer column was always derivable from the float; remove redundancy.

ALTER TABLE books DROP COLUMN IF EXISTS series_index;
ALTER TABLE books RENAME COLUMN series_index_float TO series_index;
