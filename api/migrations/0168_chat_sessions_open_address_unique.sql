-- One *open* session per address, not one session ever.
--
-- 0157 guarded (transport, space, thread) without excluding closed sessions, but
-- `open_addressed_session` closes a stale session and immediately inserts a fresh one
-- on the same address. So the first message after the 45-minute idle window collided
-- with the row it had just closed: the turn died before it started, the sender was told
-- "I couldn't get to the library just then — try me again in a minute", and every later
-- message in that conversation failed the same way, because the closed row went on
-- occupying the slot. It never recovered.
--
-- Narrowing the index is what the code always assumed: an address has at most one open
-- session, and closed ones are history that may repeat. It only ever covers fewer rows
-- than before, so it cannot fail against existing data — and dropping the old index
-- frees every conversation that is currently wedged.
DROP INDEX IF EXISTS chat_sessions_address;

CREATE UNIQUE INDEX chat_sessions_address
    ON chat_sessions (transport, space, thread)
    WHERE space IS NOT NULL AND thread IS NOT NULL AND closed_at IS NULL;
