use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct GuideRow {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub start_time: OffsetDateTime,
    pub end_time: OffsetDateTime,
    pub title: String,
    pub episode_number: Option<String>,
    pub episode_title: Option<String>,
    pub synopsis: Option<String>,
    pub original_airdate: Option<OffsetDateTime>,
    pub series_id: Option<String>,
    pub genres: Vec<String>,
    pub image_url: Option<String>,
    pub poster_url: Option<String>,
    pub is_first: bool,
    pub episode_key: Option<String>,
    pub episode_key_kind: Option<String>,
    pub synced_at: OffsetDateTime,
}

#[derive(Debug)]
pub struct UpsertGuideEntry {
    pub channel_id: Uuid,
    pub start_time: OffsetDateTime,
    pub end_time: OffsetDateTime,
    pub title: String,
    pub episode_number: Option<String>,
    pub episode_title: Option<String>,
    pub synopsis: Option<String>,
    pub original_airdate: Option<OffsetDateTime>,
    pub series_id: Option<String>,
    pub genres: Vec<String>,
    pub image_url: Option<String>,
    pub poster_url: Option<String>,
    pub is_first: bool,
    pub episode_key: Option<String>,
    pub episode_key_kind: Option<String>,
}

pub async fn upsert<'e, E>(exec: E, input: UpsertGuideEntry) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let r = sqlx::query!(
        r#"
        INSERT INTO livetv_guide (
            id, channel_id, start_time, end_time, title, episode_number, episode_title, synopsis,
            original_airdate, series_id, genres, image_url, poster_url, is_first,
            episode_key, episode_key_kind, synced_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, NOW())
        ON CONFLICT (channel_id, start_time) DO UPDATE SET
            end_time = EXCLUDED.end_time,
            title = EXCLUDED.title,
            episode_number = EXCLUDED.episode_number,
            episode_title = EXCLUDED.episode_title,
            synopsis = EXCLUDED.synopsis,
            original_airdate = EXCLUDED.original_airdate,
            series_id = EXCLUDED.series_id,
            genres = EXCLUDED.genres,
            image_url = EXCLUDED.image_url,
            poster_url = EXCLUDED.poster_url,
            is_first = EXCLUDED.is_first,
            episode_key = EXCLUDED.episode_key,
            episode_key_kind = EXCLUDED.episode_key_kind,
            synced_at = NOW()
        RETURNING id
        "#,
        id,
        input.channel_id,
        input.start_time,
        input.end_time,
        input.title,
        input.episode_number,
        input.episode_title,
        input.synopsis,
        input.original_airdate,
        input.series_id,
        &input.genres,
        input.image_url,
        input.poster_url,
        input.is_first,
        input.episode_key,
        input.episode_key_kind,
    )
    .fetch_one(exec)
    .await?;
    Ok(r.id)
}

pub async fn prune_expired<'e, E>(exec: E, older_than_hours: i64) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let res = sqlx::query!(
        "DELETE FROM livetv_guide WHERE end_time < NOW() - make_interval(hours => $1::int)",
        older_than_hours as i32,
    )
    .execute(exec)
    .await?;
    Ok(res.rows_affected())
}

/// Airings the feed no longer lists, inside the window it has just answered for.
///
/// The upsert is keyed on (channel, start time), so a programme the broadcaster moves by
/// fifteen minutes is written at its new slot and the old row stays behind — the guide
/// then shows the same show twice, and the stale one is what a series rule matches. Only
/// the span the feed actually covered for this channel is touched, and only for channels
/// it answered about, so a partial sync can never empty a row.
///
/// A scheduled recording's `guide_id` is `ON DELETE SET NULL`, so a row it points at is
/// left where it is rather than having the link cut underneath it. It will expire out of
/// the guide on its own once it has aired.
pub async fn prune_missing<'e, E>(
    exec: E,
    channel_id: Uuid,
    from: OffsetDateTime,
    to: OffsetDateTime,
    keep_starts: &[OffsetDateTime],
) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let res = sqlx::query!(
        r#"
        DELETE FROM livetv_guide g
        WHERE g.channel_id = $1
          AND g.start_time >= $2
          AND g.start_time <= $3
          AND g.start_time <> ALL($4)
          AND NOT EXISTS (
              SELECT 1 FROM livetv_recordings r
              WHERE r.guide_id = g.id AND r.status NOT IN ('cancelled', 'moved')
          )
        "#,
        channel_id,
        from,
        to,
        keep_starts,
    )
    .execute(exec)
    .await?;
    Ok(res.rows_affected())
}

pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<GuideRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, channel_id, start_time, end_time, title, episode_number, episode_title,
               synopsis, original_airdate, series_id, genres, image_url, poster_url, is_first,
               episode_key, episode_key_kind, synced_at
        FROM livetv_guide WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| GuideRow {
        id: r.id,
        channel_id: r.channel_id,
        start_time: r.start_time,
        end_time: r.end_time,
        title: r.title,
        episode_number: r.episode_number,
        episode_title: r.episode_title,
        synopsis: r.synopsis,
        original_airdate: r.original_airdate,
        series_id: r.series_id,
        genres: r.genres,
        image_url: r.image_url,
        poster_url: r.poster_url,
        is_first: r.is_first,
        episode_key: r.episode_key,
        episode_key_kind: r.episode_key_kind,
        synced_at: r.synced_at,
    }))
}

pub async fn list_window<'e, E>(
    exec: E,
    from: OffsetDateTime,
    to: OffsetDateTime,
    channel_ids: Option<&[Uuid]>,
    search: Option<&str>,
    genre: Option<&str>,
) -> Result<Vec<GuideRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let pattern = search
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{s}%"));

    let rows = sqlx::query!(
        r#"
        SELECT id, channel_id, start_time, end_time, title, episode_number, episode_title,
               synopsis, original_airdate, series_id, genres, image_url, poster_url, is_first,
               episode_key, episode_key_kind, synced_at
        FROM livetv_guide
        WHERE end_time > $1 AND start_time < $2
          AND ($3::uuid[] IS NULL OR channel_id = ANY($3))
          AND ($4::text IS NULL OR title ILIKE $4
               OR COALESCE(episode_title, '') ILIKE $4
               OR COALESCE(synopsis, '') ILIKE $4)
          AND ($5::text IS NULL OR $5 = ANY(genres))
        ORDER BY channel_id, start_time
        "#,
        from,
        to,
        channel_ids,
        pattern,
        genre,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| GuideRow {
            id: r.id,
            channel_id: r.channel_id,
            start_time: r.start_time,
            end_time: r.end_time,
            title: r.title,
            episode_number: r.episode_number,
            episode_title: r.episode_title,
            synopsis: r.synopsis,
            original_airdate: r.original_airdate,
            series_id: r.series_id,
            genres: r.genres,
            image_url: r.image_url,
            poster_url: r.poster_url,
            is_first: r.is_first,
            episode_key: r.episode_key,
            episode_key_kind: r.episode_key_kind,
            synced_at: r.synced_at,
        })
        .collect())
}

/// Full rows for a set of ids, constrained to the same window, channel set and genre the
/// windowed query uses.
///
/// The search index carries a projection of a guide entry — no episode number, duration,
/// recording state or repeat count — so hitting it can only ever produce ids. Hydrating
/// them here is what keeps one response shape whether the entries were found by index or
/// by scan.
pub async fn list_by_ids<'e, E>(
    exec: E,
    ids: &[Uuid],
    from: OffsetDateTime,
    to: OffsetDateTime,
    channel_ids: Option<&[Uuid]>,
    genre: Option<&str>,
) -> Result<Vec<GuideRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, channel_id, start_time, end_time, title, episode_number, episode_title,
               synopsis, original_airdate, series_id, genres, image_url, poster_url, is_first,
               episode_key, episode_key_kind, synced_at
        FROM livetv_guide
        WHERE id = ANY($1)
          AND end_time > $2 AND start_time < $3
          AND ($4::uuid[] IS NULL OR channel_id = ANY($4))
          AND ($5::text IS NULL OR $5 = ANY(genres))
        ORDER BY channel_id, start_time
        "#,
        ids,
        from,
        to,
        channel_ids,
        genre,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| GuideRow {
            id: r.id,
            channel_id: r.channel_id,
            start_time: r.start_time,
            end_time: r.end_time,
            title: r.title,
            episode_number: r.episode_number,
            episode_title: r.episode_title,
            synopsis: r.synopsis,
            original_airdate: r.original_airdate,
            series_id: r.series_id,
            genres: r.genres,
            image_url: r.image_url,
            poster_url: r.poster_url,
            is_first: r.is_first,
            episode_key: r.episode_key,
            episode_key_kind: r.episode_key_kind,
            synced_at: r.synced_at,
        })
        .collect())
}

