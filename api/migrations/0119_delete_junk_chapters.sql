-- Delete auto-generated junk chapters (scene detection, timestamp titles, bare numbers).
-- Only deletes from files where ALL chapters are junk; mixed sets are kept.
DELETE FROM video_chapters
WHERE COALESCE(episode_file_id, movie_file_id) IN (
    SELECT COALESCE(episode_file_id, movie_file_id) AS file_id
    FROM video_chapters
    GROUP BY file_id
    HAVING COUNT(*) FILTER (
        WHERE title IS NOT NULL
        AND title != ''
        AND title !~ '^\d{2}:\d{2}:\d{2}'
        AND title !~ '^Scene \d+$'
        AND title != 'No Scene Description'
        AND title !~ '^\d+\.?$'
    ) = 0
);
