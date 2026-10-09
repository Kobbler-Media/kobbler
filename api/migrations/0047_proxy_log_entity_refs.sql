ALTER TABLE proxy_logs
  ADD COLUMN entity_refs JSONB NOT NULL DEFAULT '[]'::jsonb,
  ADD COLUMN has_local_leak BOOLEAN NOT NULL DEFAULT false;

CREATE INDEX proxy_logs_has_local_leak_idx ON proxy_logs(has_local_leak) WHERE has_local_leak = true;
