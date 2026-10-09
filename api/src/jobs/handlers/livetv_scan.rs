use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::livetv::{classify, hdhomerun, sdt};
use crate::repositories::{livetv_channels, livetv_tuners};
use crate::search::{self, SearchBackend};
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use time::{Duration, OffsetDateTime};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct LivetvScanPayload {
    /// Absent means every tuner, which is how the schedule enqueues it. A named
    /// tuner is somebody pressing Scan, and that always runs.
    pub tuner_id: Option<Uuid>,
}

/// A lineup changes when the broadcaster retunes, which is a handful of times a
/// year. The schedule ticks more often than this only so a box that restarts
/// daily still gets there — `last_scan_at` is what actually paces it.
const RESCAN_AFTER: Duration = Duration::hours(20);

fn due_for_rescan(last_scan_at: Option<OffsetDateTime>, now: OffsetDateTime) -> bool {
    last_scan_at.is_none_or(|at| now - at >= RESCAN_AFTER)
}

/// A weak-signal or mid-retune read comes back short rather than empty. Half is
/// well below any real channel loss and well above the noise of one service
/// dropping off a mux.
fn lineup_looks_truncated(seen: usize, known: i64) -> bool {
    known > 0 && (seen as i64) * 2 < known
}

pub struct LivetvScanHandler {
    pool: PgPool,
    nats_publisher: Publisher,
    search: Arc<dyn SearchBackend>,
    library_events: tokio::sync::broadcast::Sender<crate::events::LibraryEvent>,
}

impl LivetvScanHandler {
    pub fn new(
        pool: PgPool,
        nats_publisher: Publisher,
        search: Arc<dyn SearchBackend>,
        library_events: tokio::sync::broadcast::Sender<crate::events::LibraryEvent>,
    ) -> Self {
        Self {
            pool,
            nats_publisher,
            search,
            library_events,
        }
    }
}

#[async_trait]
impl JobHandler for LivetvScanHandler {
    fn job_type(&self) -> &'static str {
        "livetv_scan"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: LivetvScanPayload =
            serde_json::from_value(payload).unwrap_or_else(|_| LivetvScanPayload::default());
        nats_jobs::check_cancelled(&cancel)?;

        let requested = payload.tuner_id.is_some();
        let tuners = match payload.tuner_id {
            Some(id) => vec![
                livetv_tuners::get_by_id(&self.pool, id)
                    .await?
                    .ok_or_else(|| JobError::ExecutionFailed(format!("Tuner {id} not found")))?,
            ],
            None => livetv_tuners::list_all(&self.pool).await?,
        };

        let now = OffsetDateTime::now_utc();
        let mut results = Vec::new();
        let mut skipped = 0usize;

        for tuner in tuners {
            nats_jobs::check_cancelled(&cancel)?;
            // The scheduler holds no state across a restart, so the age check is
            // the thing that stops a restart loop rescanning on every boot.
            if !requested && !due_for_rescan(tuner.last_scan_at, now) {
                skipped += 1;
                continue;
            }
            match self.scan_one(job_id, &tuner, requested, &cancel).await {
                Ok(result) => results.push(result),
                // One unreachable tuner must not strand the others.
                Err(e) if !requested => {
                    warn!(tuner_id = %tuner.id, error = %e, "Live TV scan failed for this tuner")
                }
                Err(e) => return Err(e),
            }
        }

        Ok(json!({ "tuners": results, "skipped_recent": skipped }))
    }
}

