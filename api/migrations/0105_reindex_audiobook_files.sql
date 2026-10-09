-- Recompute file_index for all audiobook files using natural sort order.
-- Sort priority: disc_number, track_number, last numeric sequence in
-- filename stem (as integer), file_path lexicographic fallback.
--
-- The UNIQUE(book_id, file_index) constraint requires a two-step update:
-- first shift all indices to negative (guaranteed unique), then assign
-- correct values.

-- Step 1: Move all file_index to negative to avoid uniqueness conflicts
UPDATE audiobook_files SET file_index = -(file_index + 1);

-- Step 2: Assign correct 0-based indices using natural sort
WITH numbered AS (
    SELECT id,
           ROW_NUMBER() OVER (
               PARTITION BY book_id
               ORDER BY disc_number ASC NULLS LAST,
                      track_number ASC NULLS LAST,
                      NULLIF(
                          (regexp_match(
                              regexp_replace(
                                  regexp_replace(file_path, '^.*/', ''),
                                  '\.[^.]+$', ''
                              ),
                              '(\d+)(?!.*\d)'
                          ))[1],
                          NULL
                      )::int ASC NULLS LAST,
                      file_path ASC
           ) - 1 AS new_index
    FROM audiobook_files
)
UPDATE audiobook_files af
SET file_index = n.new_index
FROM numbered n
WHERE af.id = n.id;
