//! MPEG-TS packet plumbing shared by everything that reads the tuner's stream.

pub const TS_PACKET: usize = 188;
const SYNC: u8 = 0x47;

/// Cuts an arbitrary byte stream into whole transport packets. Chunk boundaries never
/// line up with packets, and the device's first bytes need not start on one either.
#[derive(Default)]
pub struct PacketAligner {
    carry: Vec<u8>,
    synced: bool,
}

impl PacketAligner {
    /// Every whole packet the stream now holds, concatenated; the rest waits for more.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.carry.extend_from_slice(chunk);
        let mut out = Vec::with_capacity(self.carry.len());
        loop {
            if !self.synced && !self.sync() {
                return out;
            }
            let whole = self.carry.len() / TS_PACKET * TS_PACKET;
            let good = (0..whole)
                .step_by(TS_PACKET)
                .find(|&i| self.carry[i] != SYNC)
                .unwrap_or(whole);
            out.extend(self.carry.drain(..good));
            if good == whole {
                return out;
            }
            self.synced = false;
        }
    }

    // A lone 0x47 is a common payload value, so the offset is confirmed by a run of them.
    fn sync(&mut self) -> bool {
        let runs = (self.carry.len() / TS_PACKET).clamp(1, 4);
        let span = TS_PACKET * (runs - 1) + 1;
        let found = (0..TS_PACKET.min(self.carry.len()))
            .filter(|&i| i + span <= self.carry.len())
            .find(|&i| (0..runs).all(|k| self.carry[i + k * TS_PACKET] == SYNC));
        match found {
            Some(offset) => {
                self.carry.drain(..offset);
                self.synced = true;
                true
            }
            None => {
                // Not a transport stream at all, or still too short to tell.
                if self.carry.len() > TS_PACKET * 64 {
                    self.carry.drain(..TS_PACKET * 32);
                }
                false
            }
        }
    }
}

pub fn pid(p: &[u8]) -> u16 {
    (u16::from(p[1] & 0x1f) << 8) | u16::from(p[2])
}

pub fn unit_start(p: &[u8]) -> bool {
    p[1] & 0x40 != 0
}

pub fn random_access(p: &[u8]) -> bool {
    let adaptation = (p[3] >> 4) & 0x3;
    adaptation & 0x2 != 0 && p[4] > 0 && p[5] & 0x40 != 0
}

pub fn payload(p: &[u8]) -> Option<&[u8]> {
    let adaptation = (p[3] >> 4) & 0x3;
    if adaptation & 0x1 == 0 {
        return None;
    }
    let start = if adaptation == 3 {
        5 + p[4] as usize
    } else {
        4
    };
    p.get(start..).filter(|rest| !rest.is_empty())
}

/// The PSI section a unit-start packet opens, when the whole of it fits in that packet.
pub fn section(p: &[u8]) -> Option<&[u8]> {
    let payload = payload(p)?;
    let pointer = *payload.first()? as usize;
    let s = payload.get(1 + pointer..)?;
    let length = (usize::from(s.get(1)? & 0x0f) << 8) | usize::from(*s.get(2)?);
    s.get(..3 + length)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIP: &[u8] = include_bytes!("../../tests/fixtures/livetv/clip.ts");

    #[test]
    fn any_chunking_yields_the_same_packets() {
        for size in [1usize, 7, 100, 188, 189, 1500, 8192] {
            let mut aligner = PacketAligner::default();
            let out: Vec<u8> = CLIP.chunks(size).flat_map(|c| aligner.push(c)).collect();
            assert_eq!(out, CLIP, "chunks of {size}");
        }
    }

    #[test]
    fn a_stream_joined_mid_packet_resyncs_on_the_next_one() {
        let mut aligner = PacketAligner::default();
        let out = aligner.push(&CLIP[100..]);
        assert_eq!(out, &CLIP[TS_PACKET..]);
    }

    #[test]
    fn a_packet_that_lost_sync_is_dropped_and_the_stream_recovers() {
        let mut torn = CLIP[..TS_PACKET * 10].to_vec();
        torn.drain(TS_PACKET * 3 + 50..TS_PACKET * 3 + 60);
        let mut aligner = PacketAligner::default();
        let mut out = aligner.push(&torn);
        out.extend(aligner.push(&CLIP[TS_PACKET * 10..TS_PACKET * 20]));
        assert!(out.starts_with(&CLIP[..TS_PACKET * 3]));
        assert!(out.ends_with(&CLIP[TS_PACKET * 5..TS_PACKET * 20]));
        assert_eq!(out.len() % TS_PACKET, 0);
    }
}
