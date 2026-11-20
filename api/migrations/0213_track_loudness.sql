-- Loudness was measured on whichever audio stream ffmpeg ranked best — the default-flagged
-- one, which in a dub release is the dub — and players levelled every track by it. Each
-- stream is measured now: [{"stream_index", "lufs", "downmix_peak_db"}], keyed by the
-- absolute stream index the audio tracks are addressed by.
ALTER TABLE movie_files ADD COLUMN audio_loudness JSONB;
ALTER TABLE tv_episode_files ADD COLUMN audio_loudness JSONB;
ALTER TABLE debrid_stream_sessions ADD COLUMN audio_loudness JSONB;

-- With a single audio stream the old figure was that stream's; anything else is
-- re-measured by the loudness fill.
UPDATE movie_files f
SET audio_loudness = jsonb_build_array(jsonb_strip_nulls(jsonb_build_object(
    'stream_index', t.track_index, 'lufs', f.lufs, 'downmix_peak_db', f.downmix_peak_db)))
FROM audio_tracks t
WHERE t.movie_file_id = f.id
  AND f.lufs IS NOT NULL
  AND f.downmix_peak_db IS NOT NULL
  AND (SELECT COUNT(*) FROM audio_tracks c WHERE c.movie_file_id = f.id) = 1;

UPDATE tv_episode_files f
SET audio_loudness = jsonb_build_array(jsonb_strip_nulls(jsonb_build_object(
    'stream_index', t.track_index, 'lufs', f.lufs, 'downmix_peak_db', f.downmix_peak_db)))
FROM audio_tracks t
WHERE t.episode_file_id = f.id
  AND f.lufs IS NOT NULL
  AND f.downmix_peak_db IS NOT NULL
  AND (SELECT COUNT(*) FROM audio_tracks c WHERE c.episode_file_id = f.id) = 1;

ALTER TABLE movie_files DROP COLUMN lufs, DROP COLUMN downmix_peak_db;
ALTER TABLE tv_episode_files DROP COLUMN lufs, DROP COLUMN downmix_peak_db;
ALTER TABLE debrid_stream_sessions DROP COLUMN lufs, DROP COLUMN downmix_peak_db;
