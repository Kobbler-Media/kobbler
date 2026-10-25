//! Put Info and Tracks ahead of Cues in a Matroska SeekHead.
//!
//! libVLC's mkv demuxer loads the SeekHead's targets in the order they are listed, and
//! a cue point whose track it has not parsed yet is disabled for good. A SeekHead that
//! lists Cues before Tracks therefore leaves the player without a usable index, and
//! on a stream that is not fast-seekable (anything over HTTP) every seek and resume
//! decodes forward from the first cluster: 50s to resume 7 minutes into an episode.
//!
//! The Seek entries are the same bytes in a different order, so the SeekHead keeps
//! its length and nothing else in the file moves.

use super::mkv_cue_trim::{children, first_seekhead, seek_entry, write_id};
use super::mkv_cues::{CRC32, CUES, INFO, SEEK, TRACKS};
use super::mkv_splice::Splice;

const CRC32_ELEMENT_LEN: usize = 6;

fn rank(target: Option<u32>) -> u8 {
    match target {
        Some(INFO) => 0,
        Some(TRACKS) => 1,
        _ => 2,
    }
}

/// The splice that reorders the first SeekHead, or None when it already lists Info and
/// Tracks before Cues (or cannot be read).
pub fn reorder_seekhead(head: &[u8]) -> Option<Splice> {
    let sh = first_seekhead(head)?;
    let kids: Vec<_> = children(sh.body).collect();
    if kids.iter().map(|k| k.raw.len()).sum::<usize>() != sh.body.len() {
        return None;
    }

    let seeks: Vec<_> = kids.iter().filter(|k| k.id == SEEK).collect();
    let targets: Vec<Option<u32>> = seeks.iter().map(|k| seek_entry(k.body).0).collect();
    let first_cues = targets.iter().position(|t| *t == Some(CUES))?;
    if !targets[first_cues..].iter().any(|t| rank(*t) < 2) {
        return None;
    }

    let mut order: Vec<usize> = (0..seeks.len()).collect();
    order.sort_by_key(|&i| rank(targets[i]));

    let crc = kids.iter().find(|k| k.id == CRC32);
    if crc.is_some_and(|c| c.raw.len() != CRC32_ELEMENT_LEN) {
        return None;
    }
    let mut rest: Vec<u8> = Vec::with_capacity(sh.body.len());
    for i in order {
        rest.extend_from_slice(seeks[i].raw);
    }
    for k in kids.iter().filter(|k| k.id != SEEK && k.id != CRC32) {
        rest.extend_from_slice(k.raw);
    }

    // A CRC-32 covers every byte after it in its parent, so it moves to the front
    // and is recomputed rather than left vouching for the old order.
    let mut body = Vec::with_capacity(sh.body.len());
    if crc.is_some() {
        write_id(CRC32, &mut body);
        body.push(0x84);
        body.extend_from_slice(&crc32fast::hash(&rest).to_le_bytes());
    }
    body.extend_from_slice(&rest);
    debug_assert_eq!(body.len(), sh.body.len());

    Some(Splice::new(sh.body_offset, body, sh.body.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTED_CUES_FIRST: &[u8] =
        include_bytes!("../../tests/fixtures/mkv/cues-before-tracks.mkv");
    const TAGS: u32 = 0x1254_C367;

    fn order(file: &[u8]) -> Vec<u32> {
        let sh = first_seekhead(file).unwrap();
        children(sh.body)
            .filter(|k| k.id == SEEK)
            .map(|k| seek_entry(k.body).0.unwrap())
            .collect()
    }

    fn patched(file: &[u8]) -> Option<Vec<u8>> {
        let splice = reorder_seekhead(file)?;
        let mut out = file.to_vec();
        splice.apply(0, &mut out);
        Some(out)
    }

    #[test]
    fn info_and_tracks_move_ahead_of_cues_and_the_rest_keep_their_order() {
        assert_eq!(order(LISTED_CUES_FIRST), [INFO, CUES, TAGS, TRACKS]);
        let out = patched(LISTED_CUES_FIRST).expect("no reorder");
        assert_eq!(out.len(), LISTED_CUES_FIRST.len());
        assert_eq!(order(&out), [INFO, TRACKS, CUES, TAGS]);
    }

    #[test]
    fn the_crc_is_recomputed_over_the_new_order() {
        let out = patched(LISTED_CUES_FIRST).unwrap();
        let sh = first_seekhead(&out).unwrap();
        let crc = children(sh.body).next().unwrap();
        assert_eq!(crc.id, CRC32);
        let after = &sh.body[crc.raw.len()..];
        assert_eq!(crc.body, crc32fast::hash(after).to_le_bytes());
    }

    #[test]
    fn a_seekhead_already_in_a_usable_order_is_left_alone() {
        let out = patched(LISTED_CUES_FIRST).unwrap();
        assert_eq!(reorder_seekhead(&out), None);
        let built = crate::ffmpeg::mkv_cue_trim::fixture::build_file(&["eng"], 10, None);
        assert_eq!(reorder_seekhead(&built.file), None);
    }

    #[test]
    fn something_that_is_not_matroska_is_left_alone() {
        assert_eq!(reorder_seekhead(&[0u8; 4096]), None);
        assert_eq!(reorder_seekhead(&LISTED_CUES_FIRST[..40]), None);
    }
}
