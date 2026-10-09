//! DVB teletext data-unit rate metering, straight off the transport stream.
//!
//! No decoder is involved — this counts `data_unit_id == 0x03` units per second
//! on the teletext PID. Measured on a real Seven capture the rate steps between
//! ~25/s in programme and ~50/s across an ad break, within one frame of the
//! black+silence cut points.

use std::collections::HashMap;
use std::path::Path;

const TS_PACKET: usize = 188;
const TS_SYNC: u8 = 0x47;
const PTS_HZ: f64 = 90_000.0;
const PTS_WRAP: u64 = 1 << 33;
/// A teletext PES is a few hundred bytes; anything past this is a desync.
const MAX_PES: usize = 64 * 1024;
const COMPACT_AFTER: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitCount {
    pub second: u32,
    pub units: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Low,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TeletextRun {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub level: Level,
}

#[derive(Debug, Clone)]
pub struct TeletextConfig {
    pub min_run_secs: f64,
    pub smoothing_window: usize,
    pub smoothing_needed: usize,
    pub min_level_separation: f64,
    pub max_ad_fraction: f64,
}

impl Default for TeletextConfig {
    fn default() -> Self {
        Self {
            min_run_secs: 15.0,
            smoothing_window: 7,
            smoothing_needed: 4,
            min_level_separation: 8.0,
            max_ad_fraction: 0.40,
        }
    }
}

pub struct TeletextMeter {
    buf: Vec<u8>,
    pos: usize,
    pmt_pids: Vec<u16>,
    teletext_pid: Option<u16>,
    pes: Vec<u8>,
    collecting: bool,
    first_pts: Option<u64>,
    last_raw_pts: Option<u64>,
    wraps: u64,
    buckets: HashMap<u32, u32>,
    max_second: u32,
    counts: Vec<UnitCount>,
}

impl Default for TeletextMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl TeletextMeter {
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            pos: 0,
            pmt_pids: Vec::new(),
            teletext_pid: None,
            pes: Vec::new(),
            collecting: false,
            first_pts: None,
            last_raw_pts: None,
            wraps: 0,
            buckets: HashMap::new(),
            max_second: 0,
            counts: Vec::new(),
        }
    }

    pub fn teletext_pid(&self) -> Option<u16> {
        self.teletext_pid
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        loop {
            if self.buf.len() - self.pos < TS_PACKET {
                break;
            }
            if self.buf[self.pos] != TS_SYNC {
                match self.buf[self.pos + 1..].iter().position(|b| *b == TS_SYNC) {
                    Some(off) => {
                        self.pos += off + 1;
                        continue;
                    }
                    None => {
                        self.pos = self.buf.len();
                        break;
                    }
                }
            }
            let packet: [u8; TS_PACKET] = match self.buf[self.pos..self.pos + TS_PACKET].try_into()
            {
                Ok(p) => p,
                Err(_) => break,
            };
            self.pos += TS_PACKET;
            self.packet(&packet);
        }
        if self.pos >= COMPACT_AFTER {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
    }

    pub fn finish(&mut self) {
        self.flush_pes();
        self.materialise();
    }

    pub fn counts(&self) -> &[UnitCount] {
        &self.counts
    }

    fn materialise(&mut self) {
        self.counts.clear();
        if self.buckets.is_empty() {
            return;
        }
        self.counts.reserve(self.max_second as usize + 1);
        for second in 0..=self.max_second {
            self.counts.push(UnitCount {
                second,
                units: self.buckets.get(&second).copied().unwrap_or(0),
            });
        }
    }

    fn packet(&mut self, p: &[u8; TS_PACKET]) {
        if p[1] & 0x80 != 0 {
            return;
        }
        let pid = (u16::from(p[1] & 0x1F) << 8) | u16::from(p[2]);
        let afc = (p[3] >> 4) & 0x03;
        if afc == 0 || afc == 2 {
            return;
        }
        let mut off = 4usize;
        if afc == 3 {
            let af_len = usize::from(p[4]);
            off = 5 + af_len;
            if off >= TS_PACKET {
                return;
            }
        }
        let pusi = p[1] & 0x40 != 0;
        let payload = &p[off..];

        if pid == 0 {
            self.section(payload, pusi, Self::parse_pat);
            return;
        }
        if self.teletext_pid.is_none() && self.pmt_pids.contains(&pid) {
            self.section(payload, pusi, Self::parse_pmt);
            return;
        }
        if Some(pid) == self.teletext_pid {
            if pusi {
                self.flush_pes();
                self.collecting = true;
            }
            if self.collecting && self.pes.len() < MAX_PES {
                self.pes.extend_from_slice(payload);
            }
        }
    }

    fn section(&mut self, payload: &[u8], pusi: bool, parse: fn(&mut Self, &[u8])) {
        if !pusi || payload.is_empty() {
            return;
        }
        let pointer = usize::from(payload[0]);
        let start = 1 + pointer;
        if start >= payload.len() {
            return;
        }
        parse(self, &payload[start..]);
    }

    fn parse_pat(&mut self, s: &[u8]) {
        if s.len() < 8 || s[0] != 0x00 {
            return;
        }
        let section_len = ((usize::from(s[1] & 0x0F)) << 8) | usize::from(s[2]);
        let end = (3 + section_len).min(s.len()).saturating_sub(4);
        let mut i = 8usize;
        while i + 4 <= end {
            let program = (u16::from(s[i]) << 8) | u16::from(s[i + 1]);
            let pid = (u16::from(s[i + 2] & 0x1F) << 8) | u16::from(s[i + 3]);
            if program != 0 && !self.pmt_pids.contains(&pid) {
                self.pmt_pids.push(pid);
            }
            i += 4;
        }
    }

    fn parse_pmt(&mut self, s: &[u8]) {
        if s.len() < 12 || s[0] != 0x02 {
            return;
        }
        let section_len = ((usize::from(s[1] & 0x0F)) << 8) | usize::from(s[2]);
        let end = (3 + section_len).min(s.len()).saturating_sub(4);
        let program_info_len = ((usize::from(s[10] & 0x0F)) << 8) | usize::from(s[11]);
        let mut i = 12 + program_info_len;
        while i + 5 <= end {
            let stream_type = s[i];
            let pid = (u16::from(s[i + 1] & 0x1F) << 8) | u16::from(s[i + 2]);
            let es_len = ((usize::from(s[i + 3] & 0x0F)) << 8) | usize::from(s[i + 4]);
            let desc_start = i + 5;
            let desc_end = (desc_start + es_len).min(end);
            if stream_type == 0x06 && has_descriptor(&s[desc_start..desc_end], 0x56) {
                self.teletext_pid = Some(pid);
                return;
            }
            i = desc_start + es_len;
        }
    }

    fn flush_pes(&mut self) {
        if !self.collecting {
            return;
        }
        let pes = std::mem::take(&mut self.pes);
        self.collecting = false;
        let Some((raw_pts, units)) = parse_teletext_pes(&pes) else {
            return;
        };
        // 33-bit PTS rolls over roughly every 26.5 h; a recording can straddle it.
        if let Some(prev) = self.last_raw_pts
            && raw_pts + PTS_WRAP / 2 < prev
        {
            self.wraps += 1;
        }
        self.last_raw_pts = Some(raw_pts);
        let pts = raw_pts + self.wraps * PTS_WRAP;
        let base = *self.first_pts.get_or_insert(pts);
        if pts < base {
            return;
        }
        let second = ((pts - base) as f64 / PTS_HZ) as u32;
        *self.buckets.entry(second).or_insert(0) += units;
        self.max_second = self.max_second.max(second);
    }
}

