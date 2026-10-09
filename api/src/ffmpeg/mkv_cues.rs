//! Matroska Cues-based keyframe extraction.
//!
//! Reads keyframe timestamps from the MKV container's Cues element instead of
//! scanning every packet with ffprobe. Reads ~500KB of metadata instead of the
//! entire file (14GB+), providing orders-of-magnitude speedup on network storage.
//!
//! Algorithm follows Jellyfin's `MatroskaKeyframeExtractor`:
//! SeekHead → Info (TimecodeScale) → Tracks (video track number) → Cues (keyframes).

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use super::probe::ProbeError;

// EBML / Matroska element IDs
pub(crate) const EBML_HEADER: u32 = 0x1A45DFA3;
pub(crate) const SEGMENT: u32 = 0x18538067;
pub(crate) const SEEKHEAD: u32 = 0x114D9B74;
pub(crate) const SEEK: u32 = 0x4DBB;
pub(crate) const SEEKID: u32 = 0x53AB;
pub(crate) const SEEKPOSITION: u32 = 0x53AC;
pub(crate) const INFO: u32 = 0x1549A966;
pub(crate) const TIMECODE_SCALE: u32 = 0x2AD7B1;
pub(crate) const TRACKS: u32 = 0x1654AE6B;
pub(crate) const TRACK_ENTRY: u32 = 0xAE;
pub(crate) const TRACK_NUMBER: u32 = 0xD7;
pub(crate) const TRACK_TYPE: u32 = 0x83;
pub(crate) const CUES: u32 = 0x1C53BB6B;
pub(crate) const CUE_POINT: u32 = 0xBB;
pub(crate) const CUE_TIME: u32 = 0xB3;
pub(crate) const CUE_TRACK_POSITIONS: u32 = 0xB7;
pub(crate) const CUE_TRACK: u32 = 0xF7;

pub(crate) const TRACK_LANGUAGE: u32 = 0x22B59C;
pub(crate) const TRACK_LANGUAGE_BCP47: u32 = 0x22B59D;
pub(crate) const TRACK_FLAG_FORCED: u32 = 0x55AA;
pub(crate) const VOID: u32 = 0xEC;
pub(crate) const CRC32: u32 = 0xBF;

pub(crate) const TRACK_TYPE_VIDEO: u64 = 1;
pub(crate) const TRACK_TYPE_AUDIO: u64 = 2;
pub(crate) const TRACK_TYPE_SUBTITLE: u64 = 17;
pub(crate) const DEFAULT_TIMECODE_SCALE: u64 = 1_000_000; // 1ms per tick

/// Read keyframe timestamps from a Matroska file's Cues element.
///
/// Returns timestamps in seconds, sorted and deduped. Falls back to `Err` if
/// the file lacks Cues or has an unsupported structure — caller should fall
/// back to ffprobe packet scanning.
pub async fn read_mkv_keyframes(path: &Path) -> Result<Vec<f64>, ProbeError> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || read_mkv_keyframes_sync(&path))
        .await
        .map_err(|e| ProbeError::ParseError(format!("Join error: {e}")))?
}

