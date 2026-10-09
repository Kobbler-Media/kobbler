-- Add sidecar metadata support to media root files and write_sidecars flag to media roots.

-- Store parsed sidecar metadata (NFO, metadata.json, OPF) alongside discovered files.
-- This lets the match phase access provider IDs without re-reading the filesystem.
ALTER TABLE media_root_files ADD COLUMN sidecar_metadata JSONB;

-- Per-root opt-in flag for writing sidecar files after enrichment.
ALTER TABLE media_roots ADD COLUMN write_sidecars BOOLEAN NOT NULL DEFAULT true;

-- Per-root opt-in flag for writing image files after enrichment.
ALTER TABLE media_roots ADD COLUMN write_images BOOLEAN NOT NULL DEFAULT true;

-- Store discovered image sidecar paths (poster.jpg, fanart.jpg, cover.jpg, etc.)
-- alongside the file record, parallel to sidecar_metadata.
ALTER TABLE media_root_files ADD COLUMN sidecar_images JSONB;
