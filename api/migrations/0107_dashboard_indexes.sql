-- Dashboard query performance indexes

-- In-progress movies: covers WHERE user_id = $1 AND watched = FALSE AND position_seconds >= 60
-- ORDER BY updated_at DESC with a partial index so only active watch states are indexed.
CREATE INDEX idx_movie_watch_states_in_progress
  ON movie_watch_states(user_id, updated_at DESC)
  WHERE watched = FALSE AND position_seconds >= 60;

-- In-progress episodes: same pattern as movies.
CREATE INDEX idx_tv_watch_states_in_progress
  ON tv_watch_states(user_id, updated_at DESC)
  WHERE watched = FALSE AND position_seconds >= 60;

-- Recent movies: supports the JOIN on movie_id + GROUP BY + ORDER BY MAX(created_at) DESC
-- and the correlated subquery MAX(duration_seconds) WHERE movie_id = $X AND available = true.
CREATE INDEX idx_movie_files_movie_available
  ON movie_files(movie_id, created_at DESC)
  WHERE available = TRUE;

-- Recent episodes: supports WHERE available = true ORDER BY created_at DESC
-- for the DISTINCT ON query that finds the latest episode per show.
CREATE INDEX idx_episode_files_available_created
  ON tv_episode_files(created_at DESC)
  WHERE available = TRUE;
