ALTER TABLE activity_log    ADD COLUMN user_id UUID REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE wanted_attempts ADD COLUMN user_id UUID REFERENCES users(id) ON DELETE SET NULL;

CREATE INDEX idx_activity_log_user_id ON activity_log (user_id) WHERE user_id IS NOT NULL;
CREATE INDEX idx_wanted_attempts_user_id ON wanted_attempts (user_id) WHERE user_id IS NOT NULL;
