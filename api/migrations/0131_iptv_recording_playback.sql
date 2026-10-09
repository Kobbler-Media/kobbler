-- Add media metadata and thumbnail tracking to IPTV recordings
ALTER TABLE iptv_recordings
    ADD COLUMN duration_seconds DOUBLE PRECISION,
    ADD COLUMN video_codec TEXT,
    ADD COLUMN container TEXT,
    ADD COLUMN thumbnails_generated BOOLEAN NOT NULL DEFAULT false;
