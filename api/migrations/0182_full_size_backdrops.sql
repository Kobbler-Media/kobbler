-- Backdrops cached before the TMDB provider moved to w1280 are 780px wide, which a TV
-- stretches to 1920. Clearing cached_at makes the image route discard the old file and
-- fetch the new source on next request.
UPDATE movies
SET images = jsonb_set(
    images #- '{backdrop,cached_at}',
    '{backdrop,source_url}',
    to_jsonb(replace(images->'backdrop'->>'source_url', '/t/p/w780/', '/t/p/w1280/'))
)
WHERE images->'backdrop'->>'source_url' LIKE '%/t/p/w780/%';

UPDATE tv_shows
SET images = jsonb_set(
    images #- '{backdrop,cached_at}',
    '{backdrop,source_url}',
    to_jsonb(replace(images->'backdrop'->>'source_url', '/t/p/w780/', '/t/p/w1280/'))
)
WHERE images->'backdrop'->>'source_url' LIKE '%/t/p/w780/%';
