//! Channel stacking. A broadcaster puts one schedule out as several services — an
//! H.264 HD, an MPEG-2 SD and a regional copy on another mux — and the guide cannot
//! tell them apart, so a viewer is shown the same programme three times. Channels whose
//! next day of guide is identical are one stack, shown as its best member, with the
//! rest kept as tune fallbacks.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::repositories::{self, livetv_channels};

const WINDOW_HOURS: i64 = 24;
/// One or two entries is a placeholder ("Programme information unavailable"), which a
/// dozen unrelated channels share.
const MIN_ENTRIES: i64 = 3;
/// Guide sync runs a few times a day; a minute of staleness is invisible.
const TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
pub struct Stacks {
    members: HashMap<Uuid, Arc<Vec<Uuid>>>,
    favourite: HashSet<Uuid>,
}

impl Stacks {
    pub fn build(
        channels: &[livetv_channels::ChannelRow],
        fingerprints: &[(Uuid, String)],
    ) -> Self {
        let by_id: HashMap<Uuid, &livetv_channels::ChannelRow> =
            channels.iter().map(|c| (c.id, c)).collect();
        let mut groups: HashMap<(&str, &str), Vec<&livetv_channels::ChannelRow>> = HashMap::new();
        for (id, fingerprint) in fingerprints {
            if let Some(row) = by_id.get(id).filter(|c| c.enabled) {
                groups
                    .entry((row.kind.as_str(), fingerprint.as_str()))
                    .or_default()
                    .push(row);
            }
        }

        let mut stacks = Stacks::default();
        for mut group in groups.into_values().filter(|g| g.len() > 1) {
            group.sort_by(|a, b| preference(a, b));
            let ids = Arc::new(group.iter().map(|c| c.id).collect::<Vec<_>>());
            if group.iter().any(|c| c.favourite) {
                stacks.favourite.insert(ids[0]);
            }
            for id in ids.iter() {
                stacks.members.insert(*id, ids.clone());
            }
        }
        stacks
    }

    pub fn is_hidden(&self, id: Uuid) -> bool {
        self.members.get(&id).is_some_and(|m| m[0] != id)
    }

    /// Empty for anything but a stack's primary.
    pub fn alternates(&self, id: Uuid) -> &[Uuid] {
        match self.members.get(&id) {
            Some(m) if m[0] == id => &m[1..],
            _ => &[],
        }
    }

    /// Every other member, best first — what to fall back to when `id` will not tune,
    /// whichever member the viewer picked.
    pub fn fallbacks(&self, id: Uuid) -> Vec<Uuid> {
        self.members
            .get(&id)
            .map(|m| m.iter().copied().filter(|m| *m != id).collect())
            .unwrap_or_default()
    }

    pub fn hidden_ids(&self) -> Vec<Uuid> {
        self.members
            .iter()
            .filter(|(id, m)| m[0] != **id)
            .map(|(id, _)| *id)
            .collect()
    }

    pub fn any_favourite(&self, id: Uuid) -> bool {
        self.favourite.contains(&id)
    }
}

fn codec_rank(codec: Option<&str>) -> u8 {
    match codec.unwrap_or("").to_ascii_uppercase().as_str() {
        "H264" | "AVC" | "HEVC" | "H265" => 0,
        "MPEG2" => 2,
        _ => 1,
    }
}

/// Best first: HD, a modern codec, the cleaner signal, a favourite, the lower LCN.
pub fn preference(a: &livetv_channels::ChannelRow, b: &livetv_channels::ChannelRow) -> Ordering {
    b.hd.cmp(&a.hd)
        .then_with(|| {
            codec_rank(a.video_codec.as_deref()).cmp(&codec_rank(b.video_codec.as_deref()))
        })
        .then_with(|| {
            b.signal_quality
                .unwrap_or(0)
                .cmp(&a.signal_quality.unwrap_or(0))
        })
        .then_with(|| {
            b.signal_strength
                .unwrap_or(0)
                .cmp(&a.signal_strength.unwrap_or(0))
        })
        .then_with(|| b.favourite.cmp(&a.favourite))
        .then_with(|| a.lcn.unwrap_or(i32::MAX).cmp(&b.lcn.unwrap_or(i32::MAX)))
        .then_with(|| a.id.cmp(&b.id))
}

