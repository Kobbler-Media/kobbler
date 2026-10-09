-- When skip detection last completed for a file, as opposed to when its media
-- cache was last built (`media_cache_generated_at`). A file is stale, and worth
-- re-detecting, when detection has never run or last ran before the cache it
-- reads was rebuilt.
--
-- Needed because "has skip segments" is NOT a usable proxy for "detection ran".
-- 557 episode files currently hold a complete cache and zero auto-detected
-- segments -- detection ran and legitimately found no intro or credits. Gating a
-- re-run on the presence of segment rows therefore marks those files stale
-- forever, and because detection is season-scoped a single such file drags its
-- whole season into every subsequent pass.
ALTER TABLE tv_episode_files ADD COLUMN IF NOT EXISTS skip_detect_ran_at TIMESTAMPTZ;
ALTER TABLE movie_files      ADD COLUMN IF NOT EXISTS skip_detect_ran_at TIMESTAMPTZ;

-- Seed from the newest auto-detected segment each file already has. Without
-- this every row starts NULL, every season reads as never-detected, and the
-- first pass after deploy re-detects the entire library -- 688 seasons instead
-- of the 132 that are genuinely out of date.
--
-- The segment timestamp is an approximation of when detection ran, and it is
-- only ever an under-estimate (segments are written at the end of a run), so it
-- can schedule an unnecessary re-run but never skip a needed one. Files with no
-- auto-detected segments stay NULL: they get detected once more, are stamped
-- properly on the way out, and settle.
UPDATE tv_episode_files tef
   SET skip_detect_ran_at = s.last_seg
  FROM (
        SELECT episode_file_id, MAX(created_at) AS last_seg
          FROM episode_skip_segments
         WHERE source <> 'chapter'
         GROUP BY episode_file_id
       ) s
 WHERE s.episode_file_id = tef.id;

UPDATE movie_files mf
   SET skip_detect_ran_at = s.last_seg
  FROM (
        SELECT movie_file_id, MAX(created_at) AS last_seg
          FROM movie_skip_segments
         WHERE source <> 'chapter'
         GROUP BY movie_file_id
       ) s
 WHERE s.movie_file_id = mf.id;

-- The staleness gate reads these two columns together, per file, on every
-- skip_detect job and on every batch enqueue.
CREATE INDEX IF NOT EXISTS tv_episode_files_skip_detect_staleness_idx
    ON tv_episode_files (skip_detect_ran_at, media_cache_generated_at);
CREATE INDEX IF NOT EXISTS movie_files_skip_detect_staleness_idx
    ON movie_files (skip_detect_ran_at, media_cache_generated_at);
