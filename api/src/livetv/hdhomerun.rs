use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const DISCOVER_SERVICE_URL: &str = "https://ipv4-api.hdhomerun.com/discover";
const GUIDE_SERVICE_URL: &str = "https://api.hdhomerun.com/api/guide.php";

/// One un-paged `guide.php` call returns ~4 h and the walk steps by whatever the
/// thinnest channel has left, so a lineup whose channels run out at different times
/// needs more, shorter, steps than the span alone suggests. Measured against a live
/// 85-channel lineup: 12 pages to exhaust the feed, which then answers 400 — every
/// channel out to 48 h, the deepest to 54 h, 4933 airings. The cap is headroom over
/// that, not a target; the 400 is what actually ends the walk.
const GUIDE_MAX_PAGES: usize = 24;

#[derive(Debug, thiserror::Error)]
pub enum HdhrError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("device returned {status} for {url}")]
    Status { status: u16, url: String },
    #[error("invalid response from {url}: {source}")]
    Decode {
        url: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("device has no DeviceAuth token")]
    MissingDeviceAuth,
}

pub type Result<T> = std::result::Result<T, HdhrError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Discover {
    #[serde(rename = "DeviceID")]
    pub device_id: String,
    #[serde(rename = "FriendlyName")]
    pub friendly_name: Option<String>,
    #[serde(rename = "ModelNumber")]
    pub model_number: Option<String>,
    #[serde(rename = "FirmwareName")]
    pub firmware_name: Option<String>,
    #[serde(rename = "FirmwareVersion")]
    pub firmware_version: Option<String>,
    #[serde(rename = "DeviceAuth")]
    pub device_auth: Option<String>,
    #[serde(rename = "BaseURL")]
    pub base_url: Option<String>,
    #[serde(rename = "LineupURL")]
    pub lineup_url: Option<String>,
    #[serde(rename = "TunerCount")]
    pub tuner_count: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkDevice {
    #[serde(rename = "DeviceID")]
    pub device_id: String,
    #[serde(rename = "LocalIP")]
    pub local_ip: Option<String>,
    #[serde(rename = "BaseURL")]
    pub base_url: Option<String>,
    #[serde(rename = "DiscoverURL")]
    pub discover_url: Option<String>,
    #[serde(rename = "LineupURL")]
    pub lineup_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineupEntry {
    #[serde(rename = "GuideNumber")]
    pub guide_number: String,
    #[serde(rename = "GuideName")]
    pub guide_name: Option<String>,
    #[serde(rename = "OriginalNetworkID")]
    pub original_network_id: Option<i64>,
    #[serde(rename = "TransportStreamID")]
    pub transport_stream_id: Option<i64>,
    #[serde(rename = "Modulation")]
    pub modulation: Option<String>,
    #[serde(rename = "Frequency")]
    pub frequency: i64,
    #[serde(rename = "ProgramNumber")]
    pub program_number: i64,
    #[serde(rename = "VideoCodec")]
    pub video_codec: Option<String>,
    #[serde(rename = "AudioCodec")]
    pub audio_codec: Option<String>,
    // The device emits `HD: 1` or omits the key entirely — it never sends 0, so
    // absence is the only signal for SD.
    #[serde(rename = "HD")]
    pub hd: Option<i32>,
    #[serde(rename = "SignalStrength")]
    pub signal_strength: Option<i32>,
    #[serde(rename = "SignalQuality")]
    pub signal_quality: Option<i32>,
    #[serde(rename = "URL")]
    pub url: Option<String>,
}

impl LineupEntry {
    pub fn is_hd(&self) -> bool {
        self.hd.unwrap_or(0) != 0
    }

    pub fn lcn(&self) -> Option<i32> {
        self.guide_number.trim().parse::<i32>().ok()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuideChannel {
    #[serde(rename = "GuideNumber")]
    pub guide_number: String,
    #[serde(rename = "GuideName")]
    pub guide_name: Option<String>,
    #[serde(rename = "Affiliate")]
    pub affiliate: Option<String>,
    #[serde(rename = "ImageURL")]
    pub image_url: Option<String>,
    #[serde(rename = "Guide", default)]
    pub guide: Vec<GuideProgramme>,
}

impl GuideChannel {
    pub fn lcn(&self) -> Option<i32> {
        self.guide_number.trim().parse::<i32>().ok()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuideProgramme {
    #[serde(rename = "StartTime")]
    pub start_time: i64,
    #[serde(rename = "EndTime")]
    pub end_time: i64,
    #[serde(rename = "Title")]
    pub title: String,
    #[serde(rename = "EpisodeNumber")]
    pub episode_number: Option<String>,
    #[serde(rename = "EpisodeTitle")]
    pub episode_title: Option<String>,
    #[serde(rename = "Synopsis")]
    pub synopsis: Option<String>,
    #[serde(rename = "OriginalAirdate")]
    pub original_airdate: Option<i64>,
    #[serde(rename = "SeriesID")]
    pub series_id: Option<String>,
    #[serde(rename = "ImageURL")]
    pub image_url: Option<String>,
    #[serde(rename = "PosterURL")]
    pub poster_url: Option<String>,
    #[serde(rename = "Filter", default)]
    pub filter: Vec<String>,
    #[serde(rename = "First")]
    pub first: Option<i32>,
}

impl GuideProgramme {
    pub fn is_first(&self) -> bool {
        self.first.unwrap_or(0) != 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunerStatus {
    #[serde(rename = "Resource")]
    pub resource: String,
    #[serde(rename = "VctNumber")]
    pub vct_number: Option<String>,
    #[serde(rename = "VctName")]
    pub vct_name: Option<String>,
    #[serde(rename = "Frequency")]
    pub frequency: Option<i64>,
    #[serde(rename = "SignalStrengthPercent")]
    pub signal_strength_percent: Option<i32>,
    #[serde(rename = "SignalQualityPercent")]
    pub signal_quality_percent: Option<i32>,
    #[serde(rename = "SymbolQualityPercent")]
    pub symbol_quality_percent: Option<i32>,
    #[serde(rename = "TargetIP")]
    pub target_ip: Option<String>,
    #[serde(rename = "NetworkRate")]
    pub network_rate: Option<i64>,
}

impl TunerStatus {
    pub fn in_use(&self) -> bool {
        self.frequency.is_some()
    }
}

fn normalize_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    }
}

async fn fetch<T: serde::de::DeserializeOwned>(url: &str) -> Result<T> {
    let resp = crate::http::new_client().get(url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(HdhrError::Status {
            status: status.as_u16(),
            url: url.to_string(),
        });
    }
    let body = resp.text().await?;
    serde_json::from_str(&body).map_err(|source| HdhrError::Decode {
        url: url.to_string(),
        source,
    })
}

pub async fn discover(base_url: &str) -> Result<Discover> {
    fetch(&format!("{}/discover.json", normalize_base_url(base_url))).await
}

pub async fn discover_network() -> Result<Vec<NetworkDevice>> {
    fetch(DISCOVER_SERVICE_URL).await
}

pub async fn lineup(base_url: &str) -> Result<Vec<LineupEntry>> {
    fetch(&format!(
        "{}/lineup.json?show=found&tuning=1",
        normalize_base_url(base_url)
    ))
    .await
}

/// Fold a page into the accumulator, returning the furthest `EndTime` it carried
/// and how many programmes were new. A channel absent from the accumulator is
/// appended; one already there gains only the airings it does not hold yet, so
/// the overlap every page ships is idempotent.
/// Where the next page has to start so that no channel is left with a hole.
///
/// The earliest moment any channel runs out of guide — not the furthest moment any
/// channel reaches. A 24/7 channel carries a single all-day block, so paging on the
/// furthest end time jumped `Start` past most of a day in one step and every other
/// channel lost the hours in between: the grid showed an hour of programmes, then
/// nothing until the evening.
///
/// Channels that cannot advance past [`after`] are ignored rather than pinning the walk
/// in place — a channel that has simply stopped broadcasting must not end the sync for
/// everyone else.
fn next_page_start(merged: &[GuideChannel], after: i64) -> Option<i64> {
    merged
        .iter()
        .filter_map(|c| c.guide.iter().map(|p| p.end_time).max())
        .filter(|end| *end > after)
        .min()
}

fn merge_guide_page(into: &mut Vec<GuideChannel>, page: Vec<GuideChannel>) -> usize {
    let index: HashMap<String, usize> = into
        .iter()
        .enumerate()
        .map(|(i, c)| (c.guide_number.clone(), i))
        .collect();
    let mut seen: HashSet<(usize, i64)> = into
        .iter()
        .enumerate()
        .flat_map(|(i, c)| c.guide.iter().map(move |p| (i, p.start_time)))
        .collect();

    let mut added = 0usize;

    for channel in page {
        let slot = match index.get(&channel.guide_number) {
            Some(i) => *i,
            None => {
                into.push(GuideChannel {
                    guide: Vec::new(),
                    ..channel.clone()
                });
                into.len() - 1
            }
        };
        for p in channel.guide {
            if seen.insert((slot, p.start_time)) {
                into[slot].guide.push(p);
                added += 1;
            }
        }
    }

    added
}

/// The feed is a moving ~4 h window, not a dump: without `Start` it answers from
/// now to about four hours out, which is less than the refresh interval and left
/// the guide grid empty for most of a day. Paging on the furthest end time walks
/// it to the ~48 h ceiling, where it answers 400.
pub async fn guide(device_auth: &str) -> Result<Vec<GuideChannel>> {
    if device_auth.trim().is_empty() {
        return Err(HdhrError::MissingDeviceAuth);
    }
    let auth = urlencoding::encode(device_auth);
    let mut merged: Vec<GuideChannel> = Vec::new();
    let mut start: Option<i64> = None;

    for _ in 0..GUIDE_MAX_PAGES {
        let url = match start {
            Some(s) => format!("{GUIDE_SERVICE_URL}?DeviceAuth={auth}&Start={s}"),
            None => format!("{GUIDE_SERVICE_URL}?DeviceAuth={auth}"),
        };
        let page: Vec<GuideChannel> = match fetch(&url).await {
            Ok(page) => page,
            // Past the ceiling the feed answers 400. That is the end of the
            // guide, not a failed sync — but only once a page has landed.
            Err(HdhrError::Status { status: 400, .. }) if !merged.is_empty() => break,
            Err(e) => return Err(e),
        };

        let added = merge_guide_page(&mut merged, page);
        let Some(next) = next_page_start(&merged, start.unwrap_or(0)) else {
            break;
        };
        if added == 0 || start.is_some_and(|s| next <= s) {
            break;
        }
        start = Some(next);
    }

    Ok(merged)
}

pub async fn status(base_url: &str) -> Result<Vec<TunerStatus>> {
    fetch(&format!("{}/status.json", normalize_base_url(base_url))).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUIDE_FIXTURE: &str = include_str!("../../tests/fixtures/livetv/guide.json");

    #[test]
    fn normalizes_base_urls() {
        assert_eq!(normalize_base_url("192.0.2.10"), "http://192.0.2.10");
        assert_eq!(
            normalize_base_url("http://192.0.2.10/"),
            "http://192.0.2.10"
        );
        assert_eq!(normalize_base_url("https://tuner/"), "https://tuner");
    }

    #[test]
    fn hd_is_present_or_absent_never_zero() {
        let sd: LineupEntry = serde_json::from_str(
            r#"{"GuideNumber":"1","GuideName":"10","Frequency":219500000,"ProgramNumber":1589}"#,
        )
        .unwrap();
        assert!(!sd.is_hd());
        let hd: LineupEntry = serde_json::from_str(
            r#"{"GuideNumber":"5","Frequency":529500000,"ProgramNumber":2049,"HD":1}"#,
        )
        .unwrap();
        assert!(hd.is_hd());
        assert_eq!(hd.lcn(), Some(5));
    }

    fn page(number: &str, spans: &[(i64, i64)]) -> GuideChannel {
        GuideChannel {
            guide_number: number.to_string(),
            guide_name: Some("CALLSIGN".into()),
            affiliate: None,
            image_url: None,
            guide: spans
                .iter()
                .map(|(s, e)| GuideProgramme {
                    start_time: *s,
                    end_time: *e,
                    title: format!("p{s}"),
                    episode_number: None,
                    episode_title: None,
                    synopsis: None,
                    original_airdate: None,
                    series_id: None,
                    image_url: None,
                    poster_url: None,
                    filter: vec![],
                    first: None,
                })
                .collect(),
        }
    }

    #[test]
    fn a_page_contributes_only_the_airings_the_accumulator_lacks() {
        let mut merged = Vec::new();

        let added = merge_guide_page(&mut merged, vec![page("2", &[(0, 10), (10, 20)])]);
        assert_eq!((next_page_start(&merged, 0), added), (Some(20), 2));

        // Every page overlaps the one before it, and the overlap must not double up.
        let added = merge_guide_page(&mut merged, vec![page("2", &[(10, 20), (20, 30)])]);
        assert_eq!((next_page_start(&merged, 20), added), (Some(30), 1));
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].guide.len(), 3);

        // A channel first seen on a later page joins the accumulator whole.
        let added = merge_guide_page(&mut merged, vec![page("22", &[(20, 30)])]);
        assert_eq!(added, 1);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[1].guide_number, "22");
    }

    #[test]
    fn a_page_that_repeats_the_previous_one_adds_nothing_and_stops_the_walk() {
        let mut merged = Vec::new();
        merge_guide_page(&mut merged, vec![page("2", &[(0, 10)])]);
        let added = merge_guide_page(&mut merged, vec![page("2", &[(0, 10)])]);
        assert_eq!(next_page_start(&merged, 0), Some(10));
        assert_eq!(
            added, 0,
            "no forward progress must be visible to the caller"
        );
    }

    /// The bug this fixes: one 24/7 channel dragged `Start` past most of a day, and the
    /// guide grid showed an hour of programmes and then nothing until the evening.
    #[test]
    fn the_walk_resumes_where_the_thinnest_channel_ran_out() {
        let mut merged = Vec::new();
        merge_guide_page(
            &mut merged,
            vec![
                page("2", &[(0, 3600), (3600, 7200)]),
                page("99", &[(0, 86_400)]),
            ],
        );

        assert_eq!(next_page_start(&merged, 0), Some(7200));
    }

    /// A channel that has stopped broadcasting has no more guide to give and must not
    /// hold the walk at its own last airing for everybody else.
    #[test]
    fn a_channel_that_cannot_advance_does_not_stall_the_walk() {
        let mut merged = Vec::new();
        merge_guide_page(
            &mut merged,
            vec![page("2", &[(0, 7200)]), page("3", &[(0, 14_400)])],
        );
        merge_guide_page(&mut merged, vec![page("3", &[(14_400, 21_600)])]);

        assert_eq!(next_page_start(&merged, 7200), Some(21_600));
    }

    #[test]
    fn one_unpaged_call_is_a_few_hours_not_a_day() {
        // Why `guide` pages at all: the fixture is one live `guide.php` response.
        let channels: Vec<GuideChannel> = serde_json::from_str(GUIDE_FIXTURE).unwrap();
        let spans: Vec<i64> = channels
            .iter()
            .flat_map(|c| c.guide.iter().map(|p| p.end_time))
            .collect();
        let earliest = channels
            .iter()
            .flat_map(|c| c.guide.iter().map(|p| p.start_time))
            .min()
            .unwrap();
        let hours = (spans.iter().max().unwrap() - earliest) / 3600;
        assert!(
            hours < 24,
            "a single call covered {hours} h, so paging would be pointless"
        );
    }

    #[test]
    fn parses_a_guide_programme() {
        let p: GuideProgramme = serde_json::from_str(
            r#"{"StartTime":1787974200,"EndTime":1787976000,"First":1,
                "Title":"Country House Facelift","EpisodeNumber":"S02E05",
                "OriginalAirdate":1787961600,"SeriesID":"C33677068ENP2IK"}"#,
        )
        .unwrap();
        assert!(p.is_first());
        assert!(p.filter.is_empty());
        assert_eq!(p.series_id.as_deref(), Some("C33677068ENP2IK"));
    }
}
