-- Reset thumbnails for regeneration with 5s interval and accurate timestamps.
UPDATE movie_files SET thumbnails_generated = false WHERE thumbnails_generated = true;
UPDATE tv_episode_files SET thumbnails_generated = false WHERE thumbnails_generated = true;
UPDATE iptv_recordings SET thumbnails_generated = false WHERE thumbnails_generated = true;
