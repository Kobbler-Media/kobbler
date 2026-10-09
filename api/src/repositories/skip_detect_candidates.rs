//! Skip detection candidates repository.
//!
//! Intermediate storage for the split skip detection pipeline.
//! Candidates are written by the audio pass, updated by the visual pass,
//! and consumed by the fusion pass.

use super::Result;
use serde_json::Value as JsonValue;
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Replace a TV season's candidate rows in a single round trip.
///
/// `skip_detect_candidates` is debug-audit data — it can get large (every
/// episode × every segment type × every path the detector explored), so the
/// DELETE + INSERT pair uses UNNEST-array parameters to do the whole batch
/// in one statement each. No per-row round trips.
pub async fn replace_season_candidates(
    pool: &PgPool,
    season_id: Uuid,
    candidates: &[CandidateRow],
    config_tag: &str,
) -> Result<usize> {
    let mut tx = pool.begin().await?;

    sqlx::query("DELETE FROM skip_detect_candidates WHERE season_id = $1")
        .bind(season_id)
        .execute(&mut *tx)
        .await?;

    if candidates.is_empty() {
        tx.commit().await?;
        return Ok(0);
    }

    let cols = split_rows(candidates);
    sqlx::query(
        r#"
        INSERT INTO skip_detect_candidates
            (id, season_id, file_id, segment_type, start_seconds, end_seconds,
             chromaprint_score, dhash_score, boundary_has_black_frame, boundary_has_silence,
             boundary_refined_start, boundary_refined_end, audio_profile,
             credit_text_score, chapter_agreement,
             file_duration, audio_pass_at, visual_pass_at, fused_at,
             config_tag, selected, rejection_reason)
        SELECT
            id, $2, file_id, segment_type, start_seconds, end_seconds,
            chromaprint_score, dhash_score, boundary_has_black_frame, boundary_has_silence,
            boundary_refined_start, boundary_refined_end, audio_profile,
            credit_text_score, chapter_agreement,
            file_duration, now(), now(), now(),
            $19, selected, rejection_reason
        FROM UNNEST(
            $1::uuid[], $3::uuid[], $4::text[], $5::float8[], $6::float8[],
            $7::float8[], $8::float8[], $9::bool[], $10::bool[],
            $11::float8[], $12::float8[], $13::text[],
            $14::float8[], $15::float8[],
            $16::float8[], $17::bool[], $18::text[]
        ) AS t(
            id, file_id, segment_type, start_seconds, end_seconds,
            chromaprint_score, dhash_score, boundary_has_black_frame, boundary_has_silence,
            boundary_refined_start, boundary_refined_end, audio_profile,
            credit_text_score, chapter_agreement,
            file_duration, selected, rejection_reason
        )
        "#,
    )
    .bind(&cols.ids)
    .bind(season_id)
    .bind(&cols.file_ids)
    .bind(&cols.segment_types)
    .bind(&cols.start_seconds)
    .bind(&cols.end_seconds)
    .bind(&cols.chromaprint_score)
    .bind(&cols.dhash_score)
    .bind(&cols.boundary_has_black_frame)
    .bind(&cols.boundary_has_silence)
    .bind(&cols.boundary_refined_start)
    .bind(&cols.boundary_refined_end)
    .bind(&cols.audio_profile)
    .bind(&cols.credit_text_score)
    .bind(&cols.chapter_agreement)
    .bind(&cols.file_duration)
    .bind(&cols.selected)
    .bind(&cols.rejection_reason)
    .bind(config_tag)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(candidates.len())
}

