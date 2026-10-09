//! Job concurrency configuration API.
//!
//! GET  /api/v1/admin/jobs/concurrency — list all configurable job types with effective concurrency
//! PUT  /api/v1/admin/jobs/concurrency — update concurrency for a job type

use crate::AppState;
use crate::routes::AuthUser;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Static metadata for a configurable job type.
struct JobMeta {
    name: &'static str,
    description: &'static str,
    default_concurrency: u32,
    resource_notes: &'static str,
    resource_group: &'static str,
}

fn job_registry() -> Vec<(&'static str, JobMeta)> {
    vec![
        (
            "media_analyze",
            JobMeta {
                name: "Media Analyze (single-pass)",
                description: "One-decode pipeline: trickplay sprites, vhash, chromaprint, silence, loudness, black frames, and inline vision embeddings",
                default_concurrency: 1,
                resource_notes: "Streams the full file once and fans into 6-7 sinks. Effectively serialized by per-root MediaRootIoGate (cap 1 by default), so raising this without also raising the per-root io cap buys nothing. Default: 1 for NAS; raise both with the per-root cap if your NAS handles parallel reads cleanly.",
                resource_group: "I/O (heavy) + GPU",
            },
        ),
        (
            "media_integrity_check",
            JobMeta {
                name: "Media Integrity Check",
                description: "Re-examines files already in the library: compares each one against the size and mtime its cache was built from, and samples blocks looking for the all-zero runs an unfinished download leaves behind",
                default_concurrency: 1,
                resource_notes: "Reads ~768 KB per file in the sampling pass and nothing at all in stamp-only mode. Sequential NAS reads across the whole library, so leave this at 1 and run it off-peak; stamp-only is cheap enough to run often.",
                resource_group: "I/O (light)",
            },
        ),
        (
            "skip_detect",
            JobMeta {
                name: "Skip Detection V2 (cache-driven)",
                description: "Runs intro/credits/post-credits detection from cached sidecars produced by media_analyze",
                default_concurrency: 2,
                resource_notes: "Cache-only — no decode, no NAS I/O. Mostly DB writes + a small vision call for the credit-text scan. Safe to increase if you have many seasons queued at once.",
                resource_group: "DB + (optional) GPU",
            },
        ),
        (
            "audiobook_transcribe",
            JobMeta {
                name: "Audiobook Transcription",
                description: "Transcribes audiobook files to text using STT service",
                default_concurrency: 1,
                resource_notes: "GPU. Limited by VRAM.",
                resource_group: "GPU",
            },
        ),
        (
            "media_extract",
            JobMeta {
                name: "Media Extraction",
                description: "Extracts keyframes and subtitles from video files",
                default_concurrency: 1,
                resource_notes: "Heavy keyframe + subtitle extraction. Streams the full file. Default: 1 for NAS; raise only on fast local storage.",
                resource_group: "I/O (heavy)",
            },
        ),
        (
            "media_probe",
            JobMeta {
                name: "Media Probe",
                description: "Reads metadata from media files via ffprobe",
                default_concurrency: 16,
                resource_notes: "Fast metadata-only ffprobe. Cheap per-job; default 4-16 fine even on NAS.",
                resource_group: "I/O (light)",
            },
        ),
        (
            "media_root_scan",
            JobMeta {
                name: "Media Root Scan",
                description: "Scans media root directories for new/changed files",
                default_concurrency: 2,
                resource_notes: "Mostly stat walks, low byte cost. Default: 1-2. Not gated by per-root IO limit.",
                resource_group: "I/O",
            },
        ),
        (
            "media_root_interpret",
            JobMeta {
                name: "File Interpretation",
                description: "Parses filenames to extract show/episode/movie metadata",
                default_concurrency: 3,
                resource_notes: "CPU only. Safe to increase.",
                resource_group: "CPU",
            },
        ),
        (
            "movie_enrich",
            JobMeta {
                name: "Movie Enrichment",
                description: "Fetches movie metadata and artwork from TMDB",
                default_concurrency: 4,
                resource_notes: "Network (TMDB API). Limited by API rate limits.",
                resource_group: "Network",
            },
        ),
        (
            "tv_show_enrich",
            JobMeta {
                name: "TV Show Enrichment",
                description: "Fetches TV show metadata and artwork from TMDB",
                default_concurrency: 4,
                resource_notes: "Network (TMDB API). Limited by API rate limits.",
                resource_group: "Network",
            },
        ),
        (
            "book_enrich",
            JobMeta {
                name: "Book Enrichment",
                description: "Fetches book metadata from OpenLibrary/Google Books/Hardcover",
                default_concurrency: 2,
                resource_notes: "Network (metadata APIs). Limited by API rate limits.",
                resource_group: "Network",
            },
        ),
        (
            "entity_write_images",
            JobMeta {
                name: "Image Download",
                description: "Downloads and stores poster/cover images for library items",
                default_concurrency: 4,
                resource_notes: "Network + disk write. Safe to increase.",
                resource_group: "Network + I/O",
            },
        ),
    ]
}

