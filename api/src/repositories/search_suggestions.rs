use serde::Serialize;
use sqlx::{Executor, Postgres};

use super::Result;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Suggestion {
    pub title: String,
    pub media_type: String,
    pub source: String,
}

pub struct SuggestionRequest<'a> {
    pub query: &'a str,
    pub media_types: &'a [String],
    pub kids: bool,
    pub live_tv: bool,
    pub iptv_languages: Option<&'a [String]>,
    pub book_titles: bool,
    pub limit: i64,
}

/// Titles completing `query` at the start of any word, those it starts outright first. Movies
/// and TV — the library's, the TMDB shelves the discover sync caches, and the TMDB export —
/// rank by TMDB popularity, so a popular show the query is half of beats an obscure library
/// film it spells out. Everything without a popularity, and books, which have no comparable
/// one, keep source order: the library's own titles, authors and channel lineups; then the
/// shelves and the live guide by soonest airing; then the export and, when enabled,
/// Hardcover's most-read books and authors. The two pools interleave rank by rank. A blank
/// query seeds from the shelves alone.
///
/// Kids: the kids rule on the library and only kids shelves. Exports carry no rating, so a
/// Kids profile gets them all except titles its library hides, which its search would only
/// come up empty on. Books are open to Kids; live channels and their guides are not.
///
/// The export and Hardcover matches repeat their trigram indexes' expressions verbatim.
pub async fn suggest<'e, E>(exec: E, request: &SuggestionRequest<'_>) -> Result<Vec<Suggestion>>
where
    E: Executor<'e, Database = Postgres>,
{
    let query = request.query.trim();
    let escaped = query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let starts = format!("{escaped}%");
    let word_starts = format!("% {escaped}%");

    // Runtime rather than `query_as!` for `persistent(false)`: a cached generic plan cannot
    // see the pattern, so it walks the popularity index even for rare words, scanning every
    // export title. A one-shot statement is planned against each query's own pattern.
    let rows = sqlx::query_as::<_, Suggestion>(
        r#"
        WITH candidates AS (
            SELECT m.title, 'movie' AS media_type, 'library' AS source, 0 AS shelf, 0::bigint AS pos,
                   (SELECT t.popularity FROM tmdb_titles t
                    WHERE t.media_type = 'movie' AND t.tmdb_id = CASE
                        WHEN m.identifiers->>'tmdb_id' ~ '^[0-9]{1,9}$' THEN (m.identifiers->>'tmdb_id')::int
                    END) AS popularity
            FROM movies m
            WHERE $1 <> '' AND 'movie' = ANY($2)
              AND (m.title ILIKE $3 OR m.title ILIKE $4)
              AND (NOT $5 OR kids_allowed(m.kids_override, m.content_rating))
            UNION ALL
            SELECT s.title, 'tv', 'library', 0, 0,
                   (SELECT t.popularity FROM tmdb_titles t
                    WHERE t.media_type = 'tv' AND t.tmdb_id = CASE
                        WHEN s.identifiers->>'tmdb_id' ~ '^[0-9]{1,9}$' THEN (s.identifiers->>'tmdb_id')::int
                    END)
            FROM tv_shows s
            WHERE $1 <> '' AND 'tv' = ANY($2)
              AND (s.title ILIKE $3 OR s.title ILIKE $4)
              AND (NOT $5 OR kids_allowed(s.kids_override, s.content_rating))
            UNION ALL
            SELECT * FROM (
                SELECT b.title, 'book', 'library', 0, 0::bigint, NULL::real
                FROM books b
                WHERE $1 <> '' AND 'book' = ANY($2) AND b.ingest_status IS NULL
                  AND (b.title ILIKE $3 OR b.title ILIKE $4)
                ORDER BY lower(b.title)
                LIMIT $6::bigint * 4
            ) library_books
            UNION ALL
            SELECT * FROM (
                SELECT a.name, 'author', 'library', 0, 0::bigint, NULL::real
                FROM authors a
                WHERE $1 <> '' AND 'book' = ANY($2)
                  AND (a.name ILIKE $3 OR a.name ILIKE $4)
                  AND EXISTS (
                      SELECT 1 FROM book_authors ba JOIN books b ON b.id = ba.book_id
                      WHERE ba.author_id = a.id AND b.ingest_status IS NULL
                  )
                ORDER BY lower(a.name)
                LIMIT $6::bigint * 4
            ) library_authors
            UNION ALL
            SELECT c.name, 'livetv', 'library', 0, COALESCE(c.lcn, 0)::bigint, NULL::real
            FROM (
                SELECT lcn, COALESCE(NULLIF(trim(display_name), ''), NULLIF(trim(scan_name), ''),
                                     NULLIF(trim(callsign), '')) AS name
                FROM livetv_channels
                WHERE $1 <> '' AND 'livetv' = ANY($2) AND $7 AND enabled
            ) c
            WHERE c.name ILIKE $3 OR c.name ILIKE $4
            UNION ALL
            SELECT * FROM (
                SELECT c.name, 'iptv', 'library', 0, c.display_order::bigint, NULL::real
                FROM iptv_channels c
                WHERE $1 <> '' AND 'iptv' = ANY($2) AND NOT $5 AND c.enabled
                  AND ($8::text[] IS NULL OR c.language IS NULL OR c.language = ANY($8))
                  AND (c.name ILIKE $3 OR c.name ILIKE $4)
                ORDER BY c.display_order, lower(c.name)
                LIMIT $6::bigint * 4
            ) iptv
            UNION ALL
            SELECT i.item->>'title', i.item->>'media_type', 'tmdb',
                   1 + COALESCE((mc.response->>'sort_order')::int, 0), i.pos, NULL::real
            FROM metadata_cache mc,
                 jsonb_array_elements(mc.response->'items') WITH ORDINALITY AS i(item, pos)
            WHERE mc.provider_type = 'discover' AND mc.expires_at > NOW()
              AND COALESCE((mc.response->>'kids')::boolean, FALSE) = $5
              AND i.item->>'media_type' = ANY($2)
              AND i.item->>'title' IS NOT NULL
              AND ($1 = '' OR i.item->>'title' ILIKE $3 OR i.item->>'title' ILIKE $4)
            UNION ALL
            SELECT g.title, 'livetv', 'guide', 1, extract(epoch FROM min(g.start_time))::bigint,
                   NULL::real
            FROM livetv_guide g
            JOIN livetv_channels c ON c.id = g.channel_id AND c.enabled
            WHERE $1 <> '' AND 'livetv' = ANY($2) AND $7
              AND g.end_time > NOW() AND g.start_time < NOW() + INTERVAL '48 hours'
              AND (g.title ILIKE $3 OR g.title ILIKE $4)
            GROUP BY g.title
            UNION ALL
            SELECT * FROM (
                SELECT COALESCE(t.title, t.original_title), t.media_type, 'tmdb', 1000,
                       row_number() OVER (ORDER BY t.popularity DESC), t.popularity
                FROM tmdb_titles t
                WHERE $1 <> '' AND t.media_type = ANY($2)
                  AND (lower(COALESCE(t.title || ' ', '') || t.original_title) LIKE lower($3)
                       OR lower(COALESCE(t.title || ' ', '') || t.original_title) LIKE lower($4))
                  AND NOT ($5 AND EXISTS (
                      SELECT 1 FROM movies m
                      WHERE t.media_type = 'movie' AND lower(m.title) = lower(COALESCE(t.title, t.original_title))
                        AND NOT kids_allowed(m.kids_override, m.content_rating)
                      UNION ALL
                      SELECT 1 FROM tv_shows s
                      WHERE t.media_type = 'tv' AND lower(s.title) = lower(COALESCE(t.title, t.original_title))
                        AND NOT kids_allowed(s.kids_override, s.content_rating)
                  ))
                ORDER BY t.popularity DESC
                LIMIT $6::bigint * 4
            ) popular
            UNION ALL
            SELECT title, media_type, 'hardcover', 1000,
                   row_number() OVER (ORDER BY users DESC), NULL::real
            FROM (
                SELECT * FROM (
                    SELECT h.title, 'book' AS media_type, h.users_count AS users
                    FROM hardcover_titles h
                    WHERE $1 <> '' AND 'book' = ANY($2) AND $9
                      AND (lower(h.title) LIKE lower($3) OR lower(h.title) LIKE lower($4))
                    ORDER BY h.users_count DESC
                    LIMIT $6::bigint * 4
                ) titles
                UNION ALL
                SELECT * FROM (
                    SELECT min(h.author), 'author', max(h.users_count)
                    FROM hardcover_titles h
                    WHERE $1 <> '' AND 'book' = ANY($2) AND $9
                      AND (lower(h.author) LIKE lower($3) OR lower(h.author) LIKE lower($4))
                    GROUP BY lower(h.author)
                    ORDER BY 3 DESC
                    LIMIT $6::bigint * 4
                ) authors
            ) most_read
        ),
        scored AS (
            SELECT title, media_type, source, shelf, pos,
                   max(popularity) OVER (PARTITION BY lower(title), media_type) AS popularity
            FROM candidates
        ),
        deduped AS (
            SELECT DISTINCT ON (lower(title)) title, media_type, source, shelf, pos, popularity,
                   CASE WHEN $1 = '' OR title ILIKE $3 THEN 0 ELSE 1 END AS tier,
                   media_type IN ('movie', 'tv') AS video
            FROM scored
            ORDER BY lower(title), shelf, pos
        ),
        ranked AS (
            SELECT title, media_type, source, shelf, pos, tier,
                   row_number() OVER (
                       PARTITION BY tier, video
                       ORDER BY popularity DESC NULLS LAST, shelf, pos, lower(title)
                   ) AS place
            FROM deduped
        )
        SELECT title, media_type, source
        FROM ranked
        ORDER BY tier, place, shelf, pos, lower(title)
        LIMIT $6
        "#,
    )
    .bind(query)
    .bind(request.media_types)
    .bind(starts)
    .bind(word_starts)
    .bind(request.kids)
    .bind(request.limit)
    .bind(request.live_tv && !request.kids)
    .bind(request.iptv_languages)
    .bind(request.book_titles)
    .persistent(false)
    .fetch_all(exec)
    .await?;

    Ok(rows)
}
