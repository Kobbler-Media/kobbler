ALTER TABLE media_roots
    ADD COLUMN max_concurrent_io INT NOT NULL DEFAULT 1;

COMMENT ON COLUMN media_roots.max_concurrent_io IS
    'Maximum simultaneous heavy-IO jobs (thumbnails/skip_detect/extract) '
    'against this media root. Default 1 is safe for slow NAS. Bump to 2-4 '
    'for fast local SSD or modern NAS.';