/// Replace a movie file's candidate rows in a single round trip.
/// Mirrors [`replace_season_candidates`] — see that doc for rationale.
pub async fn replace_movie_candidates(
    pool: &PgPool,
    movie_file_id: Uuid,
    candidates: &[CandidateRow],
    config_tag: &str,
) -> Result<usize> {
    let mut tx = pool.begin().await?;

    sqlx::query("DELETE FROM skip_detect_candidates WHERE movie_file_id = $1")
        .bind(movie_file_id)
        .execute(&mut *tx)
        .await?;

    if candidates.is_empty() {
        tx.commit().await?;
        return Ok(0);
    }

    let cols = split_rows(candidates);
    sqlx::query(
        r#"
        INSERT INTO skip_detect_candidates
            (id, movie_file_id, file_id, segment_type, start_seconds, end_seconds,
             chromaprint_score, dhash_score, boundary_has_black_frame, boundary_has_silence,
             boundary_refined_start, boundary_refined_end, audio_profile,
             credit_text_score, chapter_agreement,
             file_duration, audio_pass_at, visual_pass_at, fused_at,
             config_tag, selected, rejection_reason)
        SELECT
            id, $2, file_id, segment_type, start_seconds, end_seconds,
            chromaprint_score, dhash_score, boundary_has_black_frame, boundary_has_silence,
            boundary_refined_start, boundary_refined_end, audio_profile,
            credit_text_score, chapter_agreement,
            file_duration, now(), now(), now(),
            $19, selected, rejection_reason
        FROM UNNEST(
            $1::uuid[], $3::uuid[], $4::text[], $5::float8[], $6::float8[],
            $7::float8[], $8::float8[], $9::bool[], $10::bool[],
            $11::float8[], $12::float8[], $13::text[],
            $14::float8[], $15::float8[],
            $16::float8[], $17::bool[], $18::text[]
        ) AS t(
            id, file_id, segment_type, start_seconds, end_seconds,
            chromaprint_score, dhash_score, boundary_has_black_frame, boundary_has_silence,
            boundary_refined_start, boundary_refined_end, audio_profile,
            credit_text_score, chapter_agreement,
            file_duration, selected, rejection_reason
        )
        "#,
    )
    .bind(&cols.ids)
    .bind(movie_file_id)
    .bind(&cols.file_ids)
    .bind(&cols.segment_types)
    .bind(&cols.start_seconds)
    .bind(&cols.end_seconds)
    .bind(&cols.chromaprint_score)
    .bind(&cols.dhash_score)
    .bind(&cols.boundary_has_black_frame)
    .bind(&cols.boundary_has_silence)
    .bind(&cols.boundary_refined_start)
    .bind(&cols.boundary_refined_end)
    .bind(&cols.audio_profile)
    .bind(&cols.credit_text_score)
    .bind(&cols.chapter_agreement)
    .bind(&cols.file_duration)
    .bind(&cols.selected)
    .bind(&cols.rejection_reason)
    .bind(config_tag)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(candidates.len())
}

/// Columnar view of `CandidateRow`s for a single UNNEST-array batched INSERT.
/// Does not include `match_detail` — that column is always NULL in V2
/// (pair-level detail lives on `EpisodeDiagnostics`, not `CandidateSignals`).
/// If it ever starts getting populated, add a typed `sqlx::types::Json<_>`
/// field rather than reaching back to `Option<serde_json::Value>`.
struct Columnar {
    ids: Vec<Uuid>,
    file_ids: Vec<Uuid>,
    segment_types: Vec<String>,
    start_seconds: Vec<f64>,
    end_seconds: Vec<f64>,
    chromaprint_score: Vec<Option<f64>>,
    dhash_score: Vec<Option<f64>>,
    boundary_has_black_frame: Vec<Option<bool>>,
    boundary_has_silence: Vec<Option<bool>>,
    boundary_refined_start: Vec<Option<f64>>,
    boundary_refined_end: Vec<Option<f64>>,
    audio_profile: Vec<Option<String>>,
    credit_text_score: Vec<Option<f64>>,
    chapter_agreement: Vec<Option<f64>>,
    file_duration: Vec<Option<f64>>,
    selected: Vec<bool>,
    rejection_reason: Vec<Option<String>>,
}

fn split_rows(rows: &[CandidateRow]) -> Columnar {
    let n = rows.len();
    let mut c = Columnar {
        ids: Vec::with_capacity(n),
        file_ids: Vec::with_capacity(n),
        segment_types: Vec::with_capacity(n),
        start_seconds: Vec::with_capacity(n),
        end_seconds: Vec::with_capacity(n),
        chromaprint_score: Vec::with_capacity(n),
        dhash_score: Vec::with_capacity(n),
        boundary_has_black_frame: Vec::with_capacity(n),
        boundary_has_silence: Vec::with_capacity(n),
        boundary_refined_start: Vec::with_capacity(n),
        boundary_refined_end: Vec::with_capacity(n),
        audio_profile: Vec::with_capacity(n),
        credit_text_score: Vec::with_capacity(n),
        chapter_agreement: Vec::with_capacity(n),
        file_duration: Vec::with_capacity(n),
        selected: Vec::with_capacity(n),
        rejection_reason: Vec::with_capacity(n),
    };
    for r in rows {
        c.ids.push(Uuid::now_v7());
        c.file_ids.push(r.file_id);
        c.segment_types.push(r.segment_type.clone());
        c.start_seconds.push(r.start_seconds);
        c.end_seconds.push(r.end_seconds);
        c.chromaprint_score.push(r.chromaprint_score);
        c.dhash_score.push(r.dhash_score);
        c.boundary_has_black_frame.push(r.boundary_has_black_frame);
        c.boundary_has_silence.push(r.boundary_has_silence);
        c.boundary_refined_start.push(r.boundary_refined_start);
        c.boundary_refined_end.push(r.boundary_refined_end);
        c.audio_profile.push(r.audio_profile.clone());
        c.credit_text_score.push(r.credit_text_score);
        c.chapter_agreement.push(r.chapter_agreement);
        c.file_duration.push(r.file_duration);
        c.selected.push(r.selected);
        c.rejection_reason.push(r.rejection_reason.clone());
    }
    c
}

