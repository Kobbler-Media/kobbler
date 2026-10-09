-- Add keyframe timestamp cache for instant HLS playlist generation.
-- Stores an array of keyframe PTS values: [0.0, 10.01, 20.02, ...]
-- Populated by media_probe job, used by segment cache for accurate
-- segment boundary prediction without waiting for FFmpeg.

ALTER TABLE movie_files ADD COLUMN keyframes JSONB;
ALTER TABLE tv_episode_files ADD COLUMN keyframes JSONB;
