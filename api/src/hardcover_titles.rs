use std::collections::HashSet;
use std::time::Duration;

use sqlx::PgPool;
use time::{OffsetDateTime, Time};
use tokio_util::sync::CancellationToken;

use crate::jobs::{JobError, JobResult};
use crate::metadata::hardcover::{HardcoverProvider, MostReadTitle};
use crate::notifications::prefs::{Zone, in_quiet_hours};
use crate::repositories::hardcover_titles as repo;

/// By 100,000 a book has ~17 readers on Hardcover; past that the list is long-tail editions
/// nobody types into a search box. 100 requests a week against the free plan's 5,000 a day.
#[derive(Debug, Clone, Copy)]
pub struct TitleLimits {
    pub total: u32,
    pub page: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct OffPeak {
    pub from: Time,
    pub to: Time,
}

impl OffPeak {
    pub fn contains(&self, instant: OffsetDateTime) -> bool {
        in_quiet_hours(self.from, self.to, Zone::ServerLocal.local_time(instant))
    }
}

/// The sync shares Hardcover's limiter and key with enrichment, author/series checks and
/// reading sync, so it starts only overnight, leaves a gap between pages, and stands aside
/// whenever one of those jobs is running.
#[derive(Debug, Clone, Copy)]
pub struct SyncPlan {
    pub limits: TitleLimits,
    pub off_peak: OffPeak,
    pub page_gap: Duration,
    pub busy_poll: Duration,
}

impl Default for SyncPlan {
    fn default() -> Self {
        Self {
            limits: TitleLimits {
                total: 100_000,
                page: 1_000,
            },
            off_peak: OffPeak {
                from: Time::from_hms(2, 0, 0).expect("valid time"),
                to: Time::from_hms(6, 0, 0).expect("valid time"),
            },
            page_gap: Duration::from_secs(5),
            busy_poll: Duration::from_secs(30),
        }
    }
}

pub const BOOK_PROVIDER_JOBS: &[&str] = &[
    "book_enrich",
    "enrich_audiobook",
    "series_enrich",
    "series_sync_check",
    "author_enrich",
    "author_sync_check",
    "book_want",
    "book_want_check",
    "hardcover_reconcile",
    "sync_pull",
];

pub async fn most_read(
    pool: &PgPool,
    hardcover: &HardcoverProvider,
    plan: SyncPlan,
    cancel: &CancellationToken,
) -> JobResult<Vec<MostReadTitle>> {
    let limits = plan.limits;
    let mut seen = HashSet::new();
    let mut titles = Vec::new();
    let mut offset = 0;
    while offset < limits.total {
        if offset > 0 {
            pause(cancel, plan.page_gap).await?;
        }
        while repo::book_provider_jobs_running(pool, BOOK_PROVIDER_JOBS).await? {
            pause(cancel, plan.busy_poll).await?;
        }
        let page = hardcover
            .most_read_titles(offset, limits.page.min(limits.total - offset))
            .await
            .map_err(|e| JobError::retryable(format!("Hardcover offset {offset}: {e}")))?;
        let fetched = page.len() as u32;
        titles.extend(page.into_iter().filter(|t| seen.insert(t.id)));
        if fetched < limits.page {
            break;
        }
        offset += fetched;
    }
    Ok(titles)
}

async fn pause(cancel: &CancellationToken, duration: Duration) -> JobResult<()> {
    nats_jobs::check_cancelled(cancel)?;
    if !duration.is_zero() {
        tokio::select! {
            _ = tokio::time::sleep(duration) => {}
            _ = cancel.cancelled() => {}
        }
    }
    nats_jobs::check_cancelled(cancel)?;
    Ok(())
}