/// Synchronous implementation (runs on blocking thread pool).
fn read_mkv_keyframes_sync(path: &Path) -> Result<Vec<f64>, ProbeError> {
    let mut f = std::fs::File::open(path).map_err(ProbeError::IoError)?;

    // Verify EBML header
    let (header_id, _) = read_element_id(&mut f)?;
    if header_id != EBML_HEADER {
        return Err(ProbeError::ParseError("not a valid EBML file".into()));
    }
    let header_size = read_vint(&mut f)?;
    f.seek(SeekFrom::Current(header_size as i64))
        .map_err(ProbeError::IoError)?;

    // Find Segment element
    let (seg_id, _) = read_element_id(&mut f)?;
    if seg_id != SEGMENT {
        return Err(ProbeError::ParseError("expected Segment element".into()));
    }
    let _seg_size = read_vint(&mut f)?;
    let segment_data_offset = f.stream_position().map_err(ProbeError::IoError)?;

    // Parse SeekHead to find Info, Tracks, and Cues positions
    let (sh_id, _) = read_element_id(&mut f)?;
    if sh_id != SEEKHEAD {
        return Err(ProbeError::ParseError(
            "expected SeekHead as first Segment child".into(),
        ));
    }
    let sh_size = read_vint(&mut f)?;
    let sh_end = f.stream_position().map_err(ProbeError::IoError)? + sh_size;

    let mut info_pos: Option<u64> = None;
    let mut tracks_pos: Option<u64> = None;
    let mut cues_pos: Option<u64> = None;

    while f.stream_position().map_err(ProbeError::IoError)? < sh_end {
        let (eid, _) = read_element_id(&mut f)?;
        let esize = read_vint(&mut f)?;

        if eid == SEEK {
            let seek_end = f.stream_position().map_err(ProbeError::IoError)? + esize;
            let mut seek_id: Option<u32> = None;
            let mut seek_position: Option<u64> = None;

            while f.stream_position().map_err(ProbeError::IoError)? < seek_end {
                let (sid, _) = read_element_id(&mut f)?;
                let ssize = read_vint(&mut f)?;

                if sid == SEEKID {
                    seek_id = Some(read_uint_be(&mut f, ssize)? as u32);
                } else if sid == SEEKPOSITION {
                    seek_position = Some(read_uint_be(&mut f, ssize)?);
                } else {
                    f.seek(SeekFrom::Current(ssize as i64))
                        .map_err(ProbeError::IoError)?;
                }
            }

            if let Some(id) = seek_id
                && let Some(pos) = seek_position
            {
                match id {
                    INFO => info_pos = Some(pos),
                    TRACKS => tracks_pos = Some(pos),
                    CUES => cues_pos = Some(pos),
                    _ => {}
                }
            }
        } else {
            f.seek(SeekFrom::Current(esize as i64))
                .map_err(ProbeError::IoError)?;
        }

        if info_pos.is_some() && tracks_pos.is_some() && cues_pos.is_some() {
            break;
        }
    }

    let info_pos =
        info_pos.ok_or_else(|| ProbeError::ParseError("SeekHead missing Info".into()))?;
    let tracks_pos =
        tracks_pos.ok_or_else(|| ProbeError::ParseError("SeekHead missing Tracks".into()))?;
    let cues_pos =
        cues_pos.ok_or_else(|| ProbeError::ParseError("SeekHead missing Cues".into()))?;

    // Read Info → TimecodeScale
    f.seek(SeekFrom::Start(segment_data_offset + info_pos))
        .map_err(ProbeError::IoError)?;
    let (info_id, _) = read_element_id(&mut f)?;
    if info_id != INFO {
        return Err(ProbeError::ParseError(
            "expected Info element at SeekHead position".into(),
        ));
    }
    let info_size = read_vint(&mut f)?;
    let info_end = f.stream_position().map_err(ProbeError::IoError)? + info_size;

    let mut timecode_scale = DEFAULT_TIMECODE_SCALE;
    while f.stream_position().map_err(ProbeError::IoError)? < info_end {
        let (eid, _) = read_element_id(&mut f)?;
        let esize = read_vint(&mut f)?;

        if eid == TIMECODE_SCALE {
            timecode_scale = read_uint_be(&mut f, esize)?;
            break;
        }
        f.seek(SeekFrom::Current(esize as i64))
            .map_err(ProbeError::IoError)?;
    }

    // Read Tracks → find first video track number
    f.seek(SeekFrom::Start(segment_data_offset + tracks_pos))
        .map_err(ProbeError::IoError)?;
    let (tracks_id, _) = read_element_id(&mut f)?;
    if tracks_id != TRACKS {
        return Err(ProbeError::ParseError(
            "expected Tracks element at SeekHead position".into(),
        ));
    }
    let tracks_size = read_vint(&mut f)?;
    let tracks_end = f.stream_position().map_err(ProbeError::IoError)? + tracks_size;

    let mut video_track_number: Option<u64> = None;
    while f.stream_position().map_err(ProbeError::IoError)? < tracks_end {
        let (eid, _) = read_element_id(&mut f)?;
        let esize = read_vint(&mut f)?;

        if eid == TRACK_ENTRY {
            let entry_end = f.stream_position().map_err(ProbeError::IoError)? + esize;
            let mut track_num: Option<u64> = None;
            let mut track_type: Option<u64> = None;

            while f.stream_position().map_err(ProbeError::IoError)? < entry_end {
                let (tid, _) = read_element_id(&mut f)?;
                let tsize = read_vint(&mut f)?;

                if tid == TRACK_NUMBER {
                    track_num = Some(read_uint_be(&mut f, tsize)?);
                } else if tid == TRACK_TYPE {
                    track_type = Some(read_uint_be(&mut f, tsize)?);
                } else {
                    f.seek(SeekFrom::Current(tsize as i64))
                        .map_err(ProbeError::IoError)?;
                }
            }

            if track_type == Some(TRACK_TYPE_VIDEO)
                && let Some(num) = track_num
            {
                video_track_number = Some(num);
                break;
            }
        } else {
            f.seek(SeekFrom::Current(esize as i64))
                .map_err(ProbeError::IoError)?;
        }
    }

    let video_track_number =
        video_track_number.ok_or_else(|| ProbeError::ParseError("no video track found".into()))?;

    // Read Cues → keyframe timestamps
    f.seek(SeekFrom::Start(segment_data_offset + cues_pos))
        .map_err(ProbeError::IoError)?;
    let (cues_id, _) = read_element_id(&mut f)?;
    if cues_id != CUES {
        return Err(ProbeError::ParseError(
            "expected Cues element at SeekHead position".into(),
        ));
    }
    let cues_size = read_vint(&mut f)?;
    let cues_end = f.stream_position().map_err(ProbeError::IoError)? + cues_size;

    let mut keyframes: Vec<f64> = Vec::new();

    while f.stream_position().map_err(ProbeError::IoError)? < cues_end {
        let (eid, _) = read_element_id(&mut f)?;
        let esize = read_vint(&mut f)?;

        if eid == CUE_POINT {
            let cp_end = f.stream_position().map_err(ProbeError::IoError)? + esize;
            let mut cue_time: Option<u64> = None;
            let mut cue_track: Option<u64> = None;

            while f.stream_position().map_err(ProbeError::IoError)? < cp_end {
                let (cid, _) = read_element_id(&mut f)?;
                let csize = read_vint(&mut f)?;

                if cid == CUE_TIME {
                    cue_time = Some(read_uint_be(&mut f, csize)?);
                } else if cid == CUE_TRACK_POSITIONS {
                    let ctp_end = f.stream_position().map_err(ProbeError::IoError)? + csize;
                    while f.stream_position().map_err(ProbeError::IoError)? < ctp_end {
                        let (tid, _) = read_element_id(&mut f)?;
                        let tsize = read_vint(&mut f)?;

                        if tid == CUE_TRACK {
                            cue_track = Some(read_uint_be(&mut f, tsize)?);
                        } else {
                            f.seek(SeekFrom::Current(tsize as i64))
                                .map_err(ProbeError::IoError)?;
                        }
                    }
                } else {
                    f.seek(SeekFrom::Current(csize as i64))
                        .map_err(ProbeError::IoError)?;
                }
            }

            if cue_track == Some(video_track_number)
                && let Some(ct) = cue_time
            {
                let timestamp_seconds = (ct as f64) * (timecode_scale as f64) / 1_000_000_000.0;
                keyframes.push(timestamp_seconds);
            }
        } else {
            f.seek(SeekFrom::Current(esize as i64))
                .map_err(ProbeError::IoError)?;
        }
    }

    keyframes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    keyframes.dedup();

    if keyframes.is_empty() {
        return Err(ProbeError::ParseError(
            "Cues contained no video keyframes".into(),
        ));
    }

    Ok(keyframes)
}

