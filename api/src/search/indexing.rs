//! Functions for indexing entities to search backends.

use super::backend::{SearchBackend, SearchError};
use super::{
    AuthorDocument, BookDocument, GenreDocument, IptvChannelDocument, IptvEpgDocument,
    LivetvChannelDocument, LivetvGuideDocument, MediaRootFileDocument, MovieDocument,
    PersonDocument, SeriesDocument, TranscriptSegmentDocument, TvEpisodeDocument, TvSeasonDocument,
    TvShowDocument, compute_first_letter, truncate_description,
};
use sqlx::PgPool;
use uuid::Uuid;

/// Index a single book by ID.
///
/// Fetches the book data from the database and indexes it using the search backend.
pub async fn index_book(
    search: &dyn SearchBackend,
    pool: &PgPool,
    book_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query_as!(
        BookRow,
        r#"
        SELECT
            b.id,
            b.title,
            COALESCE(
                (SELECT array_agg(a.name ORDER BY a.name)
                 FROM book_authors ba
                 JOIN authors a ON a.id = ba.author_id
                 WHERE ba.book_id = b.id),
                ARRAY[]::text[]
            ) as "authors!: Vec<String>",
            COALESCE(
                (SELECT array_agg(ba.author_id::text)
                 FROM book_authors ba
                 WHERE ba.book_id = b.id),
                ARRAY[]::text[]
            ) as "author_ids!: Vec<String>",
            s.name as "series_name?",
            bs.series_id::text as "series_id?",
            bs.series_index,
            COALESCE(
                (SELECT array_agg(g.name ORDER BY g.name)
                 FROM book_genres bg
                 JOIN genres g ON g.id = bg.genre_id
                 WHERE bg.book_id = b.id),
                ARRAY[]::text[]
            ) as "genres!: Vec<String>",
            CASE WHEN (b.images->'cover'->>'source_url' IS NOT NULL OR b.images->'cover'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/library/books/' || b.id || '/cover'
                 ELSE NULL
            END as cover_url,
            (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)) AS "is_in_library!",
            COALESCE(b.want_status, 'none') as "want_status!",
            COALESCE(b.sort_title, b.title) as "sort_title!",
            b.ingest_status,
            b.narrator,
            EXISTS (
                SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id
            ) as "has_audiobook!",
            EXISTS (
                SELECT 1 FROM book_files bf WHERE bf.book_id = b.id
            ) as "has_ebook!",
            (SELECT string_agg(s2.name, ' | ')
             FROM book_series bs2
             JOIN series s2 ON s2.id = bs2.series_id
             WHERE bs2.book_id = b.id
            ) as all_series_names,
            b.description
        FROM books b
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        LEFT JOIN series s ON s.id = bs.series_id
        WHERE b.id = $1
        "#,
        book_id,
    )
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        // Book not found, might have been deleted - remove from index
        let _ = search.delete_book(book_id).await;
        return Ok(());
    };

    let first_letter = BookDocument::compute_first_letter(&row.sort_title);
    let doc = BookDocument {
        id: row.id.to_string(),
        title: row.title,
        authors: row.authors,
        author_ids: row.author_ids,
        series_name: row.series_name,
        series_id: row.series_id,
        series_index: row.series_index,
        genres: row.genres,
        cover_url: row.cover_url,
        is_in_library: row.is_in_library,
        want_status: row.want_status,
        first_letter,
        ingest_status: row.ingest_status,
        has_audiobook: row.has_audiobook,
        has_ebook: row.has_ebook,
        narrator: row.narrator,
        all_series_names: row.all_series_names,
        description: truncate_description(row.description),
    };

    search.index_book(doc).await?;

    Ok(())
}

struct BookRow {
    id: Uuid,
    title: String,
    authors: Vec<String>,
    author_ids: Vec<String>,
    series_name: Option<String>,
    series_id: Option<String>,
    series_index: Option<f64>,
    genres: Vec<String>,
    cover_url: Option<String>,
    is_in_library: bool,
    want_status: String,
    sort_title: String,
    ingest_status: Option<String>,
    narrator: Option<String>,
    has_audiobook: bool,
    has_ebook: bool,
    all_series_names: Option<String>,
    description: Option<String>,
}

/// Index a single author by ID.
///
/// Fetches the author data from the database and indexes it using the search backend.
pub async fn index_author(
    search: &dyn SearchBackend,
    pool: &PgPool,
    author_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query_as!(
        AuthorRow,
        r#"
        SELECT
            a.id,
            a.name,
            CASE WHEN (a.images->'profile'->>'source_url' IS NOT NULL OR a.images->'profile'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/library/authors/' || a.id || '/image'
                 ELSE NULL
            END as image_url,
            COALESCE(
                (SELECT COUNT(DISTINCT ba.book_id)
                 FROM book_authors ba
                 JOIN books b ON b.id = ba.book_id
                 WHERE ba.author_id = a.id AND (EXISTS (SELECT 1 FROM book_files bf2 WHERE bf2.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af2 WHERE af2.book_id = b.id))),
                0
            ) as "book_count!",
            a.is_monitored
        FROM authors a
        WHERE a.id = $1
        "#,
        author_id,
    )
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        // Author not found, might have been deleted - remove from index
        let _ = search.delete_author(author_id).await;
        return Ok(());
    };

    let doc = AuthorDocument {
        id: row.id.to_string(),
        name: row.name,
        image_url: row.image_url,
        book_count: row.book_count,
        is_monitored: row.is_monitored,
    };

    search.index_author(doc).await?;

    Ok(())
}

struct AuthorRow {
    id: Uuid,
    name: String,
    image_url: Option<String>,
    book_count: i64,
    is_monitored: bool,
}

/// Index a single series by ID.
///
/// Fetches the series data from the database and indexes it using the search backend.
pub async fn index_series(
    search: &dyn SearchBackend,
    pool: &PgPool,
    series_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query_as!(
        SeriesRow,
        r#"
        SELECT
            s.id,
            s.name,
            COALESCE(
                (SELECT COUNT(*) FROM book_series bs JOIN books b ON b.id = bs.book_id WHERE bs.series_id = s.id AND (EXISTS (SELECT 1 FROM book_files bf2 WHERE bf2.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af2 WHERE af2.book_id = b.id))),
                0
            ) as "book_count!",
            s.is_monitored
        FROM series s
        WHERE s.id = $1
        "#,
        series_id,
    )
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        // Series not found, might have been deleted - remove from index
        let _ = search.delete_series(series_id).await;
        return Ok(());
    };

    let doc = SeriesDocument {
        id: row.id.to_string(),
        name: row.name,
        book_count: row.book_count,
        is_monitored: row.is_monitored,
    };

    search.index_series(doc).await?;

    Ok(())
}

struct SeriesRow {
    id: Uuid,
    name: String,
    book_count: i64,
    is_monitored: bool,
}

