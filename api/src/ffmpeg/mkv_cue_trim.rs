//! Trim a Matroska Cues index down to the tracks a session will actually use.
//!
//! libVLC parses the whole Cues element before it decodes a frame. mkvmerge cues
//! subtitle tracks by default and every subtitle block is an I-frame, so a release
//! carrying 35 languages indexes every line of every one of them: 61,839 cue points
//! and 6.5s of dead time before first picture on a 12GB film, against 4,823 points
//! for the video track alone.
//!
//! Cues is one contiguous element and nothing references positions inside it, so a
//! shorter element written at the same offset with the remainder padded by EBML Void
//! is a valid file of identical length. Because the length is preserved, cluster
//! offsets, the SeekHead and Content-Length are all untouched and the replacement can
//! be spliced into a byte range as it is served instead of written to disk.

use std::collections::BTreeSet;

use super::mkv_cues::{
    CRC32, CUE_POINT, CUE_TRACK, CUE_TRACK_POSITIONS, CUES, EBML_HEADER, SEEK, SEEKHEAD, SEEKID,
    SEEKPOSITION, SEGMENT, TRACK_ENTRY, TRACK_FLAG_FORCED, TRACK_LANGUAGE, TRACK_LANGUAGE_BCP47,
    TRACK_NUMBER, TRACK_TYPE, TRACK_TYPE_AUDIO, TRACK_TYPE_SUBTITLE, TRACK_TYPE_VIDEO, TRACKS,
    VOID,
};
use super::mkv_splice::Splice;

/// An unknown-size element: every value bit set. Only the Segment uses this in practice.
const UNKNOWN_SIZE: u64 = 0x00FF_FFFF_FFFF_FFFF;

pub(super) fn read_id(b: &[u8], p: usize) -> Option<(u32, usize)> {
    let first = *b.get(p)?;
    if first == 0 {
        return None;
    }
    let width = first.leading_zeros() as usize + 1;
    if width > 4 {
        return None;
    }
    let mut v = 0u32;
    for i in 0..width {
        v = (v << 8) | *b.get(p + i)? as u32;
    }
    Some((v, width))
}

pub(super) fn read_size(b: &[u8], p: usize) -> Option<(u64, usize)> {
    let first = *b.get(p)?;
    if first == 0 {
        return None;
    }
    let width = first.leading_zeros() as usize + 1;
    if width > 8 {
        return None;
    }
    // An 8-byte size field leaves no value bits in the first byte, and shifting a
    // u8 by 8 is an overflow rather than zero — which a 12GB Segment header hits.
    let mask = if width == 8 { 0 } else { 0xFFu8 >> width };
    let mut v = (first & mask) as u64;
    for i in 1..width {
        v = (v << 8) | *b.get(p + i)? as u64;
    }
    let all_ones = (1u64 << (7 * width)) - 1;
    Some((if v == all_ones { UNKNOWN_SIZE } else { v }, width))
}

pub(super) fn read_uint(b: &[u8]) -> u64 {
    b.iter().fold(0u64, |acc, &x| (acc << 8) | x as u64)
}

pub(super) fn id_width(id: u32) -> usize {
    (32 - id.leading_zeros() as usize).div_ceil(8).max(1)
}

pub(super) fn write_id(id: u32, out: &mut Vec<u8>) {
    let w = id_width(id);
    for i in (0..w).rev() {
        out.push((id >> (8 * i)) as u8);
    }
}

pub(super) fn size_width(v: u64) -> usize {
    (1..=8)
        .find(|w| v < (1u64 << (7 * w)) - 1)
        .expect("size fits in 8 bytes")
}

pub(super) fn write_size(v: u64, width: usize, out: &mut Vec<u8>) {
    let val = v | (1u64 << (7 * width));
    for i in (0..width).rev() {
        out.push((val >> (8 * i)) as u8);
    }
}

pub(super) struct Element<'a> {
    pub id: u32,
    pub body: &'a [u8],
    /// Whole element including its header, for verbatim copies.
    pub raw: &'a [u8],
}