/// Read an EBML variable-length element ID. Returns (id_value, byte_width).
fn read_element_id(f: &mut std::fs::File) -> Result<(u32, usize), ProbeError> {
    let first = read_byte(f)?;

    let width = if first & 0x80 != 0 {
        1
    } else if first & 0x40 != 0 {
        2
    } else if first & 0x20 != 0 {
        3
    } else if first & 0x10 != 0 {
        4
    } else {
        return Err(ProbeError::ParseError(
            "invalid EBML element ID width".into(),
        ));
    };

    let mut value = first as u32;
    for _ in 1..width {
        let b = read_byte(f)?;
        value = (value << 8) | (b as u32);
    }

    Ok((value, width))
}

/// Read an EBML variable-length integer (VINT). Returns the decoded value.
fn read_vint(f: &mut std::fs::File) -> Result<u64, ProbeError> {
    let first = read_byte(f)?;

    // The leading-zero count is the width, and the marker bit is the top of the mask.
    let (width, mask) = match first.leading_zeros() {
        0 => (1u32, 0x7Fu8),
        1 => (2, 0x3F),
        2 => (3, 0x1F),
        3 => (4, 0x0F),
        4 => (5, 0x07),
        5 => (6, 0x03),
        6 => (7, 0x01),
        7 => (8, 0x00),
        _ => return Err(ProbeError::ParseError("invalid EBML VINT".into())),
    };

    let mut value = (first & mask) as u64;
    for _ in 1..width {
        let b = read_byte(f)?;
        value = (value << 8) | (b as u64);
    }

    Ok(value)
}

/// Read a big-endian unsigned integer of `len` bytes.
fn read_uint_be(f: &mut std::fs::File, len: u64) -> Result<u64, ProbeError> {
    if len > 8 {
        return Err(ProbeError::ParseError(format!(
            "uint too large: {len} bytes"
        )));
    }
    let mut value: u64 = 0;
    for _ in 0..len {
        let b = read_byte(f)?;
        value = (value << 8) | (b as u64);
    }
    Ok(value)
}