fn has_descriptor(desc: &[u8], tag: u8) -> bool {
    let mut i = 0usize;
    while i + 2 <= desc.len() {
        let t = desc[i];
        let len = usize::from(desc[i + 1]);
        if t == tag {
            return true;
        }
        i += 2 + len;
    }
    false
}

fn parse_pts(b: &[u8]) -> Option<u64> {
    if b.len() < 5 {
        return None;
    }
    Some(
        (u64::from(b[0] >> 1 & 0x07) << 30)
            | (u64::from(b[1]) << 22)
            | (u64::from(b[2] >> 1) << 15)
            | (u64::from(b[3]) << 7)
            | u64::from(b[4] >> 1),
    )
}

fn parse_teletext_pes(pes: &[u8]) -> Option<(u64, u32)> {
    if pes.len() < 14 || pes[0] != 0x00 || pes[1] != 0x00 || pes[2] != 0x01 {
        return None;
    }
    if pes[7] & 0x80 == 0 {
        return None;
    }
    let pts = parse_pts(&pes[9..])?;
    let header_len = usize::from(pes[8]);
    let body_start = 9 + header_len;
    if body_start >= pes.len() {
        return None;
    }
    let body = &pes[body_start..];
    if !(0x10..=0x1F).contains(&body[0]) {
        return None;
    }
    let mut units = 0u32;
    let mut i = 1usize;
    while i + 2 <= body.len() {
        let id = body[i];
        let len = usize::from(body[i + 1]);
        if id == 0x03 {
            units += 1;
        }
        i += 2 + len;
    }
    Some((pts, units))
}

