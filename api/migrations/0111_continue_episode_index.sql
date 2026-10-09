-- Index for continue_episode query: list_for_show joins tv_watch_states
-- by user_id through episodes/seasons. The unique constraint is on
-- (episode_id, user_id) which doesn't help when the query drives from
-- user_id. This index lets the planner efficiently find all watch states
-- for a user and join outward to episodes.
CREATE INDEX IF NOT EXISTS idx_tv_watch_states_user_id
  ON tv_watch_states(user_id);
