use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

use async_compression::tokio::bufread::GzipDecoder;
use futures::TryStreamExt;
use serde::Deserialize;
use time::Date;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::io::StreamReader;
use tokio_util::sync::CancellationToken;

use super::TitleMedia;
use crate::jobs::{JobError, JobResult};

pub const EXPORTS_URL: &str = "https://files.tmdb.org/p/exports";

/// Below roughly these ranks (popularity ~2 for films, ~3.5 for series) the exports are
/// shorts, regional dailies and talk shows nobody types into a search box, and the table
/// would only grow without adding titles anyone looks for.
#[derive(Debug, Clone, Copy)]
pub struct ExportLimits {
    pub movie: usize,
    pub tv: usize,
}

impl Default for ExportLimits {
    fn default() -> Self {
        Self {
            movie: 75_000,
            tv: 50_000,
        }
    }
}

impl ExportLimits {
    pub fn of(self, media: TitleMedia) -> usize {
        match media {
            TitleMedia::Movie => self.movie,
            TitleMedia::Tv => self.tv,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExportTitle {
    pub tmdb_id: i32,
    pub original_title: String,
    pub popularity: f32,
}

#[derive(Deserialize)]
struct ExportLine {
    id: i32,
    #[serde(alias = "original_name")]
    original_title: String,
    #[serde(default)]
    popularity: f32,
    #[serde(default)]
    adult: bool,
    #[serde(default)]
    video: bool,
}

struct Ranked(ExportTitle);

impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .popularity
            .total_cmp(&other.0.popularity)
            .then_with(|| other.0.tmdb_id.cmp(&self.0.tmdb_id))
    }
}

impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Ranked {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Ranked {}

pub fn export_url(base: &str, media: TitleMedia, date: Date) -> String {
    let prefix = match media {
        TitleMedia::Movie => "movie_ids",
        TitleMedia::Tv => "tv_series_ids",
    };
    format!(
        "{base}/{prefix}_{:02}_{:02}_{}.json.gz",
        u8::from(date.month()),
        date.day(),
        date.year()
    )
}

/// The `limit` most popular titles of one day's export, or `None` when that day has not
/// been published yet (the bucket answers 403 rather than 404 for a missing key).
pub async fn fetch(
    client: &reqwest::Client,
    url: &str,
    limit: usize,
    cancel: &CancellationToken,
) -> JobResult<Option<Vec<ExportTitle>>> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| JobError::retryable(format!("TMDB export {url}: {e}")))?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::FORBIDDEN {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(JobError::retryable(format!(
            "TMDB export {url}: HTTP {status}"
        )));
    }

    let body = StreamReader::new(response.bytes_stream().map_err(std::io::Error::other));
    let mut decoder = GzipDecoder::new(body);
    decoder.multiple_members(true);
    let mut lines = BufReader::new(decoder).lines();

    let mut top: BinaryHeap<Reverse<Ranked>> = BinaryHeap::with_capacity(limit + 1);
    let mut read = 0usize;
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| JobError::retryable(format!("TMDB export {url}: {e}")))?
    {
        read += 1;
        if read.is_multiple_of(100_000) {
            nats_jobs::check_cancelled(cancel)?;
        }
        let Ok(entry) = serde_json::from_str::<ExportLine>(&line) else {
            continue;
        };
        let original_title = entry.original_title.trim();
        if entry.adult || entry.video || original_title.is_empty() {
            continue;
        }
        let ranked = Ranked(ExportTitle {
            tmdb_id: entry.id,
            original_title: original_title.to_string(),
            popularity: entry.popularity,
        });
        if top.len() < limit {
            top.push(Reverse(ranked));
        } else if top.peek().is_some_and(|Reverse(lowest)| ranked > *lowest) {
            top.pop();
            top.push(Reverse(ranked));
        }
    }

    let mut titles: Vec<ExportTitle> = top.into_iter().map(|Reverse(r)| r.0).collect();
    titles.sort_by_key(|t| t.tmdb_id);
    titles.dedup_by_key(|t| t.tmdb_id);
    Ok(Some(titles))
}
