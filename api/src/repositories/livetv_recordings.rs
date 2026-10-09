use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct RecordingRow {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub guide_id: Option<Uuid>,
    pub series_rule_id: Option<Uuid>,
    pub title: String,
    pub episode_number: Option<String>,
    pub episode_title: Option<String>,
    pub episode_key: Option<String>,
    pub series_id: Option<String>,
    pub synopsis: Option<String>,
    pub scheduled_start: OffsetDateTime,
    pub scheduled_end: OffsetDateTime,
    pub actual_start: Option<OffsetDateTime>,
    pub actual_end: Option<OffsetDateTime>,
    pub padding_before_secs: i32,
    pub padding_after_secs: i32,
    pub status: String,
    pub tuner_index: Option<i32>,
    pub output_dir: Option<String>,
    pub duration_seconds: Option<f64>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub container: Option<String>,
    pub size_bytes: Option<i64>,
    pub episode_id: Option<Uuid>,
    pub movie_id: Option<Uuid>,
    pub match_attempted_at: Option<OffsetDateTime>,
    pub moved_to_guide_id: Option<Uuid>,
    pub image_url: Option<String>,
    pub error: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug)]
pub struct CreateRecording {
    pub channel_id: Uuid,
    pub guide_id: Option<Uuid>,
    pub series_rule_id: Option<Uuid>,
    pub title: String,
    pub episode_number: Option<String>,
    pub episode_title: Option<String>,
    pub episode_key: Option<String>,
    pub series_id: Option<String>,
    pub synopsis: Option<String>,
    pub scheduled_start: OffsetDateTime,
    pub scheduled_end: OffsetDateTime,
    pub actual_start: Option<OffsetDateTime>,
    pub status: String,
    pub output_dir: Option<String>,
    pub container: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub padding_before_secs: i32,
    pub padding_after_secs: i32,
    pub image_url: Option<String>,
}

macro_rules! to_row {
    ($r:expr) => {
        RecordingRow {
            id: $r.id,
            channel_id: $r.channel_id,
            guide_id: $r.guide_id,
            series_rule_id: $r.series_rule_id,
            title: $r.title,
            episode_number: $r.episode_number,
            episode_title: $r.episode_title,
            episode_key: $r.episode_key,
            series_id: $r.series_id,
            synopsis: $r.synopsis,
            scheduled_start: $r.scheduled_start,
            scheduled_end: $r.scheduled_end,
            actual_start: $r.actual_start,
            actual_end: $r.actual_end,
            padding_before_secs: $r.padding_before_secs,
            padding_after_secs: $r.padding_after_secs,
            status: $r.status,
            tuner_index: $r.tuner_index,
            output_dir: $r.output_dir,
            duration_seconds: $r.duration_seconds,
            video_codec: $r.video_codec,
            audio_codec: $r.audio_codec,
            container: $r.container,
            size_bytes: $r.size_bytes,
            episode_id: $r.episode_id,
            movie_id: $r.movie_id,
            match_attempted_at: $r.match_attempted_at,
            moved_to_guide_id: $r.moved_to_guide_id,
            image_url: $r.image_url,
            error: $r.error,
            created_at: $r.created_at,
            updated_at: $r.updated_at,
        }
    };
}