/// Walk the direct children of a container body. Stops rather than panics on a
/// malformed child, so a truncated or unusual file degrades to "no patch".
pub(super) fn children(buf: &[u8]) -> impl Iterator<Item = Element<'_>> {
    let mut p = 0usize;
    std::iter::from_fn(move || {
        if p >= buf.len() {
            return None;
        }
        let start = p;
        let (id, idw) = read_id(buf, p)?;
        let (size, szw) = read_size(buf, p + idw)?;
        let data = p + idw + szw;
        if size == UNKNOWN_SIZE {
            return None;
        }
        let end = data.checked_add(size as usize)?;
        if end > buf.len() {
            return None;
        }
        p = end;
        Some(Element {
            id,
            body: &buf[data..end],
            raw: &buf[start..end],
        })
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Video,
    Audio,
    Subtitle,
    Other,
}

#[derive(Debug, Clone)]
pub struct TrackInfo {
    pub number: u64,
    pub kind: TrackKind,
    pub language: Option<String>,
    pub forced: bool,
}

/// Where the Cues element lives, and what the file's tracks are.
#[derive(Debug, Clone)]
pub struct CuesLayout {
    /// Absolute file offset of the Cues element header.
    pub element_offset: u64,
    pub tracks: Vec<TrackInfo>,
}

/// Locate Cues via the SeekHead and read the track table, from a buffer holding the
/// front of the file. Returns None for anything unexpected — the caller then serves
/// the file untouched.
pub fn read_layout(head: &[u8]) -> Option<CuesLayout> {
    let sh = first_seekhead(head)?;
    let mut cues_pos = None;
    let mut tracks_pos = None;
    for seek in children(sh.body).filter(|c| c.id == SEEK) {
        match seek_entry(seek.body) {
            (Some(CUES), Some(v)) => cues_pos = Some(v),
            (Some(TRACKS), Some(v)) => tracks_pos = Some(v),
            _ => {}
        }
    }

    let cues_pos = cues_pos?;
    let tracks_at = (sh.segment_data + tracks_pos?) as usize;
    let tracks = parse_tracks(head.get(tracks_at..)?)?;

    Some(CuesLayout {
        element_offset: sh.segment_data + cues_pos,
        tracks,
    })
}

pub(super) struct SeekHeadAt<'a> {
    /// Absolute offset of the Segment's data, which every SeekPosition is relative to.
    pub segment_data: u64,
    /// Absolute offset of the SeekHead's body.
    pub body_offset: u64,
    pub body: &'a [u8],
}

pub(super) fn first_seekhead(head: &[u8]) -> Option<SeekHeadAt<'_>> {
    let (id, idw) = read_id(head, 0)?;
    if id != EBML_HEADER {
        return None;
    }
    let (hsize, hszw) = read_size(head, idw)?;
    let mut p = idw + hszw + hsize as usize;

    let (sid, sidw) = read_id(head, p)?;
    if sid != SEGMENT {
        return None;
    }
    let (_ssize, sszw) = read_size(head, p + sidw)?;
    p += sidw + sszw;
    let segment_data = p as u64;

    let base = head.as_ptr() as usize;
    let el = children(head.get(p..)?).find(|el| el.id == SEEKHEAD)?;
    Some(SeekHeadAt {
        segment_data,
        body_offset: (el.body.as_ptr() as usize - base) as u64,
        body: el.body,
    })
}

/// The (SeekID, SeekPosition) of one Seek element's body.
pub(super) fn seek_entry(seek_body: &[u8]) -> (Option<u32>, Option<u64>) {
    let (mut target, mut pos) = (None, None);
    for f in children(seek_body) {
        match f.id {
            SEEKID => target = Some(read_uint(f.body) as u32),
            SEEKPOSITION => pos = Some(read_uint(f.body)),
            _ => {}
        }
    }
    (target, pos)
}