// =============================================================================
// Segmentation
// =============================================================================

/// The programme/ad polarity is deliberately not hard-coded. It was measured on
/// exactly one network (Seven, low in programme), and a broadcaster that inserts
/// subtitles the other way round would invert it. Both levels are derived from
/// the observed distribution and the ad level is decided by which one occupies
/// the minority of the runtime.
pub fn segment(counts: &[UnitCount], cfg: &TeletextConfig) -> Vec<TeletextRun> {
    let Some((rates, base)) = dense_rates(counts) else {
        return Vec::new();
    };
    if rates.len() < 8 {
        return Vec::new();
    }
    let Some((low, high)) = two_levels(&rates) else {
        return Vec::new();
    };
    if high - low < cfg.min_level_separation {
        return Vec::new();
    }
    let threshold = (low + high) / 2.0;

    let raw: Vec<bool> = rates.iter().map(|r| *r >= threshold).collect();
    let smoothed = smooth(&raw, cfg.smoothing_window, cfg.smoothing_needed);
    let mut runs = runs_of(&smoothed, base);
    absorb_short_runs(&mut runs, cfg.min_run_secs);

    let total: f64 = runs.iter().map(|r| r.end_seconds - r.start_seconds).sum();
    if total <= 0.0 {
        return Vec::new();
    }
    let high_secs: f64 = runs
        .iter()
        .filter(|r| r.level == Level::High)
        .map(|r| r.end_seconds - r.start_seconds)
        .sum();
    let minority = (high_secs / total).min(1.0 - high_secs / total);
    if minority <= 0.0 || minority > cfg.max_ad_fraction {
        return Vec::new();
    }
    runs
}

/// The ad spans only — what the fusion layer consumes.
pub fn ad_runs(counts: &[UnitCount], cfg: &TeletextConfig) -> Vec<(f64, f64)> {
    let runs = segment(counts, cfg);
    let Some(ad_level) = minority_level(&runs) else {
        return Vec::new();
    };
    runs.iter()
        .filter(|r| r.level == ad_level)
        .map(|r| (r.start_seconds, r.end_seconds))
        .collect()
}

fn minority_level(runs: &[TeletextRun]) -> Option<Level> {
    if runs.is_empty() {
        return None;
    }
    let span = |lvl: Level| -> f64 {
        runs.iter()
            .filter(|r| r.level == lvl)
            .map(|r| r.end_seconds - r.start_seconds)
            .sum()
    };
    let high = span(Level::High);
    let low = span(Level::Low);
    if high == 0.0 || low == 0.0 {
        return None;
    }
    Some(if high < low { Level::High } else { Level::Low })
}

