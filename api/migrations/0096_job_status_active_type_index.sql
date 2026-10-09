-- Composite index for has_active_job() queries that filter on job_type + status.
-- The existing idx_job_status_active only covers status, forcing a seq-scan of all
-- active rows to then filter by job_type and payload.
CREATE INDEX idx_job_status_type_active ON job_status (job_type, status)
  WHERE status IN ('pending', 'running', 'retrying');