/// Fetch all book documents from the database for reindexing.
pub async fn fetch_all_book_docs(
    pool: &PgPool,
) -> Result<Vec<BookDocument>, Box<dyn std::error::Error + Send + Sync>> {
    let rows = sqlx::query_as!(
        BookRow,
        r#"
        SELECT
            b.id as "id!",
            b.title as "title!",
            COALESCE(
                (SELECT array_agg(a.name ORDER BY a.name)
                 FROM book_authors ba
                 JOIN authors a ON a.id = ba.author_id
                 WHERE ba.book_id = b.id),
                ARRAY[]::text[]
            ) as "authors!: Vec<String>",
            COALESCE(
                (SELECT array_agg(ba.author_id::text)
                 FROM book_authors ba
                 WHERE ba.book_id = b.id),
                ARRAY[]::text[]
            ) as "author_ids!: Vec<String>",
            s.name as "series_name?",
            bs.series_id::text as "series_id?",
            bs.series_index as "series_index?",
            COALESCE(
                (SELECT array_agg(g.name ORDER BY g.name)
                 FROM book_genres bg
                 JOIN genres g ON g.id = bg.genre_id
                 WHERE bg.book_id = b.id),
                ARRAY[]::text[]
            ) as "genres!: Vec<String>",
            CASE WHEN (b.images->'cover'->>'source_url' IS NOT NULL OR b.images->'cover'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/library/books/' || b.id || '/cover'
                 ELSE NULL
            END as cover_url,
            (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)) AS "is_in_library!",
            COALESCE(b.want_status, 'none') as "want_status!",
            COALESCE(b.sort_title, b.title) as "sort_title!",
            b.ingest_status,
            b.narrator,
            EXISTS (
                SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id
            ) as "has_audiobook!",
            EXISTS (
                SELECT 1 FROM book_files bf WHERE bf.book_id = b.id
            ) as "has_ebook!",
            (SELECT string_agg(s2.name, ' | ')
             FROM book_series bs2
             JOIN series s2 ON s2.id = bs2.series_id
             WHERE bs2.book_id = b.id
            ) as "all_series_names?",
            b.description
        FROM books b
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        LEFT JOIN series s ON s.id = bs.series_id
        "#,
    )
    .fetch_all(pool)
    .await?;

    let docs = rows
        .into_iter()
        .map(|row| {
            let first_letter = BookDocument::compute_first_letter(&row.sort_title);
            BookDocument {
                id: row.id.to_string(),
                title: row.title,
                authors: row.authors,
                author_ids: row.author_ids,
                series_name: row.series_name,
                series_id: row.series_id,
                series_index: row.series_index,
                genres: row.genres,
                cover_url: row.cover_url,
                is_in_library: row.is_in_library,
                want_status: row.want_status,
                first_letter,
                ingest_status: row.ingest_status,
                has_audiobook: row.has_audiobook,
                has_ebook: row.has_ebook,
                narrator: row.narrator,
                all_series_names: row.all_series_names,
                description: truncate_description(row.description),
            }
        })
        .collect();

    Ok(docs)
}

/// Fetch all author documents from the database for reindexing.
pub async fn fetch_all_author_docs(
    pool: &PgPool,
) -> Result<Vec<AuthorDocument>, Box<dyn std::error::Error + Send + Sync>> {
    let rows = sqlx::query_as!(
        AuthorRow,
        r#"
        SELECT
            a.id,
            a.name,
            CASE WHEN (a.images->'profile'->>'source_url' IS NOT NULL OR a.images->'profile'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/library/authors/' || a.id || '/image'
                 ELSE NULL
            END as image_url,
            COALESCE(
                (SELECT COUNT(DISTINCT ba.book_id)
                 FROM book_authors ba
                 JOIN books b ON b.id = ba.book_id
                 WHERE ba.author_id = a.id AND (EXISTS (SELECT 1 FROM book_files bf2 WHERE bf2.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af2 WHERE af2.book_id = b.id))),
                0
            ) as "book_count!",
            a.is_monitored
        FROM authors a
        "#,
    )
    .fetch_all(pool)
    .await?;

    let docs = rows
        .into_iter()
        .map(|row| AuthorDocument {
            id: row.id.to_string(),
            name: row.name,
            image_url: row.image_url,
            book_count: row.book_count,
            is_monitored: row.is_monitored,
        })
        .collect();

    Ok(docs)
}

/// Fetch all series documents from the database for reindexing.
pub async fn fetch_all_series_docs(
    pool: &PgPool,
) -> Result<Vec<SeriesDocument>, Box<dyn std::error::Error + Send + Sync>> {
    let rows = sqlx::query_as!(
        SeriesRow,
        r#"
        SELECT
            s.id,
            s.name,
            COALESCE(
                (SELECT COUNT(*) FROM book_series bs JOIN books b ON b.id = bs.book_id WHERE bs.series_id = s.id AND (EXISTS (SELECT 1 FROM book_files bf2 WHERE bf2.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af2 WHERE af2.book_id = b.id))),
                0
            ) as "book_count!",
            s.is_monitored
        FROM series s
        "#,
    )
    .fetch_all(pool)
    .await?;

    let docs = rows
        .into_iter()
        .map(|row| SeriesDocument {
            id: row.id.to_string(),
            name: row.name,
            book_count: row.book_count,
            is_monitored: row.is_monitored,
        })
        .collect();

    Ok(docs)
}

/// Fetch all genre documents from the database for reindexing.
pub async fn fetch_all_genre_docs(
    pool: &PgPool,
) -> Result<Vec<GenreDocument>, Box<dyn std::error::Error + Send + Sync>> {
    let rows = sqlx::query_as!(
        GenreRow,
        r#"
        SELECT
            g.id,
            g.name,
            COALESCE(
                (SELECT COUNT(DISTINCT bg.book_id)
                 FROM book_genres bg
                 JOIN books b ON b.id = bg.book_id
                 WHERE bg.genre_id = g.id AND (EXISTS (SELECT 1 FROM book_files bf2 WHERE bf2.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af2 WHERE af2.book_id = b.id))),
                0
            ) as "book_count!"
        FROM genres g
        "#,
    )
    .fetch_all(pool)
    .await?;

    let docs = rows
        .into_iter()
        .map(|row| GenreDocument {
            id: row.id.to_string(),
            name: row.name,
            book_count: row.book_count,
        })
        .collect();

    Ok(docs)
}

struct GenreRow {
    id: Uuid,
    name: String,
    book_count: i64,
}

/// Meili range filters and sorts want a number, so an air date is carried both ways:
/// the ISO string for display and midnight-UTC seconds for filtering.
fn iso_date(d: time::Date) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())
}

fn unix_seconds(d: time::Date) -> i64 {
    d.midnight().assume_utc().unix_timestamp()
}