pub async fn compute(pool: &PgPool) -> repositories::Result<Stacks> {
    let now = OffsetDateTime::now_utc();
    let fingerprints = livetv_channels::guide_fingerprints(
        pool,
        now,
        now + time::Duration::hours(WINDOW_HOURS),
        MIN_ENTRIES,
    )
    .await?;
    if fingerprints.len() < 2 {
        return Ok(Stacks::default());
    }
    let ids: Vec<Uuid> = fingerprints.iter().map(|(id, _)| *id).collect();
    let channels = livetv_channels::list_for_guide(pool, Some(ids), true).await?;
    Ok(Stacks::build(&channels, &fingerprints))
}

#[derive(Default)]
pub struct StackCache {
    cached: tokio::sync::Mutex<Option<(Instant, Arc<Stacks>)>>,
}

impl StackCache {
    pub async fn get(&self, pool: &PgPool) -> repositories::Result<Arc<Stacks>> {
        let mut cached = self.cached.lock().await;
        if let Some((at, stacks)) = cached.as_ref()
            && at.elapsed() < TTL
        {
            return Ok(stacks.clone());
        }
        let stacks = Arc::new(compute(pool).await?);
        *cached = Some((Instant::now(), stacks.clone()));
        Ok(stacks)
    }

    pub async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(lcn: i32, hd: bool, codec: &str, quality: i32) -> livetv_channels::ChannelRow {
        let now = OffsetDateTime::now_utc();
        livetv_channels::ChannelRow {
            id: Uuid::new_v4(),
            tuner_id: Uuid::nil(),
            lcn: Some(lcn),
            scan_name: None,
            callsign: None,
            frequency: 0,
            program_number: lcn,
            onid: None,
            tsid: None,
            modulation: None,
            video_codec: Some(codec.to_string()),
            audio_codec: None,
            hd,
            signal_strength: Some(100),
            signal_quality: Some(quality),
            last_seen_at: None,
            enabled: true,
            favourite: false,
            sort_order: 0,
            display_name: None,
            logo_url: None,
            kind: "tv".to_string(),
            region: None,
            network: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn hd_h264_leads_then_signal_then_lcn() {
        let sd = channel(61, false, "MPEG2", 100);
        let hd_weak = channel(6, true, "H264", 40);
        let hd_strong = channel(60, true, "H264", 90);
        let mut favourite_sd = channel(7, false, "MPEG2", 100);
        favourite_sd.favourite = true;
        let channels = vec![
            sd.clone(),
            hd_weak.clone(),
            hd_strong.clone(),
            favourite_sd.clone(),
        ];
        let fp: Vec<(Uuid, String)> = channels.iter().map(|c| (c.id, "x".to_string())).collect();
        let stacks = Stacks::build(&channels, &fp);
        assert_eq!(
            stacks.alternates(hd_strong.id),
            &[hd_weak.id, favourite_sd.id, sd.id]
        );
        assert!(stacks.is_hidden(sd.id));
        assert!(!stacks.is_hidden(hd_strong.id));
        assert!(stacks.any_favourite(hd_strong.id));
        assert_eq!(
            stacks.fallbacks(sd.id),
            vec![hd_strong.id, hd_weak.id, favourite_sd.id]
        );
    }

    #[test]
    fn radio_never_stacks_with_tv_and_a_lone_channel_is_no_stack() {
        let tv = channel(1, true, "H264", 100);
        let mut radio = channel(200, false, "", 100);
        radio.kind = "radio".to_string();
        let other = channel(2, true, "H264", 100);
        let channels = vec![tv.clone(), radio.clone(), other.clone()];
        let fp = vec![
            (tv.id, "a".to_string()),
            (radio.id, "a".to_string()),
            (other.id, "b".to_string()),
        ];
        let stacks = Stacks::build(&channels, &fp);
        assert!(stacks.hidden_ids().is_empty());
        assert!(stacks.alternates(tv.id).is_empty());
    }
}