pub async fn create<'e, E>(exec: E, input: CreateRecording) -> Result<RecordingRow>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let r = sqlx::query!(
        r#"
        INSERT INTO livetv_recordings
            (id, channel_id, guide_id, title, episode_number, episode_title, episode_key,
             series_id, synopsis, scheduled_start, scheduled_end, actual_start, status,
             output_dir, container, video_codec, audio_codec, series_rule_id,
             padding_before_secs, padding_after_secs, image_url)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21)
        RETURNING id, channel_id, guide_id, series_rule_id, title, episode_number, episode_title,
                  episode_key, series_id, synopsis, scheduled_start, scheduled_end, actual_start,
                  actual_end, padding_before_secs, padding_after_secs, status, tuner_index,
                  output_dir, duration_seconds, video_codec, audio_codec, container, size_bytes,
                  episode_id, movie_id, match_attempted_at, moved_to_guide_id, image_url, error,
               created_at, updated_at
        "#,
        id,
        input.channel_id,
        input.guide_id,
        input.title,
        input.episode_number,
        input.episode_title,
        input.episode_key,
        input.series_id,
        input.synopsis,
        input.scheduled_start,
        input.scheduled_end,
        input.actual_start,
        input.status,
        input.output_dir,
        input.container,
        input.video_codec,
        input.audio_codec,
        input.series_rule_id,
        input.padding_before_secs,
        input.padding_after_secs,
        input.image_url,
    )
    .fetch_one(exec)
    .await?;
    Ok(to_row!(r))
}

pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<RecordingRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, channel_id, guide_id, series_rule_id, title, episode_number, episode_title,
               episode_key, series_id, synopsis, scheduled_start, scheduled_end, actual_start,
               actual_end, padding_before_secs, padding_after_secs, status, tuner_index,
               output_dir, duration_seconds, video_codec, audio_codec, container, size_bytes,
               episode_id, movie_id, match_attempted_at, moved_to_guide_id, image_url, error,
               created_at, updated_at
        FROM livetv_recordings WHERE id = $1
        "#,
        id
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| to_row!(r)))
}

pub struct TunerRecording {
    pub id: Uuid,
    pub status: String,
    pub output_dir: Option<String>,
}

/// Everything a tuner delete would cascade away. `livetv_recordings` hangs off
/// the channel, so dropping the tuner silently destroys the rows while the
/// captured video stays on disk.
pub async fn list_for_tuner<'e, E>(exec: E, tuner_id: Uuid) -> Result<Vec<TunerRecording>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT r.id, r.status, r.output_dir
        FROM livetv_recordings r
        JOIN livetv_channels c ON c.id = r.channel_id
        WHERE c.tuner_id = $1
        ORDER BY r.scheduled_start
        "#,
        tuner_id
    )
    .fetch_all(exec)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| TunerRecording {
            id: r.id,
            status: r.status,
            output_dir: r.output_dir,
        })
        .collect())
}

#[derive(Debug, Default)]
pub struct RecordingFilters {
    pub status: Option<String>,
    pub from: Option<OffsetDateTime>,
    pub to: Option<OffsetDateTime>,
    pub series_rule_id: Option<Uuid>,
}

pub async fn list_filtered<'e, E>(
    exec: E,
    filters: &RecordingFilters,
    limit: i64,
    offset: i64,
) -> Result<(Vec<RecordingRow>, i64)>
where
    E: Executor<'e, Database = Postgres> + Copy,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, channel_id, guide_id, series_rule_id, title, episode_number, episode_title,
               episode_key, series_id, synopsis, scheduled_start, scheduled_end, actual_start,
               actual_end, padding_before_secs, padding_after_secs, status, tuner_index,
               output_dir, duration_seconds, video_codec, audio_codec, container, size_bytes,
               episode_id, movie_id, match_attempted_at, moved_to_guide_id, image_url, error,
               created_at, updated_at
        FROM livetv_recordings
        WHERE ($1::text IS NULL OR status = $1)
          AND ($2::timestamptz IS NULL OR scheduled_end >= $2)
          AND ($3::timestamptz IS NULL OR scheduled_start <= $3)
          AND ($4::uuid IS NULL OR series_rule_id = $4)
        ORDER BY scheduled_start DESC
        LIMIT $5 OFFSET $6
        "#,
        filters.status,
        filters.from,
        filters.to,
        filters.series_rule_id,
        limit,
        offset,
    )
    .fetch_all(exec)
    .await?;

    let total = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*) AS "count!"
        FROM livetv_recordings
        WHERE ($1::text IS NULL OR status = $1)
          AND ($2::timestamptz IS NULL OR scheduled_end >= $2)
          AND ($3::timestamptz IS NULL OR scheduled_start <= $3)
          AND ($4::uuid IS NULL OR series_rule_id = $4)
        "#,
        filters.status,
        filters.from,
        filters.to,
        filters.series_rule_id,
    )
    .fetch_one(exec)
    .await?;

    Ok((rows.into_iter().map(|r| to_row!(r)).collect(), total))
}

