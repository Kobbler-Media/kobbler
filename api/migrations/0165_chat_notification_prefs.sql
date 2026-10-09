-- Per-user chat notification preferences, settable by talking to the bot.
--
-- A missing row means the defaults, so nothing has to create one up front:
-- on_import only, and the default quiet window. Chatty defaults are how a bot
-- gets muted permanently in week one.
--
-- quiet_from/quiet_to are wall-clock times in the SERVER's local timezone, not
-- UTC — the household shares one clock and nobody says "mute me from 22:00Z".
-- Both NULL means "unset", which the Rust layer reads as the default 22:00-07:00
-- window; a zero-width window (quiet_from = quiet_to) is how "no quiet hours at
-- all" is stored, because NULL is already spoken for.

CREATE TABLE chat_notification_prefs (
    user_id       UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    enabled       BOOLEAN NOT NULL DEFAULT TRUE,
    triggers      JSONB NOT NULL DEFAULT '["on_import"]',
    muted_until   TIMESTAMPTZ,
    quiet_from    TIME,
    quiet_to      TIME
);
