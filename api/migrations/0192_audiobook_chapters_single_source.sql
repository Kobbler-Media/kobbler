-- Chapter writers used to upsert by (file, chapter_index), so a later source overwrote the
-- leading rows of an earlier one and left a file holding two half-sets. Keep only the
-- strongest source on each such file; the audiobook chapter repair job restores embedded
-- chapters that were lost and rebuilds the rest.
DELETE FROM audiobook_chapters c
USING (
    SELECT audiobook_file_id,
           MAX(CASE source
                   WHEN 'detected' THEN 0
                   WHEN 'filename' THEN 1
                   WHEN 'transcript' THEN 2
                   WHEN 'provider' THEN 3
                   ELSE 4
               END) AS best
    FROM audiobook_chapters
    GROUP BY audiobook_file_id
    HAVING COUNT(DISTINCT source) > 1
) mixed
WHERE c.audiobook_file_id = mixed.audiobook_file_id
  AND CASE c.source
          WHEN 'detected' THEN 0
          WHEN 'filename' THEN 1
          WHEN 'transcript' THEN 2
          WHEN 'provider' THEN 3
          ELSE 4
      END < mixed.best;

DELETE FROM audiobook_chapters WHERE end_seconds <= start_seconds;

-- A provider's chapter list for a different recording of the book runs past the end of
-- this one (or stops well short of it), so every chapter lands in the wrong place.
DELETE FROM audiobook_chapters
WHERE source = 'provider'
  AND audiobook_file_id IN (
      SELECT c.audiobook_file_id
      FROM audiobook_chapters c
      JOIN audiobook_files af ON af.id = c.audiobook_file_id
      WHERE c.source = 'provider'
      GROUP BY c.audiobook_file_id, af.duration_seconds
      HAVING ABS(MAX(c.end_seconds) - af.duration_seconds)
             > GREATEST(af.duration_seconds * 0.02, 30)
  );