/// Scheduled rows whose capture start falls inside the lookahead window. Padding
/// is part of the capture start, so it moves the deadline earlier.
pub async fn list_due<'e, E>(
    exec: E,
    now: OffsetDateTime,
    lookahead: std::time::Duration,
) -> Result<Vec<RecordingRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let lookahead_secs = lookahead.as_secs_f64();
    let rows = sqlx::query!(
        r#"
        SELECT id, channel_id, guide_id, series_rule_id, title, episode_number, episode_title,
               episode_key, series_id, synopsis, scheduled_start, scheduled_end, actual_start,
               actual_end, padding_before_secs, padding_after_secs, status, tuner_index,
               output_dir, duration_seconds, video_codec, audio_codec, container, size_bytes,
               episode_id, movie_id, match_attempted_at, moved_to_guide_id, image_url, error,
               created_at, updated_at
        FROM livetv_recordings
        WHERE status = 'scheduled'
          AND scheduled_end > $1
          AND scheduled_start
              - make_interval(secs => padding_before_secs::double precision)
              - make_interval(secs => $2::double precision) <= $1
        ORDER BY scheduled_start ASC
        "#,
        now,
        lookahead_secs,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| to_row!(r)).collect())
}

pub async fn list_by_status<'e, E>(exec: E, status: &str) -> Result<Vec<RecordingRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, channel_id, guide_id, series_rule_id, title, episode_number, episode_title,
               episode_key, series_id, synopsis, scheduled_start, scheduled_end, actual_start,
               actual_end, padding_before_secs, padding_after_secs, status, tuner_index,
               output_dir, duration_seconds, video_codec, audio_codec, container, size_bytes,
               episode_id, movie_id, match_attempted_at, moved_to_guide_id, image_url, error,
               created_at, updated_at
        FROM livetv_recordings WHERE status = $1 ORDER BY scheduled_start ASC
        "#,
        status,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| to_row!(r)).collect())
}

pub async fn get_by_guide_id<'e, E>(exec: E, guide_id: Uuid) -> Result<Option<RecordingRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, channel_id, guide_id, series_rule_id, title, episode_number, episode_title,
               episode_key, series_id, synopsis, scheduled_start, scheduled_end, actual_start,
               actual_end, padding_before_secs, padding_after_secs, status, tuner_index,
               output_dir, duration_seconds, video_codec, audio_codec, container, size_bytes,
               episode_id, movie_id, match_attempted_at, moved_to_guide_id, image_url, error,
               created_at, updated_at
        FROM livetv_recordings
        WHERE guide_id = $1 AND status NOT IN ('cancelled', 'moved')
        "#,
        guide_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| to_row!(r)))
}

/// Episode keys already recorded or already scheduled. Decision 6.2 — the
/// expander skips both, not just the completed ones.
pub async fn claimed_episode_keys<'e, E>(exec: E, keys: &[String]) -> Result<Vec<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_scalar!(
        r#"
        SELECT DISTINCT episode_key AS "episode_key!"
        FROM livetv_recordings
        WHERE episode_key = ANY($1)
          AND status IN ('scheduled', 'recording', 'completed')
        "#,
        keys,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Scheduled or in-flight rows overlapping a window — the tuner-contention