/// Index a single movie by ID.
///
/// Fetches the movie data from the database and indexes it using the search backend.
pub async fn index_movie(
    search: &dyn SearchBackend,
    pool: &PgPool,
    movie_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query_as!(
        MovieRow,
        r#"
        SELECT
            m.id,
            m.title,
            m.year,
            CASE WHEN (m.images->'poster'->>'source_url' IS NOT NULL OR m.images->'poster'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/movies/' || m.id || '/poster'
                 ELSE NULL
            END as poster_url,
            EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = m.id) AS "is_in_library!",
            COALESCE(m.want_status, 'none') as "want_status!",
            COALESCE(m.sort_title, m.title) as "sort_title!",
            COALESCE(m.genres, ARRAY[]::text[]) as "genres!: Vec<String>",
            COALESCE(
                (SELECT array_agg(p.name ORDER BY p.name)
                 FROM movie_crew mc
                 JOIN people p ON p.id = mc.person_id
                 WHERE mc.movie_id = m.id AND mc.department = 'Directing'),
                ARRAY[]::text[]
            ) as "directors!: Vec<String>",
            m.rating,
            m.runtime,
            m.content_rating,
            m.description
        FROM movies m
        WHERE m.id = $1
        "#,
        movie_id,
    )
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        // Movie not found, might have been deleted - remove from index
        let _ = search.delete_movie(movie_id).await;
        return Ok(());
    };

    let genres = row.genres;
    let first_letter = compute_first_letter(&row.sort_title);

    let doc = MovieDocument {
        id: row.id.to_string(),
        title: row.title,
        year: row.year,
        directors: row.directors,
        genres,
        poster_url: row.poster_url,
        is_in_library: row.is_in_library,
        want_status: row.want_status,
        first_letter,
        rating: row.rating,
        runtime: row.runtime,
        content_rating: row.content_rating,
        description: truncate_description(row.description),
    };

    search.index_movie(doc).await?;

    Ok(())
}

struct MovieRow {
    id: Uuid,
    title: String,
    year: Option<i32>,
    poster_url: Option<String>,
    is_in_library: bool,
    want_status: String,
    sort_title: String,
    genres: Vec<String>,
    directors: Vec<String>,
    rating: Option<f64>,
    runtime: Option<i32>,
    content_rating: Option<String>,
    description: Option<String>,
}

/// Index a single TV show by ID.
///
/// Fetches the TV show data from the database and indexes it using the search backend.
pub async fn index_tv_show(
    search: &dyn SearchBackend,
    pool: &PgPool,
    tv_show_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query_as!(
        TvShowRow,
        r#"
        SELECT
            s.id,
            s.title,
            s.year,
            CASE WHEN (s.images->'poster'->>'source_url' IS NOT NULL OR s.images->'poster'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/tv/shows/' || s.id || '/poster'
                 ELSE NULL
            END as poster_url,
            s.is_monitored,
            s.status,
            s.network,
            COALESCE(s.sort_title, s.title) as "sort_title!",
            COALESCE(s.genres, ARRAY[]::text[]) as "genres!: Vec<String>",
            COALESCE((
                SELECT COUNT(*)::int
                FROM tv_episodes te
                JOIN tv_seasons tsn ON tsn.id = te.season_id
                WHERE tsn.show_id = s.id
            ), 0) AS "total_episodes!",
            COALESCE((
                SELECT COUNT(DISTINCT te.id)::int
                FROM tv_episodes te
                JOIN tv_seasons tsn ON tsn.id = te.season_id
                JOIN tv_episode_file_episodes tefe ON tefe.episode_id = te.id
                JOIN tv_episode_files tef ON tef.id = tefe.episode_file_id AND tef.available = true
                WHERE tsn.show_id = s.id
            ), 0) AS "available_episode_count!",
            s.rating,
            s.content_rating,
            s.runtime,
            s.description
        FROM tv_shows s
        WHERE s.id = $1
        "#,
        tv_show_id,
    )
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        // TV show not found, might have been deleted - remove from index
        let _ = search.delete_tv_show(tv_show_id).await;
        return Ok(());
    };

    let genres = row.genres;
    let first_letter = compute_first_letter(&row.sort_title);

    let doc = TvShowDocument {
        id: row.id.to_string(),
        title: row.title,
        year: row.year,
        genres,
        poster_url: row.poster_url,
        is_monitored: row.is_monitored,
        status: row.status,
        network: row.network,
        first_letter,
        total_episodes: row.total_episodes,
        available_episode_count: row.available_episode_count,
        rating: row.rating,
        content_rating: row.content_rating,
        runtime: row.runtime,
        description: truncate_description(row.description),
    };

    search.index_tv_show(doc).await?;

    Ok(())
}

struct TvShowRow {
    id: Uuid,
    title: String,
    year: Option<i32>,
    poster_url: Option<String>,
    is_monitored: bool,
    status: Option<String>,
    network: Option<String>,
    sort_title: String,
    genres: Vec<String>,
    total_episodes: i32,
    available_episode_count: i32,
    rating: Option<f64>,
    content_rating: Option<String>,
    runtime: Option<i32>,
    description: Option<String>,
}

/// Fetch all movie documents from the database for reindexing.
pub async fn fetch_all_movie_docs(
    pool: &PgPool,
) -> Result<Vec<MovieDocument>, Box<dyn std::error::Error + Send + Sync>> {
    let rows = sqlx::query_as!(
        MovieRow,
        r#"
        SELECT
            m.id,
            m.title,
            m.year,
            CASE WHEN (m.images->'poster'->>'source_url' IS NOT NULL OR m.images->'poster'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/movies/' || m.id || '/poster'
                 ELSE NULL
            END as poster_url,
            EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = m.id) AS "is_in_library!",
            COALESCE(m.want_status, 'none') as "want_status!",
            COALESCE(m.sort_title, m.title) as "sort_title!",
            COALESCE(m.genres, ARRAY[]::text[]) as "genres!: Vec<String>",
            COALESCE(
                (SELECT array_agg(p.name ORDER BY p.name)
                 FROM movie_crew mc
                 JOIN people p ON p.id = mc.person_id
                 WHERE mc.movie_id = m.id AND mc.department = 'Directing'),
                ARRAY[]::text[]
            ) as "directors!: Vec<String>",
            m.rating,
            m.runtime,
            m.content_rating,
            m.description
        FROM movies m
        "#,
    )
    .fetch_all(pool)
    .await?;

    let docs = rows
        .into_iter()
        .map(|row| {
            let genres = row.genres;
            let first_letter = compute_first_letter(&row.sort_title);
            MovieDocument {
                id: row.id.to_string(),
                title: row.title,
                year: row.year,
                directors: row.directors,
                genres,
                poster_url: row.poster_url,
                is_in_library: row.is_in_library,
                want_status: row.want_status,
                first_letter,
                rating: row.rating,
                runtime: row.runtime,
                content_rating: row.content_rating,
                description: truncate_description(row.description),
            }
        })
        .collect();

    Ok(docs)
}

