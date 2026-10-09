-- Fix wanted_attempts FK delete behavior for movie_id and episode_id.
-- ON DELETE SET NULL can violate wanted_attempts_entity_check when the
-- nulled column was the only non-null entity FK. Change to CASCADE so
-- deleting a movie or episode also removes its wanted_attempts rows
-- (matching the existing book_id behavior from migration 0019).

-- movie_id: SET NULL → CASCADE
ALTER TABLE wanted_attempts DROP CONSTRAINT IF EXISTS wanted_attempts_movie_id_fkey;
ALTER TABLE wanted_attempts
    ADD CONSTRAINT wanted_attempts_movie_id_fkey
    FOREIGN KEY (movie_id) REFERENCES movies(id) ON DELETE CASCADE;

-- episode_id: SET NULL → CASCADE
ALTER TABLE wanted_attempts DROP CONSTRAINT IF EXISTS wanted_attempts_episode_id_fkey;
ALTER TABLE wanted_attempts
    ADD CONSTRAINT wanted_attempts_episode_id_fkey
    FOREIGN KEY (episode_id) REFERENCES tv_episodes(id) ON DELETE CASCADE;
