-- Re-normalize tv_shows.status to the canonical vocabulary.
-- 0095 did this once; every write path has since been re-pointed through
-- repositories::normalize_show_status, so this is the last backfill.
-- Canonical: "continuing", "ended", "canceled", "upcoming".

UPDATE tv_shows
SET status = 'continuing'
WHERE LOWER(status) IN ('returning series', 'continuing', 'running')
  AND status IS DISTINCT FROM 'continuing';

UPDATE tv_shows
SET status = 'ended'
WHERE LOWER(status) = 'ended'
  AND status IS DISTINCT FROM 'ended';

UPDATE tv_shows
SET status = 'canceled'
WHERE LOWER(status) IN ('canceled', 'cancelled')
  AND status IS DISTINCT FROM 'canceled';

UPDATE tv_shows
SET status = 'upcoming'
WHERE LOWER(status) IN ('upcoming', 'in production', 'planned', 'pilot', 'in development')
  AND status IS DISTINCT FROM 'upcoming';

-- Unknown provider vocabularies are lowercased rather than dropped, matching
-- normalize_show_status's passthrough arm.
UPDATE tv_shows
SET status = LOWER(status)
WHERE status IS NOT NULL
  AND status <> LOWER(status);
