-- Add failure backoff columns to iptv_providers so broken providers don't
-- get re-enqueued every 5 minutes in an error loop.
ALTER TABLE iptv_providers
    ADD COLUMN consecutive_sync_failures INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN next_sync_retry_at TIMESTAMPTZ;
