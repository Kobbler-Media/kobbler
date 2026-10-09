ALTER TABLE media_roots
    ADD COLUMN scan_interval_seconds INT NULL,
    ADD COLUMN last_scan_completed_at TIMESTAMPTZ NULL;

COMMENT ON COLUMN media_roots.scan_interval_seconds IS
    'Per-root override of the global media_root_scan_interval_seconds. '
    'NULL = use global default. Typical values: 300 for local SSD with '
    'external file drops, 1800-3600 for NAS where kobload handles ingest.';

COMMENT ON COLUMN media_roots.last_scan_completed_at IS
    'Wall-clock timestamp of the last successful full scan completion. '
    'Used by media_root_scan_check to rate-limit periodic scans per '
    'the effective scan_interval_seconds.';
