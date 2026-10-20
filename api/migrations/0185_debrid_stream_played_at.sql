-- Set once, the first time a session plays (a minute of progress or an explicit ok). It is
-- what "replay the stream that worked for this title" and next-episode continuity read,
-- and the NULL -> NOW() transition is what makes the success report and the next-episode
-- prewarm happen once per session. Release-level outcome history lives in kobload.
ALTER TABLE debrid_stream_sessions ADD COLUMN played_at TIMESTAMPTZ;
