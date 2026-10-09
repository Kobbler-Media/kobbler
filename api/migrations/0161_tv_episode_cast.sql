-- Per-episode credits (guest stars and episode crew), cached from the metadata
-- provider. Show-level regulars stay in tv_cast; this table is only the people
-- who are specific to one episode.
CREATE TABLE tv_episode_cast (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    episode_id UUID NOT NULL REFERENCES tv_episodes(id) ON DELETE CASCADE,
    person_id UUID NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    credit_type TEXT NOT NULL CHECK (credit_type IN ('guest', 'crew')),
    character_name TEXT,
    cast_order INTEGER,
    department TEXT,
    job TEXT
);

CREATE INDEX idx_tv_episode_cast_episode ON tv_episode_cast (episode_id, credit_type, cast_order);
CREATE INDEX idx_tv_episode_cast_person ON tv_episode_cast (person_id);

-- NULL means "never asked the provider", which is distinct from "asked and the
-- episode has no guest stars". Without it the lazy fetch re-hits TMDB forever
-- for every episode whose credits are legitimately empty.
ALTER TABLE tv_episodes ADD COLUMN credits_fetched_at TIMESTAMPTZ;
