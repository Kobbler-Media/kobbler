-- Guide rows are pruned 24 h after they air, so a recording cannot join its way
-- back to the artwork it was scheduled from — it has to snapshot it.
ALTER TABLE livetv_recordings ADD COLUMN image_url TEXT;

UPDATE livetv_recordings r
SET image_url = COALESCE(g.image_url, g.poster_url)
FROM livetv_guide g
WHERE r.guide_id = g.id AND r.image_url IS NULL;