#[derive(Serialize)]
struct JobConcurrencyItem {
    job_type: &'static str,
    name: &'static str,
    description: &'static str,
    concurrency: u32,
    is_default: bool,
    default: u32,
    resource_notes: &'static str,
    resource_group: &'static str,
    pending: i64,
    running: i64,
}

#[derive(Serialize)]
struct JobConcurrencyResponse {
    jobs: Vec<JobConcurrencyItem>,
    restart_required: bool,
}

/// GET /api/v1/admin/system/nas-health
pub async fn get_nas_health(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    auth: AuthUser,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let snapshot = state.nas_health.snapshot().await;
    axum::Json(snapshot).into_response()
}

/// How many of the most recent embedding attempts the health snapshot scores.
///
/// Deliberately a *recency* window, not a time window: the pipeline can sit
/// idle for weeks, and a "last 24h" view reports a reassuring `idle` right
/// through a total outage. Scoring the last N attempts however old they are is
/// what surfaces a break that has been silently degrading to audio-only.
const VISION_HEALTH_WINDOW: i64 = 200;

#[derive(Debug, Serialize, PartialEq)]
pub struct VisionHealthSnapshot {
    /// `disabled` | `idle` | `healthy` | `degraded` | `failing`
    status: &'static str,
    attempted: i64,
    ok: i64,
    failed: i64,
    last_error: Option<String>,
    note: Option<String>,
}

/// Classify the window. Split out from the query so the thresholds are
/// testable without a database.
fn classify_vision_health(
    configured: bool,
    ok: i64,
    failed: i64,
    last_error: Option<String>,
) -> VisionHealthSnapshot {
    let attempted = ok + failed;
    let (status, note) = if !configured {
        (
            "disabled",
            Some("Vision is not configured — skip detection runs audio-only.".to_string()),
        )
    } else if attempted == 0 {
        ("idle", None)
    } else if ok == 0 {
        (
            "failing",
            Some(format!(
                "None of the last {attempted} embedding attempts succeeded. \
                 Skip detection has been falling back to audio-only, and files \
                 are still being marked complete."
            )),
        )
    } else if failed * 5 >= attempted {
        (
            "degraded",
            Some(format!(
                "{failed} of the last {attempted} embedding attempts failed."
            )),
        )
    } else {
        ("healthy", None)
    };
    VisionHealthSnapshot {
        status,
        attempted,
        ok,
        failed,
        last_error: if status == "healthy" || status == "disabled" {
            None
        } else {
            last_error
        },
        note,
    }
}

/// GET /api/v1/admin/system/vision-health
///
/// Scores the most recent embedding attempts recorded in `media_cache_runs`.
/// Exists because a vision outage is otherwise invisible: `media_analyze`
/// logs a warning, degrades to audio-only, and still reports success.
pub async fn get_vision_health(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }

    let config = state.runtime_config.get().await;
    let configured = config.effective_vision_url().is_some_and(|u| !u.is_empty())
        && config.skip_detect_mode != "off";

    let row = sqlx::query!(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE ok) AS "ok!",
            COUNT(*) FILTER (WHERE NOT ok) AS "failed!",
            (ARRAY_REMOVE(ARRAY_AGG(err ORDER BY started_at DESC), NULL))[1] AS last_error
        FROM (
            SELECT
                COALESCE((branch_results -> 'embeddings' ->> 'ok')::bool, false) AS ok,
                branch_results -> 'embeddings' ->> 'error' AS err,
                started_at
            FROM media_cache_runs
            WHERE branch_results -> 'embeddings' IS NOT NULL
            ORDER BY started_at DESC
            LIMIT $1
        ) recent
        "#,
        VISION_HEALTH_WINDOW,
    )
    .fetch_optional(&state.pool)
    .await;

    let (ok, failed, last_error) = match row {
        Ok(Some(r)) => (r.ok, r.failed, r.last_error),
        Ok(None) => (0, 0, None),
        Err(e) => {
            tracing::warn!(error = %e, "vision health query failed");
            (0, 0, None)
        }
    };

    Json(classify_vision_health(configured, ok, failed, last_error)).into_response()
}