/// List candidates for a TV season.
pub async fn list_by_season(pool: &PgPool, season_id: Uuid) -> Result<Vec<StoredCandidate>> {
    let rows = sqlx::query(
        r#"
        SELECT id, season_id, movie_file_id, file_id, segment_type,
               start_seconds, end_seconds, chromaprint_score, dhash_score,
               boundary_has_black_frame, boundary_has_silence,
               boundary_refined_start, boundary_refined_end,
               audio_profile, visual_score, config_tag,
               selected, rejection_reason,
               credit_text_score, match_detail, chapter_agreement,
               file_duration
        FROM skip_detect_candidates
        WHERE season_id = $1
        ORDER BY file_id, start_seconds
        "#,
    )
    .bind(season_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(row_to_stored).collect())
}

/// List candidates for a movie file.
pub async fn list_by_movie_file(
    pool: &PgPool,
    movie_file_id: Uuid,
) -> Result<Vec<StoredCandidate>> {
    let rows = sqlx::query(
        r#"
        SELECT id, season_id, movie_file_id, file_id, segment_type,
               start_seconds, end_seconds, chromaprint_score, dhash_score,
               boundary_has_black_frame, boundary_has_silence,
               boundary_refined_start, boundary_refined_end,
               audio_profile, visual_score, config_tag,
               selected, rejection_reason,
               credit_text_score, match_detail, chapter_agreement,
               file_duration
        FROM skip_detect_candidates
        WHERE movie_file_id = $1
        ORDER BY start_seconds
        "#,
    )
    .bind(movie_file_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(row_to_stored).collect())
}

