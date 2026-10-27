-- Video ratings were halved onto a 0-5 star scale at ingest. They now keep their
-- providers' native 0-10 scale; books stay 0-5. The viewer's own ratings
-- (movie_watch_states, tv_watch_states, tv_show_ratings) are 1-5 stars and stay as they are.
UPDATE movies SET rating = rating * 2 WHERE rating IS NOT NULL;
UPDATE tv_shows SET rating = rating * 2 WHERE rating IS NOT NULL;
UPDATE stremio_continue_items SET rating = rating * 2 WHERE rating IS NOT NULL;

-- Cached provider lookups carry the halved value; enrichment reading one back would store it
-- halved again on the new scale.
DELETE FROM metadata_cache
WHERE query_key LIKE 'video\_movie:%'
   OR query_key LIKE 'video\_movie\_detail:%'
   OR query_key LIKE 'video\_tv:%'
   OR query_key LIKE 'video\_tv\_detail:%';

-- Discover shelves and For You lists are only refetched when they expire, so they are
-- rescaled in place. Book shelves share the 'discover' provider and keep their 0-5 scale.
CREATE FUNCTION double_video_item_ratings(items JSONB) RETURNS JSONB
LANGUAGE SQL IMMUTABLE AS $$
    SELECT COALESCE(
        jsonb_agg(
            CASE
                WHEN i->>'media_type' IN ('movie', 'tv') AND jsonb_typeof(i->'rating') = 'number'
                    THEN jsonb_set(i, '{rating}', to_jsonb((i->>'rating')::float8 * 2))
                ELSE i
            END
            ORDER BY ord
        ),
        '[]'::jsonb
    )
    FROM jsonb_array_elements(items) WITH ORDINALITY AS e(i, ord)
$$;

UPDATE metadata_cache
SET response = jsonb_set(response, '{items}', double_video_item_ratings(response->'items'))
WHERE provider_type = 'discover' AND jsonb_typeof(response->'items') = 'array';

UPDATE metadata_cache
SET response = response
    || jsonb_build_object(
        'recommendations', double_video_item_ratings(COALESCE(response->'recommendations', '[]')),
        'similar', double_video_item_ratings(COALESCE(response->'similar', '[]'))
    )
WHERE provider_type = 'tmdb_related';

DROP FUNCTION double_video_item_ratings(JSONB);
