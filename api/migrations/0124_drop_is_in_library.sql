-- Remove denormalized is_in_library column from tv_episodes, movies, and books.
-- Library status is now derived at query time via EXISTS checks against file tables.
ALTER TABLE tv_episodes DROP COLUMN IF EXISTS is_in_library;
ALTER TABLE movies DROP COLUMN IF EXISTS is_in_library;
ALTER TABLE books DROP COLUMN IF EXISTS is_in_library;
