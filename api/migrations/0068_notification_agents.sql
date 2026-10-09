-- Notification agent table.
-- Each row is a configured notification endpoint (e.g., one Pushover device, one Plex server).
CREATE TABLE notification_agents (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_type TEXT NOT NULL,
    name TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT true,
    config JSONB NOT NULL DEFAULT '{}',
    triggers JSONB NOT NULL DEFAULT '[]',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_notification_agents_enabled ON notification_agents (enabled) WHERE enabled = true;

-- Web push subscriptions (per-browser).
-- Separate from notification_agents because one "webpush" agent can have many subscriptions.
CREATE TABLE webpush_subscriptions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    endpoint TEXT NOT NULL UNIQUE,
    p256dh TEXT NOT NULL,
    auth TEXT NOT NULL,
    user_agent TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- VAPID keys for web push (singleton system-wide).
ALTER TABLE system_config
    ADD COLUMN vapid_private_key TEXT,
    ADD COLUMN vapid_public_key TEXT;