/// GET /api/v1/admin/jobs/concurrency
pub async fn get_job_concurrency(
    State(state): State<AppState>,
    auth: AuthUser,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }

    let config = state.runtime_config.get().await;
    let overrides = config.job_concurrency.unwrap_or_default();

    // Get per-type pending/running counts
    let rows: Vec<(String, i64, i64)> = sqlx::query_as::<sqlx::Postgres, _>(
        "SELECT job_type,
                COUNT(*) FILTER (WHERE status = 'pending') AS pending,
                COUNT(*) FILTER (WHERE status = 'running') AS running
         FROM job_status
         WHERE status IN ('pending', 'running')
         GROUP BY job_type",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    let counts: HashMap<String, (i64, i64)> =
        rows.into_iter().map(|(jt, p, r)| (jt, (p, r))).collect();

    let registry = job_registry();
    let jobs: Vec<JobConcurrencyItem> = registry
        .into_iter()
        .map(|(job_type, meta)| {
            let configured = overrides.get(job_type).copied();
            let (pending, running) = counts.get(job_type).copied().unwrap_or((0, 0));
            JobConcurrencyItem {
                job_type,
                name: meta.name,
                description: meta.description,
                concurrency: configured.unwrap_or(meta.default_concurrency),
                is_default: configured.is_none(),
                default: meta.default_concurrency,
                resource_notes: meta.resource_notes,
                resource_group: meta.resource_group,
                pending,
                running,
            }
        })
        .collect();

    Json(JobConcurrencyResponse {
        jobs,
        restart_required: true,
    })
    .into_response()
}

#[derive(Deserialize)]
pub struct UpdateJobConcurrency {
    pub job_type: String,
    pub concurrency: u32,
}

/// PUT /api/v1/admin/jobs/concurrency
pub async fn update_job_concurrency(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<UpdateJobConcurrency>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }

    // Validate job type is known
    let registry = job_registry();
    if !registry.iter().any(|(jt, _)| *jt == payload.job_type) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("Unknown job type: {}", payload.job_type)})),
        )
            .into_response();
    }

    if payload.concurrency < 1 || payload.concurrency > 32 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Concurrency must be between 1 and 32"})),
        )
            .into_response();
    }

    // Build a partial update with just this one key
    let mut map = HashMap::new();
    map.insert(payload.job_type, payload.concurrency);

    let update = crate::runtime_config::RuntimeConfigUpdate {
        job_concurrency: Some(map),
        ..Default::default()
    };

    match state.runtime_config.update(update).await {
        Ok(()) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err() -> Option<String> {
        Some("Failed to parse vision response".to_string())
    }

    /// The case this endpoint exists for: vision configured, every attempt
    /// failing, pipeline quietly completing anyway.
    #[test]
    fn total_outage_is_failing_and_keeps_the_error() {
        let s = classify_vision_health(true, 0, 295, err());
        assert_eq!(s.status, "failing");
        assert_eq!(s.attempted, 295);
        assert!(s.last_error.is_some());
        assert!(s.note.unwrap().contains("audio-only"));
    }

    #[test]
    fn occasional_failures_stay_healthy() {
        let s = classify_vision_health(true, 199, 1, err());
        assert_eq!(s.status, "healthy");
        assert_eq!(s.last_error, None, "healthy must not surface a stale error");
    }

    /// One failure in five is degraded: 40/200 trips it, 39/200 does not.
    #[test]
    fn degraded_threshold_is_one_in_five() {
        assert_eq!(
            classify_vision_health(true, 160, 40, err()).status,
            "degraded"
        );
        assert_eq!(
            classify_vision_health(true, 161, 39, err()).status,
            "healthy"
        );
    }

    #[test]
    fn unconfigured_vision_is_disabled_not_failing() {
        let s = classify_vision_health(false, 0, 0, None);
        assert_eq!(s.status, "disabled");
        assert_eq!(s.failed, 0);
    }

    /// Notes are shown to a person. A wrapped string literal keeps its
    /// indentation unless the line is continued, which is how the outage note
    /// shipped with eighteen spaces mid-sentence.
    #[test]
    fn no_note_carries_the_indentation_it_was_wrapped_at() {
        for note in [
            classify_vision_health(true, 0, 295, err()).note,
            classify_vision_health(true, 160, 40, err()).note,
            classify_vision_health(false, 0, 0, None).note,
        ]
        .into_iter()
        .flatten()
        {
            assert!(!note.contains("  "), "run of spaces in: {note}");
        }
    }

    /// No attempts recorded yet must not read as a failure.
    #[test]
    fn no_attempts_is_idle() {
        let s = classify_vision_health(true, 0, 0, None);
        assert_eq!(s.status, "idle");
        assert_eq!(s.note, None);
    }
}