impl LivetvScanHandler {
    async fn scan_one(
        &self,
        job_id: Uuid,
        tuner: &livetv_tuners::TunerRow,
        requested: bool,
        cancel: &CancellationToken,
    ) -> JobResult<Value> {
        info!(job_id = %job_id, tuner_id = %tuner.id, base_url = %tuner.base_url, "Scanning tuner lineup");

        let discover = hdhomerun::discover(&tuner.base_url)
            .await
            .map_err(|e| JobError::retryable(format!("Tuner discover failed: {e}")))?;

        livetv_tuners::upsert(
            &self.pool,
            livetv_tuners::CreateTuner {
                device_id: discover.device_id.clone(),
                friendly_name: discover
                    .friendly_name
                    .clone()
                    .unwrap_or_else(|| tuner.friendly_name.clone()),
                model_number: discover.model_number.clone(),
                firmware_version: discover.firmware_version.clone(),
                base_url: tuner.base_url.clone(),
                device_auth: discover.device_auth.clone(),
                tuner_count: discover.tuner_count.unwrap_or(tuner.tuner_count),
            },
        )
        .await?;

        let lineup = hdhomerun::lineup(&tuner.base_url)
            .await
            .map_err(|e| JobError::retryable(format!("Tuner lineup fetch failed: {e}")))?;

        // A device that answers mid-retune returns an empty lineup. Taking that at
        // face value would disable every channel on it.
        if lineup.is_empty() {
            return Err(JobError::retryable(
                "Tuner returned an empty lineup; not treating that as 'no channels'".to_string(),
            ));
        }

        nats_jobs::check_cancelled(cancel)?;

        let sdt_names = read_mux_names(&tuner.base_url, &lineup, cancel).await;

        let with_guide: HashSet<(i64, i32)> =
            livetv_channels::physical_ids_with_guide(&self.pool, tuner.id)
                .await?
                .into_iter()
                .collect();

        let mut seen = Vec::with_capacity(lineup.len());
        let mut created = 0usize;
        let mut tv = 0usize;
        let mut radio = 0usize;
        let mut dead = 0usize;

        let mut renamed = 0usize;

        for entry in &lineup {
            let program_number = i32::try_from(entry.program_number).unwrap_or(0);
            let has_guide = with_guide.contains(&(entry.frequency, program_number));
            // The mux knows the whole name; the lineup only ever knew 15 bytes of it.
            let scan_name = match sdt_names.get(&(entry.frequency, program_number)) {
                Some(full) => {
                    if entry.guide_name.as_deref() != Some(full.as_str()) {
                        renamed += 1;
                    }
                    Some(full.clone())
                }
                None => entry.guide_name.clone(),
            };
            let class = classify::classify(entry, has_guide);
            match class.kind {
                classify::Kind::Tv => tv += 1,
                classify::Kind::Radio => radio += 1,
                classify::Kind::Dead => dead += 1,
            }

            let (row, was_created) = livetv_channels::upsert_scan(
                &self.pool,
                livetv_channels::UpsertScanChannel {
                    tuner_id: tuner.id,
                    lcn: entry.lcn(),
                    scan_name,
                    frequency: entry.frequency,
                    program_number,
                    onid: entry
                        .original_network_id
                        .and_then(|v| i32::try_from(v).ok()),
                    tsid: entry
                        .transport_stream_id
                        .and_then(|v| i32::try_from(v).ok()),
                    modulation: entry.modulation.clone(),
                    video_codec: entry.video_codec.clone(),
                    audio_codec: entry.audio_codec.clone(),
                    hd: entry.is_hd(),
                    signal_strength: entry.signal_strength,
                    signal_quality: entry.signal_quality,
                    initial_kind: class.kind.as_str().to_string(),
                    initial_enabled: class.enabled_on_first_scan,
                },
            )
            .await?;
            if was_created {
                created += 1;
            }
            seen.push(row.id);
        }

        // Disabling is the one destructive step here — `upsert_scan` leaves every
        // curated column alone, but `enabled` never comes back on its own, so a
        // weak-signal scan that half-read the lineup would cost the curation. A
        // person pressing Scan is asking for it; the schedule has to be sure.
        let (before, _) = livetv_channels::count_by_tuner(&self.pool, tuner.id).await?;
        let disabled = if lineup_looks_truncated(seen.len(), before) && !requested {
            warn!(
                tuner_id = %tuner.id,
                before,
                seen = seen.len(),
                "Lineup shrank by more than half; leaving the missing channels enabled"
            );
            0
        } else {
            livetv_channels::disable_unseen(&self.pool, tuner.id, &seen).await?
        };
        livetv_tuners::mark_scanned(&self.pool, tuner.id).await?;

        let (total, enabled) = livetv_channels::count_by_tuner(&self.pool, tuner.id).await?;

        match search::fetch_all_livetv_channel_docs(&self.pool).await {
            Ok(docs) => {
                if let Err(e) = self.search.reindex_all_livetv_channels(docs).await {
                    warn!(error = %e, "Failed to reindex Live TV channels in MeiliSearch");
                }
            }
            Err(e) => warn!(error = %e, "Failed to fetch Live TV channel docs for reindex"),
        }

        // A rescan can renumber LCNs, and the guide feed joins on LCN, so the
        // guide has to be re-derived before it can be trusted again.
        if let Err(e) = self
            .nats_publisher
            .enqueue_job(
                "kobbler",
                "livetv_guide_sync",
                json!({ "tuner_id": tuner.id }),
            )
            .await
        {
            warn!(error = %e, "Failed to enqueue Live TV guide sync after scan");
        }

        let _ = self
            .library_events
            .send(crate::events::LibraryEvent::LivetvScanProgress {
                tuner_id: tuner.id,
                found: lineup.len() as i64,
                done: true,
            });

        info!(
            tuner_id = %tuner.id,
            scanned = lineup.len(),
            created,
            tv,
            radio,
            dead,
            disabled,
            renamed,
            "Live TV scan complete"
        );

        Ok(json!({
            "tuner_id": tuner.id,
            "scanned": lineup.len(),
            "created": created,
            "tv": tv,
            "radio": radio,
            "dead": dead,
            "disabled_missing": disabled,
            "named_from_mux": renamed,
            "channel_count": total,
            "enabled_channel_count": enabled,
        }))
    }
}