/// Read a single byte.
fn read_byte(f: &mut std::fs::File) -> Result<u8, ProbeError> {
    let mut buf = [0u8; 1];
    f.read_exact(&mut buf).map_err(ProbeError::IoError)?;
    Ok(buf[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_non_mkv_file_returns_error() {
        let result = read_mkv_keyframes(Path::new("/dev/null")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_nonexistent_file_returns_error() {
        let result = read_mkv_keyframes(Path::new("/tmp/nonexistent_file_12345.mkv")).await;
        assert!(result.is_err());
    }

    /// Cross-validate Cues-based keyframes against ffprobe packet scan.
    ///
    /// Runs on real MKV files in test-data/. Ignored by default (requires
    /// test-data on disk + ffprobe installed). Run with:
    ///   cargo test -p api mkv_cues -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn cross_validate_cues_vs_ffprobe() {
        use crate::ffmpeg::probe;

        let test_data = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("test-data");

        if !test_data.exists() {
            eprintln!("test-data/ not found, skipping cross-validation");
            return;
        }

        let mkv_files: Vec<_> = collect_mkv_files(&test_data);
        assert!(!mkv_files.is_empty(), "No MKV files found in {test_data:?}");
        eprintln!("Found {} MKV files to validate", mkv_files.len());

        let mut passed = 0;
        let mut failed = 0;

        for path in &mkv_files {
            let filename = path.file_name().unwrap().to_string_lossy();

            // Run Cues-based extraction
            let cues_result = read_mkv_keyframes(path).await;
            let cues_kf = match cues_result {
                Ok(kf) => kf,
                Err(e) => {
                    eprintln!("  SKIP {filename}: Cues unavailable ({e})");
                    continue;
                }
            };

            // Run ffprobe packet scan
            let ffprobe_result = probe::probe_keyframes(path).await;
            let ffprobe_kf = match ffprobe_result {
                Ok(kf) => kf,
                Err(e) => {
                    eprintln!("  SKIP {filename}: ffprobe failed ({e})");
                    continue;
                }
            };

            // Compare counts — Cues may omit a small percentage of keyframes
            // (the index is optional and some muxers skip trailing/leading entries).
            // Allow up to 5% fewer entries from Cues; ffprobe should never have fewer.
            let count_diff = (cues_kf.len() as i64 - ffprobe_kf.len() as i64).unsigned_abs();
            let max_len = cues_kf.len().max(ffprobe_kf.len());
            let pct_diff = if max_len > 0 {
                (count_diff as f64 / max_len as f64) * 100.0
            } else {
                0.0
            };

            if pct_diff > 5.0 {
                eprintln!(
                    "  FAIL {filename}: count mismatch — Cues={}, ffprobe={} ({pct_diff:.1}%)",
                    cues_kf.len(),
                    ffprobe_kf.len()
                );
                failed += 1;
                continue;
            }

            // Compare matching timestamps — find each Cues keyframe in the
            // ffprobe list (within 0.001s). Cues may be a subset.
            let mut ts_mismatch = false;
            let mut ffprobe_idx = 0;
            for (i, &cues_ts) in cues_kf.iter().enumerate() {
                // Advance ffprobe index to find a match
                while ffprobe_idx < ffprobe_kf.len() && ffprobe_kf[ffprobe_idx] < cues_ts - 0.001 {
                    ffprobe_idx += 1;
                }
                if ffprobe_idx < ffprobe_kf.len()
                    && (ffprobe_kf[ffprobe_idx] - cues_ts).abs() <= 0.001
                {
                    ffprobe_idx += 1; // consume the match
                } else {
                    eprintln!(
                        "  FAIL {filename}: Cues timestamp[{i}]={cues_ts:.6} has no ffprobe match",
                    );
                    ts_mismatch = true;
                    break;
                }
            }

            if ts_mismatch {
                failed += 1;
            } else {
                if count_diff > 0 {
                    eprintln!(
                        "  PASS {filename}: {} Cues keyframes verified against {} ffprobe ({pct_diff:.1}% diff)",
                        cues_kf.len(),
                        ffprobe_kf.len()
                    );
                } else {
                    eprintln!(
                        "  PASS {filename}: {} keyframes match exactly",
                        cues_kf.len()
                    );
                }
                passed += 1;
            }
        }

        eprintln!("\nResults: {passed} passed, {failed} failed");
        assert_eq!(failed, 0, "{failed} files had keyframe mismatches");
    }

    fn collect_mkv_files(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    files.extend(collect_mkv_files(&path));
                } else if let Some(ext) = path.extension()
                    && (ext.eq_ignore_ascii_case("mkv") || ext.eq_ignore_ascii_case("webm"))
                {
                    files.push(path);
                }
            }
        }
        files
    }
}
