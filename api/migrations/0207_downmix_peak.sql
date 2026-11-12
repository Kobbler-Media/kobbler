-- True peak (dBTP) of the stereo fold-down players render the audio as, so the levelling
-- boost that `lufs` asks for can be capped before it clips.
ALTER TABLE movie_files ADD COLUMN downmix_peak_db DOUBLE PRECISION;
ALTER TABLE tv_episode_files ADD COLUMN downmix_peak_db DOUBLE PRECISION;
ALTER TABLE audiobook_files ADD COLUMN downmix_peak_db DOUBLE PRECISION;
ALTER TABLE debrid_stream_sessions ADD COLUMN downmix_peak_db DOUBLE PRECISION;
