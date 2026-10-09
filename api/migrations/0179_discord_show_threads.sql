-- Discord per-show release threads.
--
-- Release notifications group by show: one thread per (agent, show) so a show's
-- episodes stop scrolling past the announcements channel as a flat firehose. The
-- thread id is Discord's snowflake, stored as text to match the message ids in
-- discord_message_map.
CREATE TABLE discord_show_threads (
    agent_id UUID NOT NULL REFERENCES notification_agents(id) ON DELETE CASCADE,
    show_id UUID NOT NULL,
    thread_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (agent_id, show_id)
);

-- Discord message map: which message announced which entity, so an upgrade can
-- edit the original post in place ("… (Upgraded)") instead of announcing the same
-- item twice. One row per (agent, entity); the latest post wins.
CREATE TABLE discord_message_map (
    agent_id UUID NOT NULL REFERENCES notification_agents(id) ON DELETE CASCADE,
    entity_id UUID NOT NULL,
    message_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (agent_id, entity_id)
);