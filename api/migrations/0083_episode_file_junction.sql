-- Multi-episode file support: extract episode_id from tv_episode_files into
-- a junction table so one physical file can map to multiple episodes.

-- 1. Create junction table
CREATE TABLE tv_episode_file_episodes (
    episode_file_id UUID NOT NULL REFERENCES tv_episode_files(id) ON DELETE CASCADE,
    episode_id UUID NOT NULL REFERENCES tv_episodes(id) ON DELETE CASCADE,
    PRIMARY KEY (episode_file_id, episode_id)
);
CREATE INDEX idx_tefe_episode_id ON tv_episode_file_episodes(episode_id);

-- 2. Populate from existing episode_id column
INSERT INTO tv_episode_file_episodes (episode_file_id, episode_id)
SELECT id, episode_id FROM tv_episode_files;

-- 3. Drop the old column (CASCADE drops any indexes referencing it)
ALTER TABLE tv_episode_files DROP COLUMN episode_id;