fn parse_tracks(buf: &[u8]) -> Option<Vec<TrackInfo>> {
    let (id, idw) = read_id(buf, 0)?;
    if id != TRACKS {
        return None;
    }
    let (size, szw) = read_size(buf, idw)?;
    let body = buf.get(idw + szw..idw + szw + size as usize)?;

    let mut out = Vec::new();
    for entry in children(body) {
        if entry.id != TRACK_ENTRY {
            continue;
        }
        let mut number = None;
        let mut kind = TrackKind::Other;
        let mut language = None;
        let mut bcp47 = None;
        let mut forced = false;
        for f in children(entry.body) {
            match f.id {
                TRACK_NUMBER => number = Some(read_uint(f.body)),
                TRACK_TYPE => {
                    kind = match read_uint(f.body) {
                        TRACK_TYPE_VIDEO => TrackKind::Video,
                        TRACK_TYPE_AUDIO => TrackKind::Audio,
                        TRACK_TYPE_SUBTITLE => TrackKind::Subtitle,
                        _ => TrackKind::Other,
                    }
                }
                TRACK_LANGUAGE => language = String::from_utf8(f.body.to_vec()).ok(),
                TRACK_LANGUAGE_BCP47 => bcp47 = String::from_utf8(f.body.to_vec()).ok(),
                TRACK_FLAG_FORCED => forced = read_uint(f.body) != 0,
                _ => {}
            }
        }
        if let Some(number) = number {
            out.push(TrackInfo {
                number,
                kind,
                // BCP-47 wins where present; it is the field mkvmerge writes for
                // newer files and the legacy Language element is then left at "eng".
                language: bcp47.or(language).map(|s| s.trim_matches('\0').to_string()),
                forced,
            });
        }
    }
    Some(out)
}

/// Which track numbers keep their cue points.
///
/// Video and audio always survive: video cues are what VLC actually seeks on, and
/// audio carries no cue points to begin with. Subtitles survive only when the client
/// will render them from the container — when the server hands out extracted sidecars
/// the embedded tracks are never selected, so their index is pure cost.
pub fn kept_tracks(
    tracks: &[TrackInfo],
    sidecars_available: bool,
    preferred_languages: &[String],
) -> BTreeSet<u64> {
    tracks
        .iter()
        .filter(|t| match t.kind {
            TrackKind::Subtitle => {
                !sidecars_available
                    && (t.forced
                        || preferred_languages.iter().any(|want| {
                            let want = want.to_ascii_lowercase();
                            t.language
                                .as_deref()
                                .map(|l| language_matches(&l.to_ascii_lowercase(), &want))
                                .unwrap_or(false)
                        }))
            }
            _ => true,
        })
        .map(|t| t.number)
        .collect()
}

/// Matroska writes ISO-639-2/B ("eng", "fre"), config carries names ("english").
fn language_matches(track: &str, want: &str) -> bool {
    let track = track.split(['-', '_']).next().unwrap_or(track);
    if track == want {
        return true;
    }
    let alias: &[(&str, &[&str])] = &[
        ("english", &["en", "eng"]),
        ("french", &["fr", "fre", "fra"]),
        ("german", &["de", "ger", "deu"]),
        ("spanish", &["es", "spa"]),
        ("italian", &["it", "ita"]),
        ("dutch", &["nl", "dut", "nld"]),
        ("portuguese", &["pt", "por"]),
        ("japanese", &["ja", "jpn"]),
        ("korean", &["ko", "kor"]),
        ("chinese", &["zh", "chi", "zho"]),
        ("russian", &["ru", "rus"]),
        ("polish", &["pl", "pol"]),
        ("swedish", &["sv", "swe"]),
        ("danish", &["da", "dan"]),
        ("norwegian", &["no", "nor"]),
        ("finnish", &["fi", "fin"]),
    ];
    alias
        .iter()
        .any(|(name, codes)| *name == want && codes.contains(&track))
}

/// The trimmed Cues element as a splice over the original one.
///
/// Only the head of the replacement is held: the trimmed Cues element plus the header
/// of the Void that pads it back out. A Void payload is zeros by definition and is the
/// bulk of the region, so storing it would cost ~1.7MB per cached film to remember
/// nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CuePatch {
    pub splice: Splice,
    pub original_points: usize,
    pub kept_points: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TrimOutcome {
    Trimmed(Box<CuePatch>),
    /// Nothing worth doing: already lean, or every point belongs to a kept track.
    NotWorthIt,
    /// Unparseable or no room to pad — serve the file untouched.
    Unsupported,
}

/// Minimum saving before a patch is worth building and caching.
const MIN_SAVING_BYTES: usize = 64 * 1024;

