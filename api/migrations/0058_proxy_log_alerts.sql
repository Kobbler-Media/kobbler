-- Generalize has_local_leak to has_alert with a descriptive alert_message.
ALTER TABLE proxy_logs RENAME COLUMN has_local_leak TO has_alert;
ALTER TABLE proxy_logs ADD COLUMN alert_message TEXT;
DROP INDEX IF EXISTS proxy_logs_has_local_leak_idx;
CREATE INDEX proxy_logs_has_alert_idx ON proxy_logs(has_alert) WHERE has_alert = true;