/// Fetch all TV show documents from the database for reindexing.
pub async fn fetch_all_tv_show_docs(
    pool: &PgPool,
) -> Result<Vec<TvShowDocument>, Box<dyn std::error::Error + Send + Sync>> {
    let rows = sqlx::query_as!(
        TvShowRow,
        r#"
        SELECT
            s.id,
            s.title,
            s.year,
            CASE WHEN (s.images->'poster'->>'source_url' IS NOT NULL OR s.images->'poster'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/tv/shows/' || s.id || '/poster'
                 ELSE NULL
            END as poster_url,
            s.is_monitored,
            s.status,
            s.network,
            COALESCE(s.sort_title, s.title) as "sort_title!",
            COALESCE(s.genres, ARRAY[]::text[]) as "genres!: Vec<String>",
            COALESCE((
                SELECT COUNT(*)::int
                FROM tv_episodes te
                JOIN tv_seasons tsn ON tsn.id = te.season_id
                WHERE tsn.show_id = s.id
            ), 0) AS "total_episodes!",
            COALESCE((
                SELECT COUNT(DISTINCT te.id)::int
                FROM tv_episodes te
                JOIN tv_seasons tsn ON tsn.id = te.season_id
                JOIN tv_episode_file_episodes tefe ON tefe.episode_id = te.id
                JOIN tv_episode_files tef ON tef.id = tefe.episode_file_id AND tef.available = true
                WHERE tsn.show_id = s.id
            ), 0) AS "available_episode_count!",
            s.rating,
            s.content_rating,
            s.runtime,
            s.description
        FROM tv_shows s
        "#,
    )
    .fetch_all(pool)
    .await?;

    let docs = rows
        .into_iter()
        .map(|row| {
            let genres = row.genres;
            let first_letter = compute_first_letter(&row.sort_title);
            TvShowDocument {
                id: row.id.to_string(),
                title: row.title,
                year: row.year,
                genres,
                poster_url: row.poster_url,
                is_monitored: row.is_monitored,
                status: row.status,
                network: row.network,
                first_letter,
                total_episodes: row.total_episodes,
                available_episode_count: row.available_episode_count,
                rating: row.rating,
                content_rating: row.content_rating,
                runtime: row.runtime,
                description: truncate_description(row.description),
            }
        })
        .collect();

    Ok(docs)
}

/// Build a padded season code like "S02" from a season identifier string.
fn season_code(identifier: &str) -> String {
    let num: i32 = identifier.parse().unwrap_or(0);
    format!("S{num:02}")
}

/// Build a padded episode code like "S02E05" from season and episode identifier strings.
fn episode_code(season_identifier: &str, episode_identifier: &str) -> String {
    let s: i32 = season_identifier.parse().unwrap_or(0);
    let e: i32 = episode_identifier.parse().unwrap_or(0);
    format!("S{s:02}E{e:02}")
}

/// Index a single TV season by ID.
pub async fn index_tv_season(
    search: &dyn SearchBackend,
    pool: &PgPool,
    season_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query_as!(
        TvSeasonRow,
        r#"
        SELECT
            ts.id,
            s.id AS show_id,
            s.title AS show_title,
            ts.season_identifier,
            ts.title AS season_title,
            ts.is_monitored,
            ts.is_special,
            CASE WHEN (ts.images->'poster'->>'source_url' IS NOT NULL OR ts.images->'poster'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/tv/seasons/' || ts.id || '/poster'
                 WHEN (s.images->'poster'->>'source_url' IS NOT NULL OR s.images->'poster'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/tv/shows/' || s.id || '/poster'
                 ELSE NULL
            END AS poster_url,
            COALESCE((
                SELECT COUNT(*)::int
                FROM tv_episodes te
                WHERE te.season_id = ts.id
            ), 0) AS "episode_count!"
        FROM tv_seasons ts
        JOIN tv_shows s ON s.id = ts.show_id
        WHERE ts.id = $1
        "#,
        season_id,
    )
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        let _ = search.delete_tv_season(season_id).await;
        return Ok(());
    };

    search.index_tv_season(tv_season_row_to_doc(row)).await?;
    Ok(())
}

fn tv_season_row_to_doc(row: TvSeasonRow) -> TvSeasonDocument {
    let code = season_code(&row.season_identifier);
    let display_title = if let Some(ref title) = row.season_title {
        format!("{} {} ({})", row.show_title, title, code)
    } else {
        format!("{} Season {}", row.show_title, row.season_identifier)
    };
    TvSeasonDocument {
        id: row.id.to_string(),
        show_id: row.show_id.to_string(),
        show_title: row.show_title,
        display_title,
        season_title: row.season_title,
        season_identifier: row.season_identifier,
        season_code: code,
        poster_url: row.poster_url,
        episode_count: row.episode_count,
        is_monitored: row.is_monitored,
        is_special: row.is_special,
    }
}

struct TvSeasonRow {
    id: Uuid,
    show_id: Uuid,
    show_title: String,
    season_identifier: String,
    season_title: Option<String>,
    is_monitored: bool,
    is_special: bool,
    poster_url: Option<String>,
    episode_count: i32,
}

/// Fetch all TV season documents from the database for reindexing.
pub async fn fetch_all_tv_season_docs(
    pool: &PgPool,
) -> Result<Vec<TvSeasonDocument>, Box<dyn std::error::Error + Send + Sync>> {
    let rows = sqlx::query_as!(
        TvSeasonRow,
        r#"
        SELECT
            ts.id,
            s.id AS show_id,
            s.title AS show_title,
            ts.season_identifier,
            ts.title AS season_title,
            ts.is_monitored,
            ts.is_special,
            CASE WHEN (ts.images->'poster'->>'source_url' IS NOT NULL OR ts.images->'poster'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/tv/seasons/' || ts.id || '/poster'
                 WHEN (s.images->'poster'->>'source_url' IS NOT NULL OR s.images->'poster'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/tv/shows/' || s.id || '/poster'
                 ELSE NULL
            END AS poster_url,
            COALESCE((
                SELECT COUNT(*)::int
                FROM tv_episodes te
                WHERE te.season_id = ts.id
            ), 0) AS "episode_count!"
        FROM tv_seasons ts
        JOIN tv_shows s ON s.id = ts.show_id
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(tv_season_row_to_doc).collect())
}

/// Index a single TV episode by ID.
pub async fn index_tv_episode(
    search: &dyn SearchBackend,
    pool: &PgPool,
    episode_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query_as!(
        TvEpisodeRow,
        r#"
        SELECT
            te.id,
            s.id AS show_id,
            ts.id AS season_id,
            s.title AS show_title,
            ts.season_identifier,
            te.episode_identifier,
            te.title AS episode_title,
            EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = te.id) AS "is_in_library!",
            COALESCE(te.want_status, 'none') AS "want_status!",
            CASE WHEN (te.images->'still'->>'source_url' IS NOT NULL OR te.images->'still'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/tv/episodes/' || te.id || '/still'
                 ELSE NULL
            END AS still_url,
            te.air_date,
            ts.season_sort_order,
            te.episode_sort_order,
            te.runtime,
            te.description
        FROM tv_episodes te
        JOIN tv_seasons ts ON ts.id = te.season_id
        JOIN tv_shows s ON s.id = ts.show_id
        WHERE te.id = $1
        "#,
        episode_id,
    )
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        let _ = search.delete_tv_episode(episode_id).await;
        return Ok(());
    };

    search.index_tv_episode(tv_episode_row_to_doc(row)).await?;
    Ok(())
}

fn tv_episode_row_to_doc(row: TvEpisodeRow) -> TvEpisodeDocument {
    let code = episode_code(&row.season_identifier, &row.episode_identifier);
    let display_title = match &row.episode_title {
        Some(title) if !title.is_empty() => format!("{} {} {}", row.show_title, code, title),
        _ => format!("{} {}", row.show_title, code),
    };
    TvEpisodeDocument {
        id: row.id.to_string(),
        show_id: row.show_id.to_string(),
        season_id: row.season_id.to_string(),
        show_title: row.show_title,
        display_title,
        episode_title: row.episode_title,
        episode_code: code,
        still_url: row.still_url,
        is_in_library: row.is_in_library,
        want_status: row.want_status,
        air_date: row.air_date.map(iso_date),
        air_date_ts: row.air_date.map(unix_seconds),
        season_sort_order: row.season_sort_order,
        episode_sort_order: row.episode_sort_order,
        runtime: row.runtime,
        description: truncate_description(row.description),
    }
}

struct TvEpisodeRow {
    id: Uuid,
    show_id: Uuid,
    season_id: Uuid,
    show_title: String,
    season_identifier: String,
    episode_identifier: String,
    episode_title: Option<String>,
    is_in_library: bool,
    want_status: String,
    still_url: Option<String>,
    air_date: Option<time::Date>,
    season_sort_order: i32,
    episode_sort_order: i32,
    runtime: Option<i32>,
    description: Option<String>,
}

/// Fetch all TV episode documents from the database for reindexing.
pub async fn fetch_all_tv_episode_docs(
    pool: &PgPool,
) -> Result<Vec<TvEpisodeDocument>, Box<dyn std::error::Error + Send + Sync>> {
    let rows = sqlx::query_as!(
        TvEpisodeRow,
        r#"
        SELECT
            te.id,
            s.id AS show_id,
            ts.id AS season_id,
            s.title AS show_title,
            ts.season_identifier,
            te.episode_identifier,
            te.title AS episode_title,
            EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = te.id) AS "is_in_library!",
            COALESCE(te.want_status, 'none') AS "want_status!",
            CASE WHEN (te.images->'still'->>'source_url' IS NOT NULL OR te.images->'still'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/tv/episodes/' || te.id || '/still'
                 ELSE NULL
            END AS still_url,
            te.air_date,
            ts.season_sort_order,
            te.episode_sort_order,
            te.runtime,
            te.description
        FROM tv_episodes te
        JOIN tv_seasons ts ON ts.id = te.season_id
        JOIN tv_shows s ON s.id = ts.show_id
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(tv_episode_row_to_doc).collect())
}

/// Re-index every season and episode of one show, two batched writes in total.
///
/// A show update changes the `display_title` of all its children. Indexing them
/// one job per entity sent hundreds of single-document writes to Meilisearch in
/// a burst for long-running shows.
pub async fn index_tv_show_children(
    search: &dyn SearchBackend,
    pool: &PgPool,
    show_id: Uuid,
) -> Result<(usize, usize), Box<dyn std::error::Error + Send + Sync>> {
    let season_rows = sqlx::query_as!(
        TvSeasonRow,
        r#"
        SELECT
            ts.id,
            s.id AS show_id,
            s.title AS show_title,
            ts.season_identifier,
            ts.title AS season_title,
            ts.is_monitored,
            ts.is_special,
            CASE WHEN (ts.images->'poster'->>'source_url' IS NOT NULL OR ts.images->'poster'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/tv/seasons/' || ts.id || '/poster'
                 WHEN (s.images->'poster'->>'source_url' IS NOT NULL OR s.images->'poster'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/tv/shows/' || s.id || '/poster'
                 ELSE NULL
            END AS poster_url,
            COALESCE((
                SELECT COUNT(*)::int
                FROM tv_episodes te
                WHERE te.season_id = ts.id
            ), 0) AS "episode_count!"
        FROM tv_seasons ts
        JOIN tv_shows s ON s.id = ts.show_id
        WHERE s.id = $1
        "#,
        show_id,
    )
    .fetch_all(pool)
    .await?;

    let episode_rows = sqlx::query_as!(
        TvEpisodeRow,
        r#"
        SELECT
            te.id,
            s.id AS show_id,
            ts.id AS season_id,
            s.title AS show_title,
            ts.season_identifier,
            te.episode_identifier,
            te.title AS episode_title,
            EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = te.id) AS "is_in_library!",
            COALESCE(te.want_status, 'none') AS "want_status!",
            CASE WHEN (te.images->'still'->>'source_url' IS NOT NULL OR te.images->'still'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/tv/episodes/' || te.id || '/still'
                 ELSE NULL
            END AS still_url,
            te.air_date,
            ts.season_sort_order,
            te.episode_sort_order,
            te.runtime,
            te.description
        FROM tv_episodes te
        JOIN tv_seasons ts ON ts.id = te.season_id
        JOIN tv_shows s ON s.id = ts.show_id
        WHERE s.id = $1
        "#,
        show_id,
    )
    .fetch_all(pool)
    .await?;

    let seasons: Vec<_> = season_rows.into_iter().map(tv_season_row_to_doc).collect();
    let episodes: Vec<_> = episode_rows
        .into_iter()
        .map(tv_episode_row_to_doc)
        .collect();
    let counts = (seasons.len(), episodes.len());

    search.index_tv_seasons_batch(seasons).await?;
    search.index_tv_episodes_batch(episodes).await?;
    Ok(counts)
}

/// Index a single person by ID.
///
/// Fetches the person data from the database and indexes it using the search backend.
pub async fn index_person(
    search: &dyn SearchBackend,
    pool: &PgPool,
    person_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query_as!(
        PersonRow,
        r#"
        SELECT
            p.id,
            p.name,
            p.sort_name,
            CASE WHEN (p.images->'profile'->>'source_url' IS NOT NULL OR p.images->'profile'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/people/' || p.id || '/image'
                 ELSE NULL
            END as image_url,
            LEFT(p.bio, 500) as bio,
            COALESCE(
                (SELECT COUNT(DISTINCT mc.movie_id) FROM movie_cast mc WHERE mc.person_id = p.id)
                + (SELECT COUNT(DISTINCT mcr.movie_id) FROM movie_crew mcr WHERE mcr.person_id = p.id),
                0
            ) as "movie_count!",
            COALESCE(
                (SELECT COUNT(DISTINCT tc.show_id) FROM tv_cast tc WHERE tc.person_id = p.id)
                + (SELECT COUNT(DISTINCT tcr.show_id) FROM tv_crew tcr WHERE tcr.person_id = p.id),
                0
            ) as "tv_show_count!"
        FROM people p
        WHERE p.id = $1
        "#,
        person_id,
    )
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        let _ = search.delete_person(person_id).await;
        return Ok(());
    };

    let doc = PersonDocument {
        id: row.id.to_string(),
        name: row.name,
        sort_name: row.sort_name,
        image_url: row.image_url,
        bio: row.bio,
        movie_count: row.movie_count,
        tv_show_count: row.tv_show_count,
    };

    search.index_person(doc).await?;

    Ok(())
}

struct PersonRow {
    id: Uuid,
    name: String,
    sort_name: Option<String>,
    image_url: Option<String>,
    bio: Option<String>,
    movie_count: i64,
    tv_show_count: i64,
}

/// Fetch all person documents from the database for reindexing.
pub async fn fetch_all_people_docs(
    pool: &PgPool,
) -> Result<Vec<PersonDocument>, Box<dyn std::error::Error + Send + Sync>> {
    let rows = sqlx::query_as!(
        PersonRow,
        r#"
        SELECT
            p.id,
            p.name,
            p.sort_name,
            CASE WHEN (p.images->'profile'->>'source_url' IS NOT NULL OR p.images->'profile'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/people/' || p.id || '/image'
                 ELSE NULL
            END as image_url,
            LEFT(p.bio, 500) as bio,
            COALESCE(
                (SELECT COUNT(DISTINCT mc.movie_id) FROM movie_cast mc WHERE mc.person_id = p.id)
                + (SELECT COUNT(DISTINCT mcr.movie_id) FROM movie_crew mcr WHERE mcr.person_id = p.id),
                0
            ) as "movie_count!",
            COALESCE(
                (SELECT COUNT(DISTINCT tc.show_id) FROM tv_cast tc WHERE tc.person_id = p.id)
                + (SELECT COUNT(DISTINCT tcr.show_id) FROM tv_crew tcr WHERE tcr.person_id = p.id),
                0
            ) as "tv_show_count!"
        FROM people p
        "#,
    )
    .fetch_all(pool)
    .await?;

    let docs = rows
        .into_iter()
        .map(|row| PersonDocument {
            id: row.id.to_string(),
            name: row.name,
            sort_name: row.sort_name,
            image_url: row.image_url,
            bio: row.bio,
            movie_count: row.movie_count,
            tv_show_count: row.tv_show_count,
        })
        .collect();

    Ok(docs)
}

/// Index a single media root file by ID.
///
/// Fetches the file data from the database and indexes it using the search backend.
pub async fn index_media_root_file(
    search: &dyn SearchBackend,
    pool: &PgPool,
    file_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query_as!(
        MediaRootFileRow,
        r#"
        SELECT
            f.id,
            f.file_name,
            f.file_path,
            f.media_root_id,
            COALESCE(mr.name, mr.path) as "media_root_name!",
            f.match_status,
            f.entity_type,
            f.interpretation,
            b.title as "book_title?",
            m.title as "movie_title?",
            tv.title as "show_name?"
        FROM media_root_files f
        JOIN media_roots mr ON mr.id = f.media_root_id
        LEFT JOIN books b ON f.entity_type = 'book' AND f.entity_id = b.id
        LEFT JOIN movies m ON f.entity_type = 'movie' AND f.entity_id = m.id
        LEFT JOIN tv_shows tv ON f.entity_type = 'tv_show' AND f.entity_id = tv.id
        WHERE f.id = $1
        "#,
        file_id,
    )
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        let _ = search.delete_media_root_file(file_id).await;
        return Ok(());
    };

    let doc = media_root_file_row_to_doc(row);
    search.index_media_root_file(doc).await?;

    Ok(())
}

/// Index multiple media root files in a single batch.
///
/// Fetches all documents in one query and sends them to the search backend
/// in a single batch call, avoiding N+1 DB queries and HTTP calls.
pub async fn batch_index_media_root_files(
    search: &dyn SearchBackend,
    pool: &PgPool,
    file_ids: &[Uuid],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if file_ids.is_empty() {
        return Ok(());
    }
    let rows: Vec<MediaRootFileRow> = sqlx::query_as(
        r#"
        SELECT
            f.id,
            f.file_name,
            f.file_path,
            f.media_root_id,
            COALESCE(mr.name, mr.path) as media_root_name,
            f.match_status,
            f.entity_type,
            f.interpretation,
            b.title as book_title,
            m.title as movie_title,
            tv.title as show_name
        FROM media_root_files f
        JOIN media_roots mr ON mr.id = f.media_root_id
        LEFT JOIN books b ON f.entity_type = 'book' AND f.entity_id = b.id
        LEFT JOIN movies m ON f.entity_type = 'movie' AND f.entity_id = m.id
        LEFT JOIN tv_shows tv ON f.entity_type = 'tv_show' AND f.entity_id = tv.id
        WHERE f.id = ANY($1)
        "#,
    )
    .bind(file_ids)
    .fetch_all(pool)
    .await?;

    let docs: Vec<_> = rows.into_iter().map(media_root_file_row_to_doc).collect();
    search.index_media_root_files_batch(docs).await?;

    Ok(())
}

/// Fetch all media root file documents from the database for reindexing.
pub async fn fetch_all_media_root_file_docs(
    pool: &PgPool,
) -> Result<Vec<MediaRootFileDocument>, Box<dyn std::error::Error + Send + Sync>> {
    let rows = sqlx::query_as!(
        MediaRootFileRow,
        r#"
        SELECT
            f.id,
            f.file_name,
            f.file_path,
            f.media_root_id,
            COALESCE(mr.name, mr.path) as "media_root_name!",
            f.match_status,
            f.entity_type,
            f.interpretation,
            b.title as "book_title?",
            m.title as "movie_title?",
            tv.title as "show_name?"
        FROM media_root_files f
        JOIN media_roots mr ON mr.id = f.media_root_id
        LEFT JOIN books b ON f.entity_type = 'book' AND f.entity_id = b.id
        LEFT JOIN movies m ON f.entity_type = 'movie' AND f.entity_id = m.id
        LEFT JOIN tv_shows tv ON f.entity_type = 'tv_show' AND f.entity_id = tv.id
        WHERE f.available = true
        "#,
    )
    .fetch_all(pool)
    .await?;

    let docs = rows.into_iter().map(media_root_file_row_to_doc).collect();
    Ok(docs)
}

#[derive(sqlx::FromRow)]
struct MediaRootFileRow {
    id: Uuid,
    file_name: String,
    file_path: String,
    media_root_id: Uuid,
    media_root_name: String,
    match_status: String,
    entity_type: Option<String>,
    interpretation: Option<serde_json::Value>,
    book_title: Option<String>,
    movie_title: Option<String>,
    show_name: Option<String>,
}

fn media_root_file_row_to_doc(row: MediaRootFileRow) -> MediaRootFileDocument {
    let interp = row.interpretation.as_ref();

    // Prefer the actual entity title (from JOIN) over the interpretation guess
    let show_name = row.show_name.or_else(|| {
        interp
            .and_then(|v| v.get("show_name"))
            .and_then(|v| v.as_str())
            .map(String::from)
    });
    let movie_title = row.movie_title.or_else(|| {
        interp
            .and_then(|v| v.get("movie_title"))
            .and_then(|v| v.as_str())
            .map(String::from)
    });
    let book_title = row.book_title.or_else(|| {
        interp
            .and_then(|v| v.get("title"))
            .and_then(|v| v.as_str())
            .map(String::from)
    });
    let series_name = interp
        .and_then(|v| v.get("series"))
        .and_then(|v| v.as_str())
        .map(String::from);

    MediaRootFileDocument {
        id: row.id.to_string(),
        file_name: row.file_name,
        file_path: row.file_path,
        media_root_id: row.media_root_id.to_string(),
        media_root_name: row.media_root_name,
        match_status: row.match_status,
        entity_type: row.entity_type,
        show_name,
        movie_title,
        book_title,
        series_name,
    }
}

/// Index transcript segments (audio transcripts + ebook text) for a book.
///
/// Loads audio transcript segments and ebook text segments from the database,
/// converts them to `TranscriptSegmentDocument`s, and indexes them for hybrid
/// semantic search.
pub async fn index_transcript_segments_for_book(
    pool: &PgPool,
    search: &dyn SearchBackend,
    book_id: Uuid,
) -> Result<(), SearchError> {
    use crate::repositories::{audiobook_chapters, audiobook_transcripts, books, ebook_segments};

    // Get the book title
    let book_title = books::get_title(pool, book_id)
        .await
        .map_err(|e| SearchError::Backend(e.to_string()))?
        .unwrap_or_default();

    let book_id_str = book_id.to_string();

    // Delete existing segments for this book before re-indexing
    search
        .delete_transcript_segments_for_book(&book_id_str)
        .await?;

    let mut docs = Vec::new();

    // Load audio transcript segments
    let audio_segments = audiobook_transcripts::get_for_book(pool, book_id)
        .await
        .map_err(|e| SearchError::Backend(e.to_string()))?;

    // Build a map of audiobook_file_id -> chapter titles (keyed by time range)
    // to look up chapter titles for each transcript segment
    let mut chapter_map: std::collections::HashMap<Uuid, Vec<(f64, f64, Option<String>)>> =
        std::collections::HashMap::new();
    for seg in &audio_segments {
        if let std::collections::hash_map::Entry::Vacant(entry) =
            chapter_map.entry(seg.audiobook_file_id)
            && let Ok(chapters) =
                audiobook_chapters::get_chapters(pool, seg.audiobook_file_id).await
        {
            entry.insert(
                chapters
                    .into_iter()
                    .map(|c| (c.start_seconds, c.end_seconds, c.title))
                    .collect(),
            );
        }
    }

    // Build individual audio segment docs and collect for windowing
    let mut audio_docs: Vec<TranscriptSegmentDocument> = Vec::new();
    for seg in audio_segments {
        let chapter_title = chapter_map
            .get(&seg.audiobook_file_id)
            .and_then(|chapters| {
                chapters
                    .iter()
                    .find(|(start, end, _)| seg.start_seconds >= *start && seg.start_seconds < *end)
                    .and_then(|(_, _, title)| title.clone())
            });

        audio_docs.push(TranscriptSegmentDocument {
            id: seg.id.to_string(),
            book_id: book_id_str.clone(),
            source_type: "audio".to_string(),
            file_id: seg.audiobook_file_id.to_string(),
            segment_index: seg.segment_index,
            start_seconds: Some(seg.start_seconds),
            cfi: None,
            text: seg.text,
            book_title: book_title.clone(),
            chapter_title,
        });
    }

    docs.extend(audio_docs);

    // Load ebook segments
    let ebook_segs = ebook_segments::get_for_book(pool, book_id)
        .await
        .map_err(|e| SearchError::Backend(e.to_string()))?;

    let mut ebook_docs: Vec<TranscriptSegmentDocument> = Vec::new();
    for seg in ebook_segs {
        ebook_docs.push(TranscriptSegmentDocument {
            id: seg.id.to_string(),
            book_id: book_id_str.clone(),
            source_type: "ebook".to_string(),
            file_id: seg.book_file_id.to_string(),
            segment_index: seg.segment_index,
            start_seconds: None,
            cfi: Some(seg.cfi),
            text: seg.text,
            book_title: book_title.clone(),
            chapter_title: seg.chapter_title,
        });
    }

    docs.extend(ebook_docs);

    if !docs.is_empty() {
        search.index_transcript_segments(docs).await?;
    }

    Ok(())
}

// =============================================================================
// IPTV Channel Indexing
// =============================================================================

/// Fetch all IPTV channel documents for search indexing.
pub async fn fetch_all_iptv_channel_docs(
    pool: &PgPool,
) -> Result<Vec<IptvChannelDocument>, SearchError> {
    let rows = sqlx::query!(
        r#"
        SELECT
            c.id,
            c.name,
            c.category,
            c.language,
            c.country,
            c.icon_url,
            c.enabled,
            (SELECT COUNT(*)::int FROM iptv_sources s WHERE s.channel_id = c.id) AS "source_count!: i32",
            EXISTS(SELECT 1 FROM iptv_epg e WHERE e.channel_id = c.id) AS "has_epg!: bool",
            (SELECT e.title FROM iptv_epg e
             WHERE e.channel_id = c.id
             AND e.start_time <= NOW() AND e.stop_time > NOW()
             ORDER BY e.start_time DESC LIMIT 1) AS "current_program?",
            (SELECT g.name FROM iptv_channel_groups g
             WHERE g.enabled AND c.category = ANY(g.auto_assign_categories)
             ORDER BY g.display_order, g.slug
             LIMIT 1) AS "group?"
        FROM iptv_channels c
        "#
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let icon_url = if r.icon_url.is_some() {
                Some(format!("/api/v1/admin/iptv/channels/{}/icon", r.id))
            } else {
                None
            };
            IptvChannelDocument {
                id: r.id.to_string(),
                name: r.name,
                category: r.category,
                group: r.group,
                language: r.language,
                country: r.country,
                icon_url,
                source_count: r.source_count,
                has_epg: r.has_epg,
                current_program: r.current_program,
                enabled: r.enabled,
            }
        })
        .collect())
}

/// Index a single IPTV channel by ID.
pub async fn index_iptv_channel(
    search: &dyn SearchBackend,
    pool: &PgPool,
    channel_id: Uuid,
) -> Result<(), SearchError> {
    let row = sqlx::query!(
        r#"
        SELECT
            c.id,
            c.name,
            c.category,
            c.language,
            c.country,
            c.icon_url,
            c.enabled,
            (SELECT COUNT(*)::int FROM iptv_sources s WHERE s.channel_id = c.id) AS "source_count!: i32",
            EXISTS(SELECT 1 FROM iptv_epg e WHERE e.channel_id = c.id) AS "has_epg!: bool",
            (SELECT e.title FROM iptv_epg e
             WHERE e.channel_id = c.id
             AND e.start_time <= NOW() AND e.stop_time > NOW()
             ORDER BY e.start_time DESC LIMIT 1) AS "current_program?",
            (SELECT g.name FROM iptv_channel_groups g
             WHERE g.enabled AND c.category = ANY(g.auto_assign_categories)
             ORDER BY g.display_order, g.slug
             LIMIT 1) AS "group?"
        FROM iptv_channels c
        WHERE c.id = $1
        "#,
        channel_id
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| SearchError::Backend(format!("Failed to fetch IPTV channel: {}", e)))?;

    let Some(r) = row else {
        return Err(SearchError::NotFound(format!(
            "IPTV channel {} not found",
            channel_id
        )));
    };

    let icon_url = if r.icon_url.is_some() {
        Some(format!("/api/v1/admin/iptv/channels/{}/icon", r.id))
    } else {
        None
    };
    let doc = IptvChannelDocument {
        id: r.id.to_string(),
        name: r.name,
        category: r.category,
        group: r.group,
        language: r.language,
        country: r.country,
        icon_url,
        source_count: r.source_count,
        has_epg: r.has_epg,
        current_program: r.current_program,
        enabled: r.enabled,
    };

    search.index_iptv_channels(&[doc]).await
}

/// Fetch all IPTV EPG documents for search indexing.
///
/// Only includes entries whose stop_time is in the future (minus a 1-hour grace window).
pub async fn fetch_all_iptv_epg_docs(pool: &PgPool) -> Result<Vec<IptvEpgDocument>, SearchError> {
    use sqlx::Row;

    let rows = sqlx::query(
        r#"
        SELECT
            e.id,
            e.channel_id,
            c.name AS channel_name,
            COALESCE(e.title, '') AS title,
            e.description,
            e.categories,
            e.start_time,
            e.stop_time,
            c.icon_url
        FROM iptv_epg e
        JOIN iptv_channels c ON c.id = e.channel_id
        WHERE e.stop_time > NOW() - INTERVAL '1 hour'
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| SearchError::Backend(format!("Failed to fetch IPTV EPG: {e}")))?;

    let now = chrono::Utc::now().timestamp();

    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let title: String = r.get("title");
            if title.is_empty() {
                return None;
            }
            let id: Uuid = r.get("id");
            let channel_id: Uuid = r.get("channel_id");
            let channel_name: String = r.get("channel_name");
            let description: Option<String> = r.get("description");
            let categories: Vec<String> = r.get("categories");
            let start_time: time::OffsetDateTime = r.get("start_time");
            let stop_time: time::OffsetDateTime = r.get("stop_time");
            let icon_url: Option<String> = r.get("icon_url");

            let start_epoch = start_time.unix_timestamp();
            let stop_epoch = stop_time.unix_timestamp();
            let ch_icon = if icon_url.is_some() {
                Some(format!("/api/v1/admin/iptv/channels/{}/icon", channel_id))
            } else {
                None
            };

            Some(IptvEpgDocument {
                id: id.to_string(),
                channel_id: channel_id.to_string(),
                channel_name,
                title,
                description,
                categories,
                start_time: start_epoch,
                stop_time: stop_epoch,
                start_time_iso: start_time
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
                stop_time_iso: stop_time
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
                channel_icon_url: ch_icon,
                is_live: now >= start_epoch && now < stop_epoch,
            })
        })
        .collect())
}