/// Tune each mux once for its SDT, keyed the way the lineup is: by
/// `(frequency, program_number)`, which is physical and survives an LCN
/// renumber. One mux at a time — a scan should never hold more than one of the
/// four tuners — and a mux that will not answer keeps its truncated names rather
/// than failing the scan.
async fn read_mux_names(
    base_url: &str,
    lineup: &[hdhomerun::LineupEntry],
    cancel: &CancellationToken,
) -> HashMap<(i64, i32), String> {
    let frequencies: BTreeSet<i64> = lineup.iter().map(|e| e.frequency).collect();
    let mut names = HashMap::new();

    for frequency in frequencies {
        if cancel.is_cancelled() {
            break;
        }
        match sdt::fetch_mux(base_url, frequency).await {
            Ok(services) if !services.is_empty() => {
                for s in services {
                    names.insert((frequency, s.service_id as i32), s.name);
                }
            }
            Ok(_) => warn!(frequency, "Mux carried no SDT; keeping the lineup names"),
            Err(e) => warn!(frequency, error = %e, "Could not read the mux SDT"),
        }
    }

    info!(
        services = names.len(),
        "Read service names from the mux SDT"
    );
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scheduler restarts with the API and remembers nothing, so a box that
    /// reboots often would rescan on every boot without this.
    #[test]
    fn a_tuner_scanned_recently_is_left_alone_until_the_window_passes() {
        let now = OffsetDateTime::now_utc();
        assert!(due_for_rescan(None, now), "a tuner never scanned is due");
        assert!(!due_for_rescan(Some(now), now));
        assert!(!due_for_rescan(Some(now - Duration::hours(19)), now));
        assert!(due_for_rescan(Some(now - Duration::hours(21)), now));
    }

    /// `enabled` never comes back on its own, so an unattended scan that half-read
    /// the lineup would quietly cost the curation.
    #[test]
    fn a_lineup_that_came_back_half_empty_is_not_trusted_to_disable_anything() {
        assert!(lineup_looks_truncated(0, 107));
        assert!(lineup_looks_truncated(50, 107));
        assert!(!lineup_looks_truncated(54, 107));
        assert!(!lineup_looks_truncated(107, 107));
        // A genuine retune that adds services must not read as a shrink.
        assert!(!lineup_looks_truncated(120, 107));
        // Nothing known yet — the first scan has nothing to lose.
        assert!(!lineup_looks_truncated(0, 0));
    }
}