/// input for clash resolution.
pub async fn list_overlapping<'e, E>(
    exec: E,
    start: OffsetDateTime,
    end: OffsetDateTime,
) -> Result<Vec<RecordingRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, channel_id, guide_id, series_rule_id, title, episode_number, episode_title,
               episode_key, series_id, synopsis, scheduled_start, scheduled_end, actual_start,
               actual_end, padding_before_secs, padding_after_secs, status, tuner_index,
               output_dir, duration_seconds, video_codec, audio_codec, container, size_bytes,
               episode_id, movie_id, match_attempted_at, moved_to_guide_id, image_url, error,
               created_at, updated_at
        FROM livetv_recordings
        WHERE status IN ('scheduled', 'recording')
          AND scheduled_start < $2 AND scheduled_end > $1
        ORDER BY scheduled_start ASC
        "#,
        start,
        end,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| to_row!(r)).collect())
}

/// Atomically transition `scheduled` -> `recording`. The loser of a duplicate
/// enqueue from the lookahead window must stop; the winner owns the capture.
pub async fn try_claim_for_recording<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let res = sqlx::query!(
        r#"
        UPDATE livetv_recordings
        SET status = 'recording', actual_start = COALESCE(actual_start, NOW()), updated_at = NOW()
        WHERE id = $1 AND status = 'scheduled'
        "#,
        id,
    )
    .execute(exec)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Claim a row that is already `recording` — a redelivered job or a boot resume.
/// Only one worker may append to the same output directory at a time, so this
/// is the same compare-exchange, keyed on a heartbeat that has gone stale.
pub async fn try_claim_for_resume<'e, E>(exec: E, id: Uuid, stale_secs: i64) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let res = sqlx::query!(
        r#"
        UPDATE livetv_recordings SET updated_at = NOW()
        WHERE id = $1 AND status = 'recording'
          AND updated_at < NOW() - make_interval(secs => $2::double precision)
        "#,
        id,
        stale_secs as f64,
    )
    .execute(exec)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn heartbeat<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE livetv_recordings SET updated_at = NOW() WHERE id = $1",
        id
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn mark_started<'e, E>(
    exec: E,
    id: Uuid,
    output_dir: &str,
    tuner_index: Option<i32>,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE livetv_recordings
        SET output_dir = $2, tuner_index = $3, container = 'hls',
            actual_start = COALESCE(actual_start, NOW()), updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        output_dir,
        tuner_index,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn set_status<'e, E>(exec: E, id: Uuid, status: &str, error: Option<&str>) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let res = sqlx::query!(
        r#"
        UPDATE livetv_recordings
        SET status = $2, error = $3,
            actual_end = CASE WHEN $2 IN ('completed', 'failed') THEN NOW() ELSE actual_end END,
            updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        status,
        error,
    )
    .execute(exec)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Stop only what is still live. A completed row must not be dragged back.
pub async fn cancel<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let res = sqlx::query!(
        r#"
        UPDATE livetv_recordings
        SET status = CASE WHEN status = 'recording' THEN 'completed' ELSE 'cancelled' END,
            actual_end = CASE WHEN status = 'recording' THEN NOW() ELSE actual_end END,
            updated_at = NOW()
        WHERE id = $1 AND status IN ('scheduled', 'recording')
        "#,
        id,
    )
    .execute(exec)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn mark_moved<'e, E>(exec: E, id: Uuid, moved_to_guide_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE livetv_recordings
        SET status = 'moved', moved_to_guide_id = $2, updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        moved_to_guide_id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn update_media_metadata<'e, E>(
    exec: E,
    id: Uuid,
    duration_seconds: Option<f64>,
    size_bytes: Option<i64>,
    video_codec: Option<&str>,
    audio_codec: Option<&str>,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE livetv_recordings SET
            duration_seconds = COALESCE($2, duration_seconds),
            size_bytes = COALESCE($3, size_bytes),
            video_codec = COALESCE($4, video_codec),
            audio_codec = COALESCE($5, audio_codec),
            updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        duration_seconds,
        size_bytes,
        video_codec,
        audio_codec,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let res = sqlx::query!("DELETE FROM livetv_recordings WHERE id = $1", id)
        .execute(exec)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn queued_counts_by_rule<'e, E>(exec: E) -> Result<Vec<(Uuid, i64)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT series_rule_id AS "series_rule_id!", COUNT(*) AS "count!"
        FROM livetv_recordings
        WHERE series_rule_id IS NOT NULL AND status IN ('scheduled', 'recording')
        GROUP BY series_rule_id
        "#
    )
    .fetch_all(exec)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.series_rule_id, r.count))
        .collect())
}