// =============================================================================
// Live TV Indexing
// =============================================================================

const LIVETV_CHANNEL_DOC_SQL: &str = r#"
        SELECT
            c.id,
            c.lcn,
            c.scan_name,
            c.callsign,
            c.display_name,
            c.network,
            c.region,
            c.kind,
            c.frequency,
            c.video_codec,
            c.hd,
            c.enabled,
            c.favourite,
            c.logo_url,
            EXISTS (SELECT 1 FROM livetv_guide g WHERE g.channel_id = c.id) AS has_guide,
            (SELECT g.title FROM livetv_guide g
             WHERE g.channel_id = c.id AND g.start_time <= NOW() AND g.end_time > NOW()
             ORDER BY g.start_time DESC LIMIT 1) AS current_program
        FROM livetv_channels c
        WHERE ($1::uuid IS NULL OR c.id = $1)
        "#;

fn livetv_channel_doc(r: &sqlx::postgres::PgRow) -> LivetvChannelDocument {
    use sqlx::Row;
    let kind: String = r.get("kind");
    let video_codec: Option<String> = r.get("video_codec");
    let hd: bool = r.get("hd");
    let parsed =
        crate::livetv::classify::Kind::parse(&kind).unwrap_or(crate::livetv::classify::Kind::Tv);
    let display_name: Option<String> = r.get("display_name");
    let scan_name: Option<String> = r.get("scan_name");
    let callsign: Option<String> = r.get("callsign");
    let id: Uuid = r.get("id");
    LivetvChannelDocument {
        id: id.to_string(),
        name: crate::livetv::classify::display_name(
            display_name.as_deref(),
            scan_name.as_deref(),
            callsign.as_deref(),
        ),
        scan_name,
        callsign,
        network: r.get("network"),
        region: r.get("region"),
        kind,
        lcn: r.get("lcn"),
        mux_label: crate::livetv::classify::mux_label(r.get("frequency")),
        type_label: crate::livetv::classify::type_label(parsed, video_codec.as_deref(), hd),
        hd,
        enabled: r.get("enabled"),
        favourite: r.get("favourite"),
        has_guide: r.get("has_guide"),
        logo_url: r.get("logo_url"),
        current_program: r.get("current_program"),
    }
}

