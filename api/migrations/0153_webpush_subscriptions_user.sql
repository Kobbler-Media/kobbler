-- Scope web push subscriptions to the user who created them. Until now only
-- admins could subscribe and delivery fanned out to every stored subscription,
-- so any existing row belongs to an admin.

ALTER TABLE webpush_subscriptions
    ADD COLUMN user_id UUID REFERENCES users(id) ON DELETE CASCADE;

UPDATE webpush_subscriptions
SET user_id = (SELECT id FROM users WHERE role = 'admin' ORDER BY created_at ASC LIMIT 1)
WHERE user_id IS NULL;

DELETE FROM webpush_subscriptions WHERE user_id IS NULL;

ALTER TABLE webpush_subscriptions
    ALTER COLUMN user_id SET NOT NULL;

CREATE INDEX idx_webpush_subscriptions_user_id ON webpush_subscriptions(user_id);