fn dense_rates(counts: &[UnitCount]) -> Option<(Vec<f64>, u32)> {
    let first = counts.first()?.second;
    let last = counts.last()?.second;
    if last < first {
        return None;
    }
    let mut rates = vec![0.0f64; (last - first) as usize + 1];
    for c in counts {
        if c.second >= first {
            rates[(c.second - first) as usize] = f64::from(c.units);
        }
    }
    Some((rates, first))
}

/// Two-means on one dimension, seeded from p10/p90 so a heavily imbalanced
/// split still finds the minority level.
fn two_levels(rates: &[f64]) -> Option<(f64, f64)> {
    let mut sorted = rates.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    let mut low = p(0.10);
    let mut high = p(0.90);
    if high <= low {
        return None;
    }
    for _ in 0..25 {
        let mid = (low + high) / 2.0;
        let (mut ls, mut ln, mut hs, mut hn) = (0.0f64, 0usize, 0.0f64, 0usize);
        for r in rates {
            if *r < mid {
                ls += *r;
                ln += 1;
            } else {
                hs += *r;
                hn += 1;
            }
        }
        if ln == 0 || hn == 0 {
            return None;
        }
        let (nl, nh) = (ls / ln as f64, hs / hn as f64);
        if (nl - low).abs() < 1e-9 && (nh - high).abs() < 1e-9 {
            break;
        }
        low = nl;
        high = nh;
    }
    Some((low, high))
}

fn smooth(raw: &[bool], window: usize, needed: usize) -> Vec<bool> {
    let half = (window / 2) as i64;
    (0..raw.len())
        .map(|i| {
            let lo = (i as i64 - half).max(0) as usize;
            let hi = ((i as i64 + half) as usize).min(raw.len() - 1);
            raw[lo..=hi].iter().filter(|v| **v).count() >= needed
        })
        .collect()
}

fn runs_of(flags: &[bool], base: u32) -> Vec<TeletextRun> {
    let mut out: Vec<TeletextRun> = Vec::new();
    let mut start = 0usize;
    for i in 1..=flags.len() {
        if i == flags.len() || flags[i] != flags[start] {
            out.push(TeletextRun {
                start_seconds: f64::from(base) + start as f64,
                end_seconds: f64::from(base) + i as f64,
                level: if flags[start] {
                    Level::High
                } else {
                    Level::Low
                },
            });
            start = i;
        }
    }
    out
}

