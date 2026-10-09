-- Indexes for user-scoped queries (roles/ACL support)

-- devices.user_id: list_ids_for_user(), list_devices filtered by user, get_web_reader_for_user()
CREATE INDEX IF NOT EXISTS idx_devices_user_id ON devices(user_id);

-- shelves.user_id: count_with_filters(), list_with_filters(), list_modified_since()
CREATE INDEX IF NOT EXISTS idx_shelves_user_id ON shelves(user_id);

-- shelf_archives.user_id: list_archives_since() for Kobo sync
CREATE INDEX IF NOT EXISTS idx_shelf_archives_user_id ON shelf_archives(user_id);

-- reading_states.user_id: list_changed_since(), get_most_recent_for_user_book(), etc.
CREATE INDEX IF NOT EXISTS idx_reading_states_user_id ON reading_states(user_id);

-- reading_states (user_id, updated_at): optimizes sync queries filtering by timestamp
CREATE INDEX IF NOT EXISTS idx_reading_states_user_updated ON reading_states(user_id, updated_at DESC);