/// Rebuild `element` (a whole Cues element, header included) keeping only `keep`.
///
/// The result is byte-for-byte the same length as the input.
pub fn build_patch(element: &[u8], offset: u64, keep: &BTreeSet<u64>) -> TrimOutcome {
    let Some((id, idw)) = read_id(element, 0) else {
        return TrimOutcome::Unsupported;
    };
    if id != CUES {
        return TrimOutcome::Unsupported;
    }
    let Some((size, szw)) = read_size(element, idw) else {
        return TrimOutcome::Unsupported;
    };
    let header_len = idw + szw;
    if size == UNKNOWN_SIZE || header_len + size as usize != element.len() {
        return TrimOutcome::Unsupported;
    }
    let body = &element[header_len..];

    let mut out: Vec<u8> = Vec::with_capacity(body.len() / 4);
    let mut original_points = 0usize;
    let mut kept_points = 0usize;

    for el in children(body) {
        match el.id {
            // Dropped rather than rewritten: the CRC covers bytes we are replacing,
            // and Void is re-emitted as one block at the end anyway.
            CRC32 | VOID => continue,
            CUE_POINT => {}
            _ => {
                out.extend_from_slice(el.raw);
                continue;
            }
        }
        original_points += 1;

        let mut point: Vec<u8> = Vec::new();
        let mut kept_any = false;
        for f in children(el.body) {
            if f.id != CUE_TRACK_POSITIONS {
                point.extend_from_slice(f.raw);
                continue;
            }
            let track = children(f.body)
                .find(|c| c.id == CUE_TRACK)
                .map(|c| read_uint(c.body));
            if track.is_some_and(|t| keep.contains(&t)) {
                point.extend_from_slice(f.raw);
                kept_any = true;
            }
        }
        if !kept_any {
            continue;
        }
        kept_points += 1;
        write_id(CUE_POINT, &mut out);
        write_size(point.len() as u64, size_width(point.len() as u64), &mut out);
        out.extend_from_slice(&point);
    }

    // Keeping the original size-field width means the header length never changes,
    // so the element offset recorded in the SeekHead stays correct.
    if (out.len() as u64) >= (1u64 << (7 * szw)) - 1 {
        return TrimOutcome::Unsupported;
    }

    let mut patched = Vec::with_capacity(element.len());
    write_id(CUES, &mut patched);
    write_size(out.len() as u64, szw, &mut patched);
    patched.extend_from_slice(&out);

    let slack = element.len() - patched.len();
    if slack < MIN_SAVING_BYTES {
        return TrimOutcome::NotWorthIt;
    }
    // A Void needs at least an ID and a size field.
    let Some(void_szw) = (1..=8).find(|w| {
        slack
            .checked_sub(1 + w)
            .is_some_and(|payload| (payload as u64) < (1u64 << (7 * w)) - 1)
    }) else {
        return TrimOutcome::Unsupported;
    };
    let void_payload = slack - 1 - void_szw;
    write_id(VOID, &mut patched);
    write_size(void_payload as u64, void_szw, &mut patched);

    debug_assert!(patched.len() <= element.len());
    TrimOutcome::Trimmed(Box::new(CuePatch {
        splice: Splice::new(offset, patched, element.len()),
        original_points,
        kept_points,
    }))
}

