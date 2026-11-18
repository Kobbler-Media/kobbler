DELETE FROM user_sync_providers WHERE provider_type = 'trakt';
DELETE FROM device_sync_providers WHERE provider_type = 'trakt';

-- Watches imported from Trakt are still the user's history; keep them as library state.
UPDATE movie_watch_states SET source = 'local' WHERE source = 'trakt';
UPDATE tv_watch_states SET source = 'local' WHERE source = 'trakt';
