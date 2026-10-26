//! The stream since its latest keyframe, so a viewer joining a running tune starts on a
//! picture instead of waiting out the rest of a GOP for one.

use bytes::Bytes;

use super::ts::{self, PacketAligner, TS_PACKET};

const PAT_PID: u16 = 0;
const TABLE_PAT: u8 = 0x00;
const TABLE_PMT: u8 = 0x02;
const VIDEO_STREAM_TYPES: [u8; 7] = [0x01, 0x02, 0x10, 0x1b, 0x24, 0x42, 0xea];

/// Several times the longest GOP a broadcaster sends at HD bitrates. Past it the stream
/// has stopped marking keyframes, and holding more is memory spent on nothing.
const MAX_GOP_BYTES: usize = 8 * 1024 * 1024;

pub struct KeyframeCache {
    packets: PacketAligner,
    pmt_pid: Option<u16>,
    video_pid: Option<u16>,
    pat: Option<Vec<u8>>,
    pmt: Option<Vec<u8>>,
    gop: Option<Vec<u8>>,
    cap: usize,
}

impl Default for KeyframeCache {
    fn default() -> Self {
        Self::with_cap(MAX_GOP_BYTES)
    }
}

impl KeyframeCache {
    fn with_cap(cap: usize) -> Self {
        Self {
            packets: PacketAligner::default(),
            pmt_pid: None,
            video_pid: None,
            pat: None,
            pmt: None,
            gop: None,
            cap,
        }
    }

    /// Takes the next slice of the stream and hands back what is ready to forward:
    /// whole packets only, so a stream started from [snapshot] continues seamlessly.
    pub fn push(&mut self, chunk: &[u8]) -> Bytes {
        let packets = self.packets.push(chunk);
        for p in packets.as_chunks::<TS_PACKET>().0 {
            self.packet(p);
        }
        Bytes::from(packets)
    }

    /// The tables a demuxer needs, then everything since the latest keyframe; empty
    /// until the stream has shown one.
    pub fn snapshot(&self) -> Bytes {
        let (Some(pat), Some(pmt), Some(gop)) = (&self.pat, &self.pmt, &self.gop) else {
            return Bytes::new();
        };
        Bytes::from([pat.as_slice(), pmt, gop].concat())
    }

    fn packet(&mut self, p: &[u8; TS_PACKET]) {
        let pid = ts::pid(p);
        if pid == PAT_PID
            && ts::unit_start(p)
            && let Some(pmt_pid) = ts::section(p).and_then(program_map_pid)
        {
            self.pmt_pid = Some(pmt_pid);
            self.pat = Some(p.to_vec());
        }
        if Some(pid) == self.pmt_pid
            && ts::unit_start(p)
            && let Some(video) = ts::section(p).and_then(video_pid)
        {
            self.video_pid = Some(video);
            self.pmt = Some(p.to_vec());
        }
        if Some(pid) == self.video_pid && ts::random_access(p) {
            self.gop = Some(Vec::with_capacity(self.cap.min(2 * 1024 * 1024)));
        }
        if let Some(gop) = self.gop.as_mut() {
            gop.extend_from_slice(p);
            if gop.len() > self.cap {
                self.gop = None;
            }
        }
    }
}

fn program_map_pid(section: &[u8]) -> Option<u16> {
    if *section.first()? != TABLE_PAT {
        return None;
    }
    let entries = section.get(8..section.len().checked_sub(4)?)?;
    entries
        .as_chunks::<4>()
        .0
        .iter()
        .find(|e| u16::from_be_bytes([e[0], e[1]]) != 0)
        .map(|e| u16::from_be_bytes([e[2] & 0x1f, e[3]]))
}

fn video_pid(section: &[u8]) -> Option<u16> {
    if *section.first()? != TABLE_PMT {
        return None;
    }
    let end = section.len().checked_sub(4)?;
    let info = usize::from(u16::from_be_bytes([
        *section.get(10)? & 0x0f,
        *section.get(11)?,
    ]));
    let mut i = 12 + info;
    while i + 5 <= end {
        let stream_type = section[i];
        let pid = u16::from_be_bytes([section[i + 1] & 0x1f, section[i + 2]]);
        if VIDEO_STREAM_TYPES.contains(&stream_type) {
            return Some(pid);
        }
        i += 5 + usize::from(u16::from_be_bytes([section[i + 3] & 0x0f, section[i + 4]]));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIP: &[u8] = include_bytes!("../../tests/fixtures/livetv/clip.ts");
    const VIDEO: u16 = 256;

    fn packets(stream: &[u8]) -> std::slice::Chunks<'_, u8> {
        stream.chunks(TS_PACKET)
    }

    fn last_keyframe(stream: &[u8]) -> usize {
        packets(stream)
            .rposition(|p| ts::pid(p) == VIDEO && ts::random_access(p))
            .unwrap()
            * TS_PACKET
    }

    fn fed(stream: &[u8], chunk: usize) -> (KeyframeCache, Vec<u8>) {
        let mut cache = KeyframeCache::default();
        let forwarded = stream.chunks(chunk).flat_map(|c| cache.push(c)).collect();
        (cache, forwarded)
    }

    #[test]
    fn a_join_gets_the_tables_then_the_stream_from_its_latest_keyframe() {
        for chunk in [1usize, 188, 189, 8192] {
            let (cache, forwarded) = fed(CLIP, chunk);
            assert_eq!(forwarded, CLIP, "chunks of {chunk}");
            let snap = cache.snapshot();
            let tables: Vec<&[u8]> = packets(&snap).take(2).collect();
            assert_eq!((ts::pid(tables[0]), ts::pid(tables[1])), (0, 4096));
            assert_eq!(
                &snap[2 * TS_PACKET..],
                &CLIP[last_keyframe(CLIP)..],
                "chunks of {chunk}"
            );
        }
    }

    #[test]
    fn nothing_is_offered_before_the_first_keyframe() {
        let first = packets(CLIP)
            .position(|p| ts::pid(p) == VIDEO && ts::random_access(p))
            .unwrap();
        let (cache, _) = fed(&CLIP[..first * TS_PACKET], 8192);
        assert!(cache.snapshot().is_empty());
    }

    /// Audio frames are all marked random-access too, and none of them is a picture.
    #[test]
    fn an_audio_entry_point_is_not_a_keyframe() {
        let audio_only: Vec<u8> = packets(CLIP)
            .filter(|p| ts::pid(p) != VIDEO)
            .flatten()
            .copied()
            .collect();
        let (cache, _) = fed(&audio_only, 8192);
        assert!(cache.snapshot().is_empty());
    }

    #[test]
    fn a_stream_that_stops_marking_keyframes_is_not_held_without_end() {
        let mut cache = KeyframeCache::with_cap(TS_PACKET * 8);
        cache.push(CLIP);
        assert!(cache.snapshot().is_empty());
    }

    #[test]
    fn a_stream_joined_mid_packet_still_lines_up() {
        let (cache, forwarded) = fed(&CLIP[100..], 8192);
        assert_eq!(forwarded, &CLIP[TS_PACKET..]);
        assert_eq!(
            &cache.snapshot()[2 * TS_PACKET..],
            &CLIP[last_keyframe(CLIP)..]
        );
    }
}