/// Boot-time: no worker in this process holds any of these rows, so age their
/// claim stamp out of the way and let the resume jobs take them immediately.
pub async fn release_recording_claims<'e, E>(exec: E, stale_secs: i64) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let res = sqlx::query!(
        r#"
        UPDATE livetv_recordings
        SET updated_at = NOW() - make_interval(secs => $1::double precision)
        WHERE status = 'recording'
        "#,
        (stale_secs + 1) as f64,
    )
    .execute(exec)
    .await?;
    Ok(res.rows_affected())
}

/// Close out a capture. Guarded on `recording` so a Stop or a delete that landed
/// mid-capture is not dragged back to `completed` by the worker finishing up.
pub async fn finish<'e, E>(exec: E, id: Uuid, status: &str, error: Option<&str>) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let res = sqlx::query!(
        r#"
        UPDATE livetv_recordings
        SET status = $2, error = $3, actual_end = NOW(), updated_at = NOW()
        WHERE id = $1 AND status = 'recording'
        "#,
        id,
        status,
        error,
    )
    .execute(exec)
    .await?;
    Ok(res.rows_affected() > 0)
}

// =============================================================================
// Library matching (Decision 16)
// =============================================================================

/// Normalised in SQL with the same rule `livetv::matching::normalise` applies in
/// Rust, so the two sides of the comparison cannot drift.
pub async fn find_shows_by_normalised_title<'e, E>(exec: E, normalised: &str) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id FROM tv_shows
        WHERE regexp_replace(lower(title), '[^a-z0-9]+', '', 'g') = $1
        "#,
        normalised
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}

pub async fn find_movies_by_normalised_title<'e, E>(exec: E, normalised: &str) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id FROM movies
        WHERE regexp_replace(lower(title), '[^a-z0-9]+', '', 'g') = $1
        "#,
        normalised
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}

pub async fn find_episode_by_numbers<'e, E>(
    exec: E,
    show_id: Uuid,
    season: i32,
    episode: i32,
) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT te.id
        FROM tv_episodes te
        JOIN tv_seasons ts ON ts.id = te.season_id
        WHERE ts.show_id = $1 AND ts.season_sort_order = $2 AND te.episode_sort_order = $3
        "#,
        show_id,
        season,
        episode
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| r.id))
}

pub async fn find_episodes_by_air_date<'e, E>(
    exec: E,
    show_id: Uuid,
    air_date: time::Date,
) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT te.id
        FROM tv_episodes te
        JOIN tv_seasons ts ON ts.id = te.season_id
        WHERE ts.show_id = $1 AND te.air_date = $2
        "#,
        show_id,
        air_date
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}

pub async fn find_episodes_by_normalised_title<'e, E>(
    exec: E,
    show_id: Uuid,
    normalised: &str,
) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT te.id
        FROM tv_episodes te
        JOIN tv_seasons ts ON ts.id = te.season_id
        WHERE ts.show_id = $1
          AND regexp_replace(lower(COALESCE(te.title, '')), '[^a-z0-9]+', '', 'g') = $2
        "#,
        show_id,
        normalised
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}

pub async fn set_match<'e, E>(
    exec: E,
    id: Uuid,
    episode_id: Option<Uuid>,
    movie_id: Option<Uuid>,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let r = sqlx::query!(
        r#"
        UPDATE livetv_recordings
        SET episode_id = $2, movie_id = $3, match_attempted_at = NOW(), updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        episode_id,
        movie_id
    )
    .execute(exec)
    .await?;
    Ok(r.rows_affected() > 0)
}