pub async fn recordings_for_entries<'e, E>(
    exec: E,
    guide_ids: &[Uuid],
) -> Result<Vec<(Uuid, Uuid, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT guide_id AS "guide_id!", id, status
        FROM livetv_recordings
        WHERE guide_id = ANY($1) AND status NOT IN ('cancelled', 'moved')
        "#,
        guide_ids,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.guide_id, r.id, r.status))
        .collect())
}

pub async fn count_all<'e, E>(exec: E) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let r = sqlx::query!(r#"SELECT COUNT(*) AS "count!" FROM livetv_guide"#)
        .fetch_one(exec)
        .await?;
    Ok(r.count)
}

pub async fn now_and_next<'e, E>(
    exec: E,
    channel_id: Uuid,
    now: OffsetDateTime,
) -> Result<(Option<GuideRow>, Option<GuideRow>)>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = list_window(
        exec,
        now - time::Duration::hours(6),
        now + time::Duration::hours(12),
        Some(&[channel_id]),
        None,
        None,
    )
    .await?;
    let mut current = None;
    let mut next: Option<GuideRow> = None;
    for row in rows {
        if row.start_time <= now && row.end_time > now {
            current = Some(row);
        } else if row.start_time > now
            && next.as_ref().is_none_or(|n| row.start_time < n.start_time)
        {
            next = Some(row);
        }
    }
    Ok((current, next))
}

/// A guide entry joined to the channel facts the series-rule expander needs to
/// pick one member of a simulcast cluster: HD, then signal quality, then the
/// rule's preferred channel.
#[derive(Debug, Clone)]
pub struct AiringRow {
    pub guide: GuideRow,
    pub channel_name: String,
    pub lcn: Option<i32>,
    pub hd: bool,
    pub signal_quality: Option<i32>,
    pub channel_enabled: bool,
}

pub async fn list_airings_for_series<'e, E>(
    exec: E,
    series_id: &str,
    from: OffsetDateTime,
    to: OffsetDateTime,
) -> Result<Vec<AiringRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT g.id, g.channel_id, g.start_time, g.end_time, g.title, g.episode_number,
               g.episode_title, g.synopsis, g.original_airdate, g.series_id, g.genres,
               g.image_url, g.poster_url, g.is_first, g.episode_key, g.episode_key_kind,
               g.synced_at,
               c.display_name, c.scan_name, c.callsign, c.lcn, c.hd, c.signal_quality,
               c.enabled AS channel_enabled
        FROM livetv_guide g
        JOIN livetv_channels c ON c.id = g.channel_id
        WHERE g.series_id = $1 AND g.start_time >= $2 AND g.start_time < $3
        ORDER BY g.start_time ASC, c.lcn ASC
        "#,
        series_id,
        from,
        to,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| AiringRow {
            channel_name: crate::livetv::classify::display_name(
                r.display_name.as_deref(),
                r.scan_name.as_deref(),
                r.callsign.as_deref(),
            ),
            lcn: r.lcn,
            hd: r.hd,
            signal_quality: r.signal_quality,
            channel_enabled: r.channel_enabled,
            guide: GuideRow {
                id: r.id,
                channel_id: r.channel_id,
                start_time: r.start_time,
                end_time: r.end_time,
                title: r.title,
                episode_number: r.episode_number,
                episode_title: r.episode_title,
                synopsis: r.synopsis,
                original_airdate: r.original_airdate,
                series_id: r.series_id,
                genres: r.genres,
                image_url: r.image_url,
                poster_url: r.poster_url,
                is_first: r.is_first,
                episode_key: r.episode_key,
                episode_key_kind: r.episode_key_kind,
                synced_at: r.synced_at,
            },
        })
        .collect())
}

pub async fn genres_for<'e, E>(exec: E, ids: &[Uuid]) -> Result<Vec<(Uuid, Vec<String>)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        "SELECT id, genres FROM livetv_guide WHERE id = ANY($1)",
        ids
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.genres)).collect())
}