/// Fetch all Live TV channel documents for search indexing.
pub async fn fetch_all_livetv_channel_docs(
    pool: &PgPool,
) -> Result<Vec<LivetvChannelDocument>, SearchError> {
    let rows = sqlx::query(LIVETV_CHANNEL_DOC_SQL)
        .bind(None::<Uuid>)
        .fetch_all(pool)
        .await
        .map_err(|e| SearchError::Backend(format!("Failed to fetch Live TV channels: {e}")))?;

    Ok(rows.iter().map(livetv_channel_doc).collect())
}

/// Fetch all Live TV guide documents for search indexing.
pub async fn fetch_all_livetv_guide_docs(
    pool: &PgPool,
) -> Result<Vec<LivetvGuideDocument>, SearchError> {
    use sqlx::Row;

    let rows = sqlx::query(
        r#"
        SELECT
            g.id,
            g.channel_id,
            COALESCE(c.display_name, c.scan_name, c.callsign, 'Unknown') AS channel_name,
            g.title,
            g.episode_title,
            g.synopsis,
            g.genres,
            g.start_time,
            g.end_time,
            g.series_id,
            g.image_url
        FROM livetv_guide g
        JOIN livetv_channels c ON c.id = g.channel_id
        WHERE g.end_time > NOW() - INTERVAL '1 hour'
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| SearchError::Backend(format!("Failed to fetch Live TV guide: {e}")))?;

    let now = time::OffsetDateTime::now_utc().unix_timestamp();

    Ok(rows
        .into_iter()
        .map(|r| {
            let id: Uuid = r.get("id");
            let channel_id: Uuid = r.get("channel_id");
            let start_time: time::OffsetDateTime = r.get("start_time");
            let end_time: time::OffsetDateTime = r.get("end_time");
            let start_epoch = start_time.unix_timestamp();
            let end_epoch = end_time.unix_timestamp();
            LivetvGuideDocument {
                id: id.to_string(),
                channel_id: channel_id.to_string(),
                channel_name: r.get("channel_name"),
                title: r.get("title"),
                episode_title: r.get("episode_title"),
                synopsis: r.get("synopsis"),
                genres: r.get("genres"),
                start_time: start_epoch,
                end_time: end_epoch,
                start_time_iso: start_time
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
                end_time_iso: end_time
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
                series_id: r.get("series_id"),
                image_url: r.get("image_url"),
                is_live: now >= start_epoch && now < end_epoch,
            }
        })
        .collect())
}

/// Index a single Live TV channel by ID.
pub async fn index_livetv_channel(
    search: &dyn SearchBackend,
    pool: &PgPool,
    channel_id: Uuid,
) -> Result<(), SearchError> {
    let row = sqlx::query(LIVETV_CHANNEL_DOC_SQL)
        .bind(channel_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| SearchError::Backend(format!("Failed to fetch Live TV channel: {e}")))?;
    let Some(row) = row else {
        return Err(SearchError::NotFound(format!(
            "Live TV channel {channel_id} not found"
        )));
    };
    search
        .index_livetv_channels(&[livetv_channel_doc(&row)])
        .await
}
