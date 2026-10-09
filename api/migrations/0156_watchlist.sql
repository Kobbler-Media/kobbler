-- Per-user "want to watch" list.
--
-- Distinct from `want_status` (acquisition intent, global) and `is_monitored`
-- (keep grabbing this, also global): this is one user saying they intend to
-- watch something. It is the only per-user *forward* signal on the video side —
-- `tv_watch_states` only records what has already been watched, which is why
-- Up Next could never surface a show you had not already started.
--
-- Polymorphic on (media_type, media_id), matching `debrid_stream_sessions`.
-- No FK is possible across two parent tables, so the Up Next queries JOIN
-- through to tv_shows/movies and orphaned rows simply stop appearing; the
-- delete paths clean them up.

CREATE TABLE watchlist_items (
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    media_type TEXT NOT NULL CHECK (media_type IN ('show', 'movie')),
    media_id   UUID NOT NULL,
    added_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, media_type, media_id)
);

CREATE INDEX idx_watchlist_items_media ON watchlist_items (media_type, media_id);

-- Once something has actually been started, the intent has been satisfied and
-- the row is dropped.
--
-- This is a trigger rather than a call in the repository layer because there
-- are a dozen write sites for watch states — the two REST handlers, the debrid
-- stream reporter and the Trakt history importer — and a thirteenth added later
-- would silently skip the cleanup. The list queries already filter started
-- items out, so this keeps the stored rows honest for the button state too.

CREATE OR REPLACE FUNCTION watchlist_drop_started() RETURNS TRIGGER AS $$
DECLARE
    target_show UUID;
BEGIN
    IF TG_TABLE_NAME = 'movie_watch_states' THEN
        DELETE FROM watchlist_items
        WHERE user_id = NEW.user_id AND media_type = 'movie' AND media_id = NEW.movie_id;
    ELSE
        SELECT ts.show_id INTO target_show
        FROM tv_episodes te
        JOIN tv_seasons ts ON ts.id = te.season_id
        WHERE te.id = NEW.episode_id;

        IF target_show IS NOT NULL THEN
            DELETE FROM watchlist_items
            WHERE user_id = NEW.user_id AND media_type = 'show' AND media_id = target_show;
        END IF;
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

-- Fires at most twice per item: once if the state is born started, once on the
-- transition. A plain `position_seconds >= 60` guard would re-run the delete on
-- every progress ping for the rest of the playthrough.

CREATE TRIGGER tv_watch_states_drop_watchlist
    AFTER INSERT ON tv_watch_states
    FOR EACH ROW
    WHEN (NEW.watched OR COALESCE(NEW.position_seconds, 0) >= 60)
    EXECUTE FUNCTION watchlist_drop_started();

CREATE TRIGGER tv_watch_states_drop_watchlist_upd
    AFTER UPDATE ON tv_watch_states
    FOR EACH ROW
    WHEN (
        (NEW.watched AND NOT OLD.watched)
        OR (COALESCE(NEW.position_seconds, 0) >= 60 AND COALESCE(OLD.position_seconds, 0) < 60)
    )
    EXECUTE FUNCTION watchlist_drop_started();

CREATE TRIGGER movie_watch_states_drop_watchlist
    AFTER INSERT ON movie_watch_states
    FOR EACH ROW
    WHEN (NEW.watched OR COALESCE(NEW.position_seconds, 0) >= 60)
    EXECUTE FUNCTION watchlist_drop_started();

CREATE TRIGGER movie_watch_states_drop_watchlist_upd
    AFTER UPDATE ON movie_watch_states
    FOR EACH ROW
    WHEN (
        (NEW.watched AND NOT OLD.watched)
        OR (COALESCE(NEW.position_seconds, 0) >= 60 AND COALESCE(OLD.position_seconds, 0) < 60)
    )
    EXECUTE FUNCTION watchlist_drop_started();
