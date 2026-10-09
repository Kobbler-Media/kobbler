-- The full broadcast instant, when a provider actually publishes one (TVMaze
-- airstamp). air_date stays the authoritative broadcast *day* — the search
-- index, the calendar and every existing query read it, and the UTC instant
-- here can land on a different calendar date. NULL means we know the day only,
-- and callers must say "Thursday" rather than inventing a time.
ALTER TABLE tv_episodes ADD COLUMN air_time TIMESTAMPTZ;

CREATE INDEX idx_tv_episodes_air_time ON tv_episodes (air_time) WHERE air_time IS NOT NULL;