/// Update visual_score for a set of candidates.
pub async fn set_visual_scores(pool: &PgPool, updates: &[(Uuid, f64)]) -> Result<u64> {
    if updates.is_empty() {
        return Ok(0);
    }

    let ids: Vec<Uuid> = updates.iter().map(|(id, _)| *id).collect();
    let scores: Vec<f64> = updates.iter().map(|(_, s)| *s).collect();

    let result = sqlx::query(
        r#"
        UPDATE skip_detect_candidates AS c
        SET visual_score = v.score,
            visual_pass_at = now()
        FROM UNNEST($1::uuid[], $2::float8[]) AS v(id, score)
        WHERE c.id = v.id
        "#,
    )
    .bind(&ids)
    .bind(&scores)
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

/// Mark candidates as fused.
pub async fn mark_fused(pool: &PgPool, candidate_ids: &[Uuid]) -> Result<u64> {
    if candidate_ids.is_empty() {
        return Ok(0);
    }

    let result =
        sqlx::query("UPDATE skip_detect_candidates SET fused_at = now() WHERE id = ANY($1)")
            .bind(candidate_ids)
            .execute(pool)
            .await?;

    Ok(result.rows_affected())
}

/// Input row for inserting candidates. Carries the outcome flags directly
/// (`selected` = fusion winner, `rejection_reason` = why it was dropped) so
/// the whole batch lands in one INSERT — no follow-up UPDATE pass needed.
///
/// No `match_detail`: the schema column exists (`jsonb`) but is always NULL
/// in V2 — pair-level match detail is captured on `EpisodeDiagnostics`, not
/// `CandidateSignals`. If per-candidate detail becomes useful, add a typed
/// `sqlx::types::Json<PairDiag[]>` field rather than reaching for raw
/// `serde_json::Value`.
pub struct CandidateRow {
    pub file_id: Uuid,
    pub segment_type: String,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub chromaprint_score: Option<f64>,
    pub dhash_score: Option<f64>,
    pub boundary_has_black_frame: Option<bool>,
    pub boundary_has_silence: Option<bool>,
    pub boundary_refined_start: Option<f64>,
    pub boundary_refined_end: Option<f64>,
    pub audio_profile: Option<String>,
    pub credit_text_score: Option<f64>,
    pub chapter_agreement: Option<f64>,
    pub file_duration: Option<f64>,
    pub selected: bool,
    pub rejection_reason: Option<String>,
}

/// Stored candidate row from the database.
#[derive(Debug)]
pub struct StoredCandidate {
    pub id: Uuid,
    pub season_id: Option<Uuid>,
    pub movie_file_id: Option<Uuid>,
    pub file_id: Uuid,
    pub segment_type: String,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub chromaprint_score: Option<f64>,
    pub dhash_score: Option<f64>,
    pub boundary_has_black_frame: Option<bool>,
    pub boundary_has_silence: Option<bool>,
    pub boundary_refined_start: Option<f64>,
    pub boundary_refined_end: Option<f64>,
    pub audio_profile: Option<String>,
    pub visual_score: Option<f64>,
    pub config_tag: Option<String>,
    pub selected: bool,
    pub rejection_reason: Option<String>,
    pub credit_text_score: Option<f64>,
    pub match_detail: Option<JsonValue>,
    pub chapter_agreement: Option<f64>,
    pub file_duration: Option<f64>,
}

/// Mark candidates as selected (the fusion winners).
pub async fn mark_selected(pool: &PgPool, candidate_ids: &[Uuid]) -> Result<u64> {
    if candidate_ids.is_empty() {
        return Ok(0);
    }

    let result =
        sqlx::query("UPDATE skip_detect_candidates SET selected = true WHERE id = ANY($1)")
            .bind(candidate_ids)
            .execute(pool)
            .await?;

    Ok(result.rows_affected())
}

/// Mark candidates as rejected with reasons.
pub async fn mark_rejected(pool: &PgPool, updates: &[(Uuid, String)]) -> Result<u64> {
    if updates.is_empty() {
        return Ok(0);
    }

    let ids: Vec<Uuid> = updates.iter().map(|(id, _)| *id).collect();
    let reasons: Vec<&str> = updates.iter().map(|(_, r)| r.as_str()).collect();

    let result = sqlx::query(
        r#"
        UPDATE skip_detect_candidates AS c
        SET rejection_reason = v.reason
        FROM UNNEST($1::uuid[], $2::text[]) AS v(id, reason)
        WHERE c.id = v.id
        "#,
    )
    .bind(&ids)
    .bind(&reasons)
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

/// Update credit_text_score on an existing candidate, optionally adjusting start_seconds.
pub async fn update_credit_text_score(
    pool: &PgPool,
    candidate_id: Uuid,
    score: f64,
    start_override: Option<f64>,
) -> Result<u64> {
    let result = if let Some(start) = start_override {
        sqlx::query(
            r#"
            UPDATE skip_detect_candidates
            SET credit_text_score = $2, start_seconds = $3, visual_pass_at = now()
            WHERE id = $1
            "#,
        )
        .bind(candidate_id)
        .bind(score)
        .bind(start)
        .execute(pool)
        .await?
    } else {
        sqlx::query(
            r#"
            UPDATE skip_detect_candidates
            SET credit_text_score = $2, visual_pass_at = now()
            WHERE id = $1
            "#,
        )
        .bind(candidate_id)
        .bind(score)
        .execute(pool)
        .await?
    };
    Ok(result.rows_affected())
}

fn row_to_stored(r: sqlx::postgres::PgRow) -> StoredCandidate {
    StoredCandidate {
        id: r.get("id"),
        season_id: r.get("season_id"),
        movie_file_id: r.get("movie_file_id"),
        file_id: r.get("file_id"),
        segment_type: r.get("segment_type"),
        start_seconds: r.get("start_seconds"),
        end_seconds: r.get("end_seconds"),
        chromaprint_score: r.get("chromaprint_score"),
        dhash_score: r.get("dhash_score"),
        boundary_has_black_frame: r.get("boundary_has_black_frame"),
        boundary_has_silence: r.get("boundary_has_silence"),
        boundary_refined_start: r.get("boundary_refined_start"),
        boundary_refined_end: r.get("boundary_refined_end"),
        audio_profile: r.get("audio_profile"),
        visual_score: r.get("visual_score"),
        config_tag: r.get("config_tag"),
        selected: r.get("selected"),
        rejection_reason: r.get("rejection_reason"),
        credit_text_score: r.get("credit_text_score"),
        match_detail: r.get("match_detail"),
        chapter_agreement: r.get("chapter_agreement"),
        file_duration: r.get("file_duration"),
    }
}