fn absorb_short_runs(runs: &mut Vec<TeletextRun>, min_secs: f64) {
    loop {
        if runs.len() < 2 {
            return;
        }
        let Some(idx) = runs
            .iter()
            .enumerate()
            .filter(|(_, r)| r.end_seconds - r.start_seconds < min_secs)
            .min_by(|a, b| {
                let da = a.1.end_seconds - a.1.start_seconds;
                let db = b.1.end_seconds - b.1.start_seconds;
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
        else {
            return;
        };
        let flipped = match runs[idx].level {
            Level::High => Level::Low,
            Level::Low => Level::High,
        };
        runs[idx].level = flipped;
        merge_adjacent(runs);
    }
}

fn merge_adjacent(runs: &mut Vec<TeletextRun>) {
    let mut i = 0usize;
    while i + 1 < runs.len() {
        if runs[i].level == runs[i + 1].level {
            runs[i].end_seconds = runs[i + 1].end_seconds;
            runs.remove(i + 1);
        } else {
            i += 1;
        }
    }
}

// =============================================================================
// Sidecar — the meter runs at capture time, the detector reads it much later
// =============================================================================

pub async fn write_sidecar(path: &Path, counts: &[UnitCount]) -> std::io::Result<()> {
    let mut out = String::with_capacity(counts.len() * 8);
    for c in counts {
        out.push_str(&c.second.to_string());
        out.push(',');
        out.push_str(&c.units.to_string());
        out.push('\n');
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, out).await
}

pub async fn read_sidecar(path: &Path) -> Option<Vec<UnitCount>> {
    let raw = tokio::fs::read_to_string(path).await.ok()?;
    let mut out = Vec::new();
    for line in raw.lines() {
        let (s, u) = line.split_once(',')?;
        out.push(UnitCount {
            second: s.trim().parse().ok()?,
            units: u.trim().parse().ok()?,
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts_packet(pid: u16, pusi: bool, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![0xFFu8; TS_PACKET];
        p[0] = TS_SYNC;
        p[1] = ((pid >> 8) as u8 & 0x1F) | if pusi { 0x40 } else { 0 };
        p[2] = (pid & 0xFF) as u8;
        p[3] = 0x10;
        let n = payload.len().min(TS_PACKET - 4);
        p[4..4 + n].copy_from_slice(&payload[..n]);
        p
    }

    fn section(table_id: u8, body: &[u8]) -> Vec<u8> {
        let len = body.len() + 4;
        let mut s = vec![0u8; 0];
        s.push(0x00); // pointer_field
        s.push(table_id);
        s.push(0xB0 | ((len >> 8) as u8 & 0x0F));
        s.push((len & 0xFF) as u8);
        s.extend_from_slice(body);
        s.extend_from_slice(&[0, 0, 0, 0]);
        s
    }

    fn pat(program: u16, pmt_pid: u16) -> Vec<u8> {
        let mut body = vec![0x00, 0x01, 0xC1, 0x00, 0x00];
        body.extend_from_slice(&program.to_be_bytes());
        body.extend_from_slice(&[0xE0 | (pmt_pid >> 8) as u8, (pmt_pid & 0xFF) as u8]);
        section(0x00, &body)
    }

    fn pmt(ttx_pid: u16) -> Vec<u8> {
        let mut body = vec![0x00, 0x01, 0xC1, 0x00, 0x00, 0xE1, 0x00, 0xF0, 0x00];
        body.push(0x06);
        body.extend_from_slice(&[0xE0 | (ttx_pid >> 8) as u8, (ttx_pid & 0xFF) as u8]);
        let desc = [0x56u8, 0x05, b'e', b'n', b'g', 0x11, 0x00];
        body.extend_from_slice(&[0xF0 | (desc.len() >> 8) as u8, desc.len() as u8]);
        body.extend_from_slice(&desc);
        section(0x02, &body)
    }

    fn ttx_pes(pts_secs: f64, units: usize) -> Vec<u8> {
        let pts = (pts_secs * PTS_HZ) as u64;
        let mut p = vec![0x00, 0x00, 0x01, 0xBD, 0x00, 0x00, 0x84, 0x80, 0x05];
        p.push((0x20 | ((pts >> 29) as u8 & 0x0E)) | 0x01);
        p.push(((pts >> 22) & 0xFF) as u8);
        p.push((((pts >> 15) & 0x7F) as u8) << 1 | 0x01);
        p.push(((pts >> 7) & 0xFF) as u8);
        p.push(((pts & 0x7F) as u8) << 1 | 0x01);
        p.push(0x10);
        for _ in 0..units {
            p.push(0x03);
            p.push(0x02);
            p.push(0x00);
            p.push(0x00);
        }
        p
    }

    #[test]
    fn parses_pat_pmt_and_counts_teletext_units() {
        let mut m = TeletextMeter::new();
        m.push(&ts_packet(0, true, &pat(1, 0x100)));
        m.push(&ts_packet(0x100, true, &pmt(0x344)));
        assert_eq!(m.teletext_pid(), Some(0x344));

        m.push(&ts_packet(0x344, true, &ttx_pes(0.0, 3)));
        m.push(&ts_packet(0x344, true, &ttx_pes(1.5, 5)));
        m.finish();

        let counts = m.counts();
        assert_eq!(
            counts[0],
            UnitCount {
                second: 0,
                units: 3
            }
        );
        assert_eq!(
            counts[1],
            UnitCount {
                second: 1,
                units: 5
            }
        );
    }

    #[test]
    fn a_stream_split_across_chunk_boundaries_parses_identically() {
        let mut whole = Vec::new();
        whole.extend_from_slice(&ts_packet(0, true, &pat(1, 0x100)));
        whole.extend_from_slice(&ts_packet(0x100, true, &pmt(0x344)));
        for i in 0..4 {
            whole.extend_from_slice(&ts_packet(0x344, true, &ttx_pes(f64::from(i), 7)));
        }

        let mut a = TeletextMeter::new();
        a.push(&whole);
        a.finish();

        let mut b = TeletextMeter::new();
        for chunk in whole.chunks(37) {
            b.push(chunk);
        }
        b.finish();

        assert_eq!(a.counts(), b.counts());
        assert_eq!(a.counts().len(), 4);
    }

    #[test]
    fn resynchronises_after_leading_garbage() {
        let mut stream = vec![0x11, 0x22, 0x33];
        stream.extend_from_slice(&ts_packet(0, true, &pat(1, 0x100)));
        stream.extend_from_slice(&ts_packet(0x100, true, &pmt(0x344)));
        let mut m = TeletextMeter::new();
        m.push(&stream);
        assert_eq!(m.teletext_pid(), Some(0x344));
    }

    fn counts_from(rates: &[u32]) -> Vec<UnitCount> {
        rates
            .iter()
            .enumerate()
            .map(|(i, u)| UnitCount {
                second: i as u32,
                units: *u,
            })
            .collect()
    }

    fn stepped(low: u32, high: u32) -> Vec<UnitCount> {
        let mut r = vec![low; 120];
        r.extend(std::iter::repeat_n(high, 40));
        r.extend(std::iter::repeat_n(low, 120));
        counts_from(&r)
    }

    #[test]
    fn finds_the_step_in_a_normal_low_to_high_feed() {
        let runs = segment(&stepped(25, 50), &TeletextConfig::default());
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[1].level, Level::High);
        let ads = ad_runs(&stepped(25, 50), &TeletextConfig::default());
        assert_eq!(ads.len(), 1);
        assert!((ads[0].0 - 120.0).abs() <= 4.0, "{ads:?}");
        assert!((ads[0].1 - 160.0).abs() <= 4.0, "{ads:?}");
    }

    /// The polarity is derived, not assumed: an inverted feed must yield the
    /// same ad span with the level flipped.
    #[test]
    fn finds_the_same_step_in_an_inverted_feed() {
        let normal = ad_runs(&stepped(25, 50), &TeletextConfig::default());
        let inverted_counts = stepped(50, 25);
        let runs = segment(&inverted_counts, &TeletextConfig::default());
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[1].level, Level::Low);
        let inverted = ad_runs(&inverted_counts, &TeletextConfig::default());
        assert_eq!(normal.len(), inverted.len());
        assert!((normal[0].0 - inverted[0].0).abs() <= 2.0);
        assert!((normal[0].1 - inverted[0].1).abs() <= 2.0);
    }

    #[test]
    fn a_flat_feed_carries_no_information() {
        assert!(segment(&counts_from(&[25; 300]), &TeletextConfig::default()).is_empty());
        assert!(ad_runs(&counts_from(&[25; 300]), &TeletextConfig::default()).is_empty());
    }

    /// Neither level is a credible minority at an even split, so the polarity
    /// cannot be decided and nothing is marked.
    #[test]
    fn a_split_too_even_to_pick_a_polarity_marks_nothing() {
        let mut r = vec![25u32; 150];
        r.extend(std::iter::repeat_n(50u32, 150));
        let runs = segment(&counts_from(&r), &TeletextConfig::default());
        assert!(runs.is_empty(), "an even split has no ad level: {runs:?}");
        assert!(ad_runs(&counts_from(&r), &TeletextConfig::default()).is_empty());
    }

    #[test]
    fn runs_shorter_than_the_minimum_are_absorbed() {
        let mut r = vec![25u32; 120];
        r.extend(std::iter::repeat_n(50u32, 5));
        r.extend(std::iter::repeat_n(25u32, 120));
        assert!(ad_runs(&counts_from(&r), &TeletextConfig::default()).is_empty());
    }

    #[tokio::test]
    async fn the_sidecar_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("teletext.csv");
        let counts = counts_from(&[25, 26, 50, 51]);
        write_sidecar(&path, &counts).await.unwrap();
        assert_eq!(read_sidecar(&path).await.unwrap(), counts);
    }

    /// Ground truth from `scratchpad/GROUNDTRUTH.md`: break 1 runs 164.2 s ->
    /// 374.2 s, break 2 ~1254 s -> 1364 s, and six frames are hand-labelled.
    #[test]
    #[ignore = "needs the scratchpad capture"]
    fn the_real_capture_reproduces_the_hand_labelled_frames() {
        use std::io::Read;

        let path = std::env::var("LIVETV_ADCAP").expect("set LIVETV_ADCAP to the capture path");
        let mut f = std::fs::File::open(&path).expect("capture opens");
        let mut meter = TeletextMeter::new();
        let mut buf = vec![0u8; 4 << 20];
        loop {
            let n = f.read(&mut buf).expect("read");
            if n == 0 {
                break;
            }
            meter.push(&buf[..n]);
        }
        meter.finish();
        println!("teletext pid: {:?}", meter.teletext_pid());
        println!("seconds metered: {}", meter.counts().len());

        let ads = ad_runs(meter.counts(), &TeletextConfig::default());
        println!("detected ad runs: {ads:?}");

        let truth: [(f64, f64); 2] = [(164.2, 374.2), (1254.0, 1364.0)];
        let is_ad = |t: f64| ads.iter().any(|(s, e)| t >= *s && t < *e);
        let truth_ad = |t: f64| truth.iter().any(|(s, e)| t >= *s && t < *e);

        // GROUNDTRUTH.md labels only the span covered by `gt.ts`. The full
        // `adcap_seven.ts` runs to 2500 s and contains at least one further
        // break the document never labelled (frame-verified: 2290 s is a Seven
        // sofa promo with no DOG, 2100 s and 2450 s are racing with one), so
        // scoring past this point would count real detections as errors.
        const LABELLED_UNTIL: u32 = 1655;
        let last = meter
            .counts()
            .last()
            .map(|c| c.second)
            .unwrap_or(0)
            .min(LABELLED_UNTIL);
        let (mut tp, mut fp, mut fnn) = (0u32, 0u32, 0u32);
        for s in 0..=last {
            let t = f64::from(s) + 0.5;
            match (is_ad(t), truth_ad(t)) {
                (true, true) => tp += 1,
                (true, false) => fp += 1,
                (false, true) => fnn += 1,
                (false, false) => {}
            }
        }
        let precision = f64::from(tp) / f64::from(tp + fp).max(1.0);
        let recall = f64::from(tp) / f64::from(tp + fnn).max(1.0);
        println!(
            "over the labelled 0-{last}s: tp={tp} fp={fp} fn={fnn}              precision={precision:.3} recall={recall:.3}"
        );
        for (s, e) in ads.iter().filter(|(s, _)| *s > f64::from(LABELLED_UNTIL)) {
            println!("unlabelled region: further break detected at {s:.0}-{e:.0}s");
        }

        let labels: [(f64, bool); 6] = [
            (60.0, false),
            (300.0, true),
            (600.0, false),
            (900.0, false),
            (1200.0, false),
            (1500.0, false),
        ];
        let mut wrong = Vec::new();
        for (t, want) in labels {
            let got = is_ad(t);
            println!("frame {t:>6.0}s expected_ad={want} detected_ad={got}");
            if got != want {
                wrong.push(t);
            }
        }
        assert!(
            wrong.is_empty(),
            "hand-labelled frames misclassified: {wrong:?}"
        );
        assert!(
            precision >= 0.9,
            "precision {precision:.3} is too low to mark on"
        );
    }
}