// =============================================================================
// Retention (keep_count / keep_until)
// =============================================================================

#[derive(Debug, Clone)]
pub struct RetentionCandidate {
    pub id: Uuid,
    pub series_rule_id: Uuid,
    pub output_dir: Option<String>,
    pub episode_id: Option<Uuid>,
    pub guide_id: Option<Uuid>,
    pub actual_end: Option<OffsetDateTime>,
    pub scheduled_end: OffsetDateTime,
    pub size_bytes: Option<i64>,
    pub keep_count: Option<i32>,
    pub keep_until: String,
    pub keep_until_date: Option<OffsetDateTime>,
}

/// Every completed recording that a rule with a retention policy owns, newest
/// first inside each rule so `keep_count` can count down from the top.
pub async fn retention_candidates<'e, E>(exec: E) -> Result<Vec<RetentionCandidate>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT r.id, r.series_rule_id AS "series_rule_id!", r.output_dir, r.episode_id,
               r.guide_id, r.actual_end, r.scheduled_end, r.size_bytes,
               s.keep_count, s.keep_until, s.keep_until_date
        FROM livetv_recordings r
        JOIN livetv_series_rules s ON s.id = r.series_rule_id
        WHERE r.status = 'completed'
        ORDER BY r.series_rule_id, COALESCE(r.actual_end, r.scheduled_end) DESC
        "#
    )
    .fetch_all(exec)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| RetentionCandidate {
            id: r.id,
            series_rule_id: r.series_rule_id,
            output_dir: r.output_dir,
            episode_id: r.episode_id,
            guide_id: r.guide_id,
            actual_end: r.actual_end,
            scheduled_end: r.scheduled_end,
            size_bytes: r.size_bytes,
            keep_count: r.keep_count,
            keep_until: r.keep_until,
            keep_until_date: r.keep_until_date,
        })
        .collect())
}

/// A recording is watched when the household has watched it: the library watch
/// state once it is matched, otherwise a live TV watch event for the airing.
pub async fn watched_ids<'e, E>(exec: E, ids: &[Uuid]) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query!(
        r#"
        SELECT r.id
        FROM livetv_recordings r
        WHERE r.id = ANY($1)
          AND (
            EXISTS (
                SELECT 1 FROM tv_watch_states ws
                WHERE ws.episode_id = r.episode_id AND ws.watched = TRUE
            )
            OR EXISTS (
                SELECT 1 FROM movie_watch_states mws
                WHERE mws.movie_id = r.movie_id AND mws.watched = TRUE
            )
            OR EXISTS (
                SELECT 1 FROM livetv_watch_events we
                WHERE we.guide_id = r.guide_id
                  AND we.source = 'recording'
                  AND we.programme_secs IS NOT NULL
                  AND we.programme_secs > 0
                  AND we.watched_secs::float8 / we.programme_secs::float8 >= 0.9
            )
          )
        "#,
        ids
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}

/// The library file a recording was imported as, if it was matched. Keyed on
/// `source_metadata` rather than a column so a recording and a downloaded copy
/// of the same episode stay separate rows.
pub async fn find_episode_file_for_recording<'e, E>(
    exec: E,
    recording_id: Uuid,
) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id FROM tv_episode_files
        WHERE source_type = 'livetv' AND source_metadata->>'recording_id' = $1
        "#,
        recording_id.to_string()
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| r.id))
}

/// The movie-side twin of [`find_episode_file_for_recording`], for a recording
/// that matched a film rather than an episode.
pub async fn find_movie_file_for_recording<'e, E>(
    exec: E,
    recording_id: Uuid,
) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id FROM movie_files
        WHERE source_type = 'livetv' AND source_metadata->>'recording_id' = $1
        "#,
        recording_id.to_string()
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| r.id))
}
