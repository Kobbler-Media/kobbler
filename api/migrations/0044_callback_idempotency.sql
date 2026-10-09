-- Add idempotency key to wanted_attempts for callback deduplication
-- When Kobload retries a callback, it sends the same idempotency_key
-- Kobbler uses this to detect and ignore duplicate callbacks

ALTER TABLE wanted_attempts ADD COLUMN IF NOT EXISTS idempotency_key UUID;

-- Index for fast lookup when checking for duplicates
CREATE UNIQUE INDEX IF NOT EXISTS wanted_attempts_idempotency_key_idx
    ON wanted_attempts (idempotency_key)
    WHERE idempotency_key IS NOT NULL;
