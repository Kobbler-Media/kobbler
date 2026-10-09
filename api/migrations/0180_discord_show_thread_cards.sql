-- Discord show threads get a channel "card".
--
-- A thread created without a starter message in a text channel defaults to a
-- PRIVATE thread (type 12), so every thread 0179 made is visible only to the bot
-- and moderators. They cannot be converted, so drop them: the next release for
-- each show creates a fresh public thread.
--
-- card_message_id is the bot's message in the parent channel that names the show
-- and links its thread. It is deleted and re-posted on every update so the show
-- surfaces at the bottom of the channel (Discord has no way to bump a message).
DELETE FROM discord_show_threads;

ALTER TABLE discord_show_threads ADD COLUMN card_message_id TEXT;