/// Read the Cues element header to learn how many bytes the whole element occupies.
pub fn cues_element_len(header: &[u8]) -> Option<usize> {
    let (id, idw) = read_id(header, 0)?;
    if id != CUES {
        return None;
    }
    let (size, szw) = read_size(header, idw)?;
    if size == UNKNOWN_SIZE {
        return None;
    }
    Some(idw + szw + size as usize)
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod fixture {
    use super::*;
    use crate::ffmpeg::mkv_cues::{CUE_TIME, INFO};

    pub const CUE_CLUSTER_POSITION: u32 = 0xF1;
    pub const TAGS: u32 = 0x1254C367;

    pub fn el(id: u32, body: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        write_id(id, &mut v);
        write_size(body.len() as u64, size_width(body.len() as u64), &mut v);
        v.extend_from_slice(body);
        v
    }

    pub fn uint_el(id: u32, val: u64) -> Vec<u8> {
        let mut bytes = val.to_be_bytes().to_vec();
        while bytes.len() > 1 && bytes[0] == 0 {
            bytes.remove(0);
        }
        el(id, &bytes)
    }

    pub fn str_el(id: u32, val: &str) -> Vec<u8> {
        el(id, val.as_bytes())
    }

    pub fn track(number: u64, kind: u64, lang: &str, forced: bool) -> Vec<u8> {
        let mut b = uint_el(TRACK_NUMBER, number);
        b.extend(uint_el(TRACK_TYPE, kind));
        b.extend(str_el(TRACK_LANGUAGE, lang));
        b.extend(uint_el(TRACK_FLAG_FORCED, forced as u64));
        el(TRACK_ENTRY, &b)
    }

    /// Video track 1, audio track 2, then subtitle tracks 3.. in the given languages.
    pub fn tracks_element(sub_langs: &[&str], forced_idx: Option<usize>) -> Vec<u8> {
        let mut b = track(1, TRACK_TYPE_VIDEO, "und", false);
        b.extend(track(2, TRACK_TYPE_AUDIO, "eng", false));
        for (i, lang) in sub_langs.iter().enumerate() {
            b.extend(track(
                3 + i as u64,
                TRACK_TYPE_SUBTITLE,
                lang,
                forced_idx == Some(i),
            ));
        }
        el(TRACKS, &b)
    }

    pub fn cue_point(time: u64, track: u64, cluster: u64) -> Vec<u8> {
        let mut ctp = uint_el(CUE_TRACK, track);
        ctp.extend(uint_el(CUE_CLUSTER_POSITION, cluster));
        let mut body = uint_el(CUE_TIME, time);
        body.extend(el(CUE_TRACK_POSITIONS, &ctp));
        el(CUE_POINT, &body)
    }

    /// One cue point per track per second, which is the shape mkvmerge produces.
    pub fn cues_element(track_numbers: &[u64], points_per_track: u64) -> Vec<u8> {
        let mut body = Vec::new();
        for t in track_numbers {
            for i in 0..points_per_track {
                body.extend(cue_point(i * 1000, *t, 0x4000 + i * 977));
            }
        }
        el(CUES, &body)
    }

    pub struct Built {
        pub file: Vec<u8>,
        pub cues_offset: u64,
        pub cues_len: usize,
    }

    /// A minimal but structurally real Matroska file: EBML header, Segment with a
    /// SeekHead, Info, Tracks, filler standing in for clusters, Cues, then Tags.
    pub fn build_file(
        sub_langs: &[&str],
        points_per_track: u64,
        forced_idx: Option<usize>,
    ) -> Built {
        let tracks = tracks_element(sub_langs, forced_idx);
        let info = el(INFO, &uint_el(0x2AD7B1, 1_000_000));
        let mut track_numbers = vec![1u64];
        track_numbers.extend((0..sub_langs.len()).map(|i| 3 + i as u64));
        let cues = cues_element(&track_numbers, points_per_track);

        // SeekHead is fixed-width so its size does not shift the offsets it records.
        let seek_entry = |target: u32, pos: u64| {
            let mut b = el(SEEKID, &target.to_be_bytes()[(4 - id_width(target))..]);
            b.extend(el(SEEKPOSITION, &pos.to_be_bytes()));
            el(SEEK, &b)
        };
        let placeholder = {
            let mut b = seek_entry(TRACKS, 0);
            b.extend(seek_entry(CUES, 0));
            el(SEEKHEAD, &b)
        };

        let header = el(EBML_HEADER, &uint_el(0x4286, 1));
        let seekhead_len = placeholder.len();
        let info_pos = seekhead_len as u64;
        let tracks_pos = info_pos + info.len() as u64;
        let filler_pos = tracks_pos + tracks.len() as u64;
        let filler = el(VOID, &vec![0u8; 4096]);
        let cues_pos = filler_pos + filler.len() as u64;

        let seekhead = {
            let mut b = seek_entry(TRACKS, tracks_pos);
            b.extend(seek_entry(CUES, cues_pos));
            el(SEEKHEAD, &b)
        };
        assert_eq!(seekhead.len(), seekhead_len, "seekhead width shifted");

        let mut segment_body = seekhead;
        segment_body.extend(info);
        segment_body.extend(tracks);
        segment_body.extend(filler);
        segment_body.extend(cues.clone());
        // Real files put Tags after Cues; without something trailing the patch would
        // end at EOF and never exercise a splice that runs off its far edge.
        segment_body.extend(el(TAGS, &str_el(0x45A3, "fixture")));

        let mut file = header;
        write_id(SEGMENT, &mut file);
        write_size(
            segment_body.len() as u64,
            size_width(segment_body.len() as u64),
            &mut file,
        );
        let segment_data = file.len() as u64;
        file.extend(segment_body);

        Built {
            file,
            cues_offset: segment_data + cues_pos,
            cues_len: cues.len(),
        }
    }

    pub fn many_langs() -> Vec<&'static str> {
        vec![
            "eng", "eng", "ara", "cze", "dan", "ger", "gre", "spa", "fin", "fre", "heb", "hin",
            "hun", "ind", "ita", "jpn", "kor", "may", "dut", "nor", "pol", "por", "swe", "tha",
            "tur", "vie", "chi",
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;
    use crate::ffmpeg::mkv_cues::CUE_TIME;

    fn parse_points(element: &[u8]) -> Vec<(u64, u64)> {
        let (_, idw) = read_id(element, 0).unwrap();
        let (_, szw) = read_size(element, idw).unwrap();
        let mut out = Vec::new();
        for p in children(&element[idw + szw..]) {
            if p.id != CUE_POINT {
                continue;
            }
            let time = children(p.body)
                .find(|c| c.id == CUE_TIME)
                .map(|c| read_uint(c.body))
                .unwrap();
            for ctp in children(p.body).filter(|c| c.id == CUE_TRACK_POSITIONS) {
                let t = children(ctp.body)
                    .find(|c| c.id == CUE_TRACK)
                    .map(|c| read_uint(c.body))
                    .unwrap();
                out.push((time, t));
            }
        }
        out
    }

    #[test]
    fn size_fields_of_every_width_round_trip() {
        for width in 1..=8usize {
            for v in [0u64, 1, 127, 128, 16_000, 12_075_155_206] {
                if v >= (1u64 << (7 * width)) - 1 {
                    continue;
                }
                let mut buf = Vec::new();
                write_size(v, width, &mut buf);
                assert_eq!(buf.len(), width);
                assert_eq!(
                    read_size(&buf, 0),
                    Some((v, width)),
                    "width {width} value {v}"
                );
            }
        }
    }

    #[test]
    fn an_unknown_size_is_recognised_at_every_width() {
        for width in 1..=8usize {
            let mut buf = Vec::new();
            write_size((1u64 << (7 * width)) - 1, width, &mut buf);
            assert_eq!(read_size(&buf, 0), Some((UNKNOWN_SIZE, width)));
        }
    }

    #[test]
    fn layout_finds_cues_and_tracks() {
        let built = build_file(&["eng", "fre"], 4, None);
        let layout = read_layout(&built.file).expect("layout");
        assert_eq!(layout.element_offset, built.cues_offset);
        assert_eq!(layout.tracks.len(), 4);
        assert_eq!(layout.tracks[0].kind, TrackKind::Video);
        assert_eq!(layout.tracks[1].kind, TrackKind::Audio);
        assert_eq!(layout.tracks[3].language.as_deref(), Some("fre"));
    }

    #[test]
    fn the_cues_element_sits_where_the_layout_says() {
        let built = build_file(&["eng"], 4, None);
        let at = built.cues_offset as usize;
        assert_eq!(
            cues_element_len(&built.file[at..]),
            Some(built.cues_len),
            "cue element length disagrees with the built file"
        );
    }

    #[test]
    fn sidecars_mean_no_subtitle_track_keeps_its_index() {
        let built = build_file(&many_langs(), 2, None);
        let layout = read_layout(&built.file).unwrap();
        let keep = kept_tracks(&layout.tracks, true, &["english".into()]);
        assert_eq!(keep, BTreeSet::from([1, 2]));
    }

    #[test]
    fn without_sidecars_the_preferred_language_keeps_its_index() {
        let built = build_file(&many_langs(), 2, None);
        let layout = read_layout(&built.file).unwrap();
        let keep = kept_tracks(&layout.tracks, false, &["english".into()]);
        // Video, audio, and the two English subtitle tracks (3 and 4).
        assert_eq!(keep, BTreeSet::from([1, 2, 3, 4]));
    }

    #[test]
    fn a_forced_track_keeps_its_index_whatever_the_language() {
        let built = build_file(&["ara", "kor"], 2, Some(1));
        let layout = read_layout(&built.file).unwrap();
        let keep = kept_tracks(&layout.tracks, false, &["english".into()]);
        assert!(
            keep.contains(&4),
            "forced Korean track was dropped: {keep:?}"
        );
    }

    /// The full replacement bytes, Void payload included.
    fn region(patch: &CuePatch) -> Vec<u8> {
        patch
            .splice
            .whole_range(patch.splice.offset, patch.splice.region_len())
            .expect("its own region")
    }

    fn trim(built: &Built, keep: &BTreeSet<u64>) -> CuePatch {
        let at = built.cues_offset as usize;
        match build_patch(
            &built.file[at..at + built.cues_len],
            built.cues_offset,
            keep,
        ) {
            TrimOutcome::Trimmed(p) => *p,
            other => panic!("expected a patch, got {other:?}"),
        }
    }

    #[test]
    fn the_patch_is_exactly_as_long_as_what_it_replaces() {
        let built = build_file(&many_langs(), 400, None);
        let patch = trim(&built, &BTreeSet::from([1, 2]));
        assert_eq!(patch.splice.region_len(), built.cues_len);
    }

    #[test]
    fn the_void_payload_is_not_held_in_memory() {
        let built = build_file(&many_langs(), 400, None);
        let patch = trim(&built, &BTreeSet::from([1, 2]));
        assert!(
            patch.splice.stored_bytes() * 4 < patch.splice.region_len(),
            "holding {} of a {} byte region",
            patch.splice.stored_bytes(),
            patch.splice.region_len()
        );
        // What it does not store still has to read back as zeros.
        let full = region(&patch);
        assert_eq!(full.len(), built.cues_len);
        assert!(full[patch.splice.stored_bytes()..].iter().all(|&b| b == 0));
    }

    #[test]
    fn the_patch_keeps_only_the_chosen_tracks_and_all_of_them() {
        let built = build_file(&many_langs(), 400, None);
        let keep = BTreeSet::from([1u64, 3, 4]);
        let patch = trim(&built, &keep);

        let before = parse_points(&built.file[built.cues_offset as usize..][..built.cues_len]);
        let after = parse_points(&region(&patch));
        let expected: Vec<_> = before
            .iter()
            .copied()
            .filter(|(_, t)| keep.contains(t))
            .collect();
        assert_eq!(after, expected);
        assert_eq!(patch.original_points, before.len());
        assert_eq!(patch.kept_points, expected.len());
    }

    #[test]
    fn the_patched_file_still_parses_end_to_end() {
        let built = build_file(&many_langs(), 400, None);
        let patch = trim(&built, &BTreeSet::from([1, 2]));
        let mut file = built.file.clone();
        patch.splice.apply(0, &mut file);

        let layout = read_layout(&file).expect("patched file lost its layout");
        assert_eq!(layout.element_offset, built.cues_offset);
        assert_eq!(layout.tracks.len(), 2 + many_langs().len());

        // The Void must take the trimmed element back up to exactly the length of
        // the original, or everything after Cues shifts.
        let at = layout.element_offset as usize;
        let trimmed_len = cues_element_len(&file[at..]).unwrap();
        assert!(trimmed_len < built.cues_len);
        let void = children(&file[at + trimmed_len..])
            .next()
            .expect("no Void after trimmed Cues");
        assert_eq!(void.id, VOID);
        assert_eq!(
            trimmed_len + void.raw.len(),
            built.cues_len,
            "Void does not pad out the original element"
        );

        // And whatever followed Cues must still parse where it always was.
        let tags = children(&file[at + built.cues_len..])
            .next()
            .expect("element after Cues was lost");
        assert_eq!(tags.id, TAGS);
    }

    #[test]
    fn a_lean_index_is_left_alone() {
        let built = build_file(&["eng"], 4, None);
        assert_eq!(
            build_patch(
                &built.file[built.cues_offset as usize..][..built.cues_len],
                built.cues_offset,
                &BTreeSet::from([1]),
            ),
            TrimOutcome::NotWorthIt
        );
    }

    #[test]
    fn something_that_is_not_a_cues_element_is_refused() {
        assert_eq!(
            build_patch(&el(TRACKS, &[0u8; 4096]), 0, &BTreeSet::from([1])),
            TrimOutcome::Unsupported
        );
        assert_eq!(
            build_patch(&[], 0, &BTreeSet::from([1])),
            TrimOutcome::Unsupported
        );
    }

    fn expected_patched(built: &Built, patch: &CuePatch) -> Vec<u8> {
        let mut f = built.file.clone();
        patch.splice.apply(0, &mut f);
        f
    }

    #[test]
    fn a_range_inside_the_patch_is_served_without_reading_the_file() {
        let built = build_file(&many_langs(), 400, None);
        let patch = trim(&built, &BTreeSet::from([1, 2]));
        let expected = expected_patched(&built, &patch);

        for (start, len) in [
            (patch.splice.offset, patch.splice.region_len()),
            (patch.splice.offset, 1),
            (patch.splice.end() - 1, 1),
            (patch.splice.offset + 1000, 5000),
        ] {
            let got = patch
                .splice
                .whole_range(start, len)
                .expect("should be contained");
            assert_eq!(got, expected[start as usize..start as usize + len]);
        }

        assert!(
            patch
                .splice
                .whole_range(patch.splice.offset - 1, 4)
                .is_none()
        );
        assert!(
            patch
                .splice
                .whole_range(patch.splice.end() - 1, 2)
                .is_none()
        );
        assert!(patch.splice.whole_range(0, built.file.len()).is_none());
    }

    #[test]
    fn splicing_any_range_in_any_chunking_matches_the_patched_file() {
        let built = build_file(&many_langs(), 400, None);
        let patch = trim(&built, &BTreeSet::from([1, 2]));
        let expected = expected_patched(&built, &patch);
        let total = built.file.len() as u64;
        let p0 = patch.splice.offset;
        let p1 = patch.splice.end();

        // Edges of the patch region are where offset arithmetic goes wrong.
        let mut cases: Vec<(u64, u64)> = vec![
            (0, total),
            (p0, p1),
            (p0, p0 + 1),
            (p1 - 1, p1),
            (p0 - 10, p0 + 10),
            (p1 - 10, p1 + 10),
            (p0 - 10, p1 + 10),
            (p0 + 100, p1 - 100),
            (0, p0),
            (p1, total),
        ];
        for c in cases.iter_mut() {
            c.1 = c.1.min(total);
        }
        let mut seed = 0x2545F4914F6CDD1Du64;
        for _ in 0..300 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let a = seed % total;
            let b = a + 1 + (seed >> 32) % (total - a);
            cases.push((a, b.min(total)));
        }

        for (start, end) in cases {
            for chunk in [1usize, 7, 4096, 65536] {
                let mut got = Vec::new();
                let mut pos = start;
                while pos < end {
                    let n = chunk.min((end - pos) as usize);
                    let mut buf = built.file[pos as usize..pos as usize + n].to_vec();
                    if patch.splice.overlaps(pos, n) {
                        patch.splice.apply(pos, &mut buf);
                    }
                    got.extend_from_slice(&buf);
                    pos += n as u64;
                }
                assert_eq!(
                    got,
                    &expected[start as usize..end as usize],
                    "range {start}..{end} chunked by {chunk}"
                );
            }
        }
    }

    #[test]
    fn a_chunk_that_misses_the_patch_is_untouched() {
        let built = build_file(&many_langs(), 400, None);
        let patch = trim(&built, &BTreeSet::from([1, 2]));
        assert!(!patch.splice.overlaps(0, patch.splice.offset as usize));
        assert!(!patch.splice.overlaps(patch.splice.end(), 4096));
        assert!(patch.splice.overlaps(patch.splice.end() - 1, 4096));
        assert!(patch.splice.overlaps(0, built.file.len()));

        let mut buf = built.file[..patch.splice.offset as usize].to_vec();
        let before = buf.clone();
        patch.splice.apply(0, &mut buf);
        assert_eq!(buf, before);
    }
}
