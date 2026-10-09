-- Free episodes stranded at want_status = 'imported' with nothing on disk.
--
-- A successful import parks an episode at 'imported'. Deleting its file removed
-- the tv_episode_files row but left the status, and every path that could pick
-- the episode back up matches want_status = 'none': list_missing, want_season,
-- want_all_missing, auto_want, and delete_orphaned_episodes. So the episode
-- rendered as missing, no want would ever touch it, and the orphan sweeper
-- would not clear it either.
--
-- The delete handlers now do this reset themselves; this catches the rows that
-- predate that. The NOT EXISTS guard leaves any episode that still has a file
-- alone, and restricting to 'imported' leaves in-flight acquisitions
-- ('wanted'/'searching'/'downloading'/'ingesting') untouched.

UPDATE tv_episodes
SET want_status = 'none',
    updated_at = NOW()
WHERE want_status = 'imported'
  AND NOT EXISTS (
      SELECT 1
      FROM tv_episode_file_episodes efe
      WHERE efe.episode_id = tv_episodes.id
  );
