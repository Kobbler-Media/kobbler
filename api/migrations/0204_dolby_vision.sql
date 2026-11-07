-- Profile 5 (base-layer compatibility id 0) has no picture a non-Dolby-Vision
-- decoder can show, so playback has to know it before choosing to copy the video.
ALTER TABLE movie_files
    ADD COLUMN dv_profile SMALLINT,
    ADD COLUMN dv_bl_compat_id SMALLINT;

ALTER TABLE tv_episode_files
    ADD COLUMN dv_profile SMALLINT,
    ADD COLUMN dv_bl_compat_id SMALLINT;
