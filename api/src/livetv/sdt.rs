//! Service names read from the mux's own SDT.
//!
//! `lineup.json` truncates `GuideName` to 15 characters — 22 of this tuner's 107
//! services hit that wall, so `ABC Kids/ABC Family` reaches us as
//! `ABC Kids/ABC Fa`. The broadcaster sends the full name in the Service
//! Description Table, which the device strips from a single-service stream
//! (`/auto/v<lcn>`) but passes through whole when the mux is tuned by frequency
//! (`/auto/ch<hz>`). `service_id` there is the lineup's `ProgramNumber`, so
//! `(frequency, program_number)` joins the two.

use super::hdhomerun::HdhrError;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::time::Duration;

const TS_PACKET: usize = 188;
const SDT_PID: u16 = 0x0011;
const TABLE_ID_SDT_ACTUAL: u8 = 0x42;
const SERVICE_DESCRIPTOR: u8 = 0x48;

/// The SDT repeats at least every 2 s, which measured 5.7 MB on this tuner's
/// busiest mux. Four times that is the give-up point, not the expected read.
const MAX_BYTES: usize = 24 * 1024 * 1024;
const MAX_WAIT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdtService {
    pub service_id: u16,
    pub provider: Option<String>,
    pub name: String,
}

/// DVB text (EN 300 468 annex A). A leading byte below 0x20 selects the
/// character table; its absence means ISO 6937, which agrees with Latin-1 over
/// the ASCII range every Australian service name stays inside. Bytes 0x80-0x9F
/// are DVB control codes, not characters.
fn dvb_text(raw: &[u8]) -> String {
    let (encoding, body) = match raw.first() {
        Some(0x15) => (Encoding::Utf8, &raw[1..]),
        // 0x10 is a three-byte selector: 0x10 0x00 <n> for ISO 8859-n.
        Some(0x10) if raw.len() >= 3 => (Encoding::Latin, &raw[3..]),
        Some(&b) if b < 0x20 => (Encoding::Latin, &raw[1..]),
        _ => (Encoding::Latin, raw),
    };

    let decoded = match encoding {
        Encoding::Utf8 => String::from_utf8_lossy(body).into_owned(),
        Encoding::Latin => body.iter().map(|&b| b as char).collect(),
    };
    decoded
        .chars()
        .filter(|c| !matches!(*c as u32, 0x80..=0x9f))
        .collect::<String>()
        .trim()
        .to_string()
}

enum Encoding {
    Utf8,
    Latin,
}

fn parse_section(section: &[u8]) -> Vec<SdtService> {
    // table_id(1) section_length(2) tsid(2) version(1) section_no(2)
    // original_network_id(2) reserved(1) = 11 before the service loop, CRC32 after.
    if section.len() < 15 || section[0] != TABLE_ID_SDT_ACTUAL {
        return Vec::new();
    }
    let section_length = (((section[1] & 0x0f) as usize) << 8) | section[2] as usize;
    let end = (section_length + 3).saturating_sub(4).min(section.len());

    let mut out = Vec::new();
    let mut i = 11;
    while i + 5 <= end {
        let service_id = ((section[i] as u16) << 8) | section[i + 1] as u16;
        let loop_len = (((section[i + 3] & 0x0f) as usize) << 8) | section[i + 4] as usize;
        let loop_end = (i + 5 + loop_len).min(end);

        let mut j = i + 5;
        while j + 2 <= loop_end {
            let tag = section[j];
            let len = section[j + 1] as usize;
            let body_end = (j + 2 + len).min(loop_end);
            if tag == SERVICE_DESCRIPTOR {
                // service_type(1) provider_len(1) provider provider_name_len(1) name
                let d = &section[j + 2..body_end];
                if d.len() >= 2 {
                    let provider_len = d[1] as usize;
                    let name_at = 2 + provider_len;
                    if name_at < d.len() {
                        let name_len = d[name_at] as usize;
                        let name_end = (name_at + 1 + name_len).min(d.len());
                        let name = dvb_text(&d[name_at + 1..name_end]);
                        if !name.is_empty() {
                            out.push(SdtService {
                                service_id,
                                provider: Some(dvb_text(&d[2..2 + provider_len.min(d.len() - 2)]))
                                    .filter(|p| !p.is_empty()),
                                name,
                            });
                        }
                    }
                }
            }
            j += 2 + len;
        }
        i += 5 + loop_len;
    }
    out
}

/// Reassembles PID 0x11 across TS packets. A section spans several packets and a
/// live capture starts mid-section, so anything before the first payload-unit
/// start is dropped rather than guessed at.
#[derive(Default)]
pub struct SdtReader {
    partial: Option<Vec<u8>>,
    services: HashMap<u16, SdtService>,
    carry: Vec<u8>,
    synced: bool,
}

impl SdtReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn services(&self) -> Vec<SdtService> {
        let mut out: Vec<SdtService> = self.services.values().cloned().collect();
        out.sort_by_key(|s| s.service_id);
        out
    }

    pub fn is_complete(&self) -> bool {
        !self.services.is_empty()
    }

    /// Feed the next slice of the stream. Chunk boundaries are arbitrary, so
    /// whatever does not make up a whole packet is carried into the next call.
    pub fn push(&mut self, chunk: &[u8]) {
        self.carry.extend_from_slice(chunk);

        // The device's first bytes are not necessarily packet-aligned, so the
        // offset is somewhere in the first packet's worth. Confirm it with a run
        // of sync bytes — a lone 0x47 is a common payload value.
        if !self.synced {
            let runs = (self.carry.len() / TS_PACKET).clamp(1, 4);
            let span = TS_PACKET * (runs - 1) + 1;
            let Some(offset) = (0..TS_PACKET.min(self.carry.len()))
                .filter(|&i| i + span <= self.carry.len())
                .find(|&i| (0..runs).all(|k| self.carry[i + k * TS_PACKET] == 0x47))
            else {
                // Not a transport stream at all, or still too short to tell.
                if self.carry.len() > TS_PACKET * 64 {
                    self.carry.drain(..TS_PACKET * 32);
                }
                return;
            };
            self.carry.drain(..offset);
            self.synced = true;
        }

        let whole = self.carry.len() / TS_PACKET * TS_PACKET;
        let packets: Vec<u8> = self.carry.drain(..whole).collect();
        for packet in packets.as_chunks::<TS_PACKET>().0 {
            self.packet(packet);
        }
    }

    fn packet(&mut self, p: &[u8]) {
        if p[0] != 0x47 {
            self.synced = false;
            return;
        }
        let pid = (((p[1] & 0x1f) as u16) << 8) | p[2] as u16;
        if pid != SDT_PID {
            return;
        }
        let adaptation = (p[3] >> 4) & 0x3;
        let mut i = 4;
        if adaptation == 2 {
            return;
        }
        if adaptation == 3 {
            i += 1 + p[4] as usize;
        }
        if i >= TS_PACKET {
            return;
        }

        if p[1] & 0x40 != 0 {
            let pointer = p[i] as usize;
            i += 1 + pointer;
            if i > TS_PACKET {
                return;
            }
            self.partial = Some(p[i..].to_vec());
        } else if let Some(buf) = self.partial.as_mut() {
            buf.extend_from_slice(&p[i..]);
        }

        let Some(buf) = self.partial.as_ref() else {
            return;
        };
        if buf.len() < 3 {
            return;
        }
        let section_length = (((buf[1] & 0x0f) as usize) << 8) | buf[2] as usize;
        if buf.len() < section_length + 3 {
            return;
        }
        for service in parse_section(&buf[..section_length + 3]) {
            self.services.insert(service.service_id, service);
        }
        self.partial = None;
    }
}

/// Tune the whole mux and read until the SDT lands. One tuner for a couple of
/// seconds per mux; the device hands back an error rather than preempting a
/// viewer when all four are busy, so this needs no lease of its own.
pub async fn fetch_mux(base_url: &str, frequency_hz: i64) -> Result<Vec<SdtService>, HdhrError> {
    let host = base_url
        .trim()
        .trim_end_matches('/')
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    let url = format!("http://{host}:5004/auto/ch{frequency_hz}");

    let resp = crate::http::new_client()
        .get(&url)
        .timeout(MAX_WAIT)
        .send()
        .await?
        .error_for_status()?;

    let mut reader = SdtReader::new();
    let mut read = 0usize;
    let mut stream = resp.bytes_stream();
    let deadline = tokio::time::Instant::now() + MAX_WAIT;

    while let Ok(Some(Ok(chunk))) = tokio::time::timeout_at(deadline, stream.next()).await {
        read += chunk.len();
        reader.push(&chunk);
        if reader.is_complete() || read >= MAX_BYTES {
            break;
        }
    }
    // Running out of time or bytes is not an error: whatever sections landed are
    // still real names, and the caller keeps the lineup's for the rest.
    Ok(reader.services())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SDT_FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/livetv/sdt.ts");

    fn parsed() -> Vec<SdtService> {
        let mut r = SdtReader::new();
        r.push(SDT_FIXTURE);
        r.services()
    }

    /// The whole point: `lineup.json` calls 562 `ABC Kids/ABC Fa`, and the mux
    /// calls it what it is.
    #[test]
    fn the_mux_carries_the_names_the_lineup_truncated() {
        let by_id: HashMap<u16, String> = parsed()
            .into_iter()
            .map(|s| (s.service_id, s.name))
            .collect();
        assert_eq!(
            by_id.get(&562).map(String::as_str),
            Some("ABC Kids/ABC Family")
        );
        assert_eq!(
            by_id.get(&570).map(String::as_str),
            Some("triple j Unearthed")
        );
        assert_eq!(by_id.get(&561).map(String::as_str), Some("ABC TV"));
    }

    #[test]
    fn every_service_on_the_mux_is_named_and_attributed() {
        let services = parsed();
        assert_eq!(services.len(), 16, "the 226.5 MHz mux carries 16 services");
        assert!(services.iter().all(|s| !s.name.is_empty()));
        assert!(
            services
                .iter()
                .all(|s| s.provider.as_deref() == Some("ABC")),
            "the SDT provider is a far tidier network name than the guide's"
        );
    }

    /// A live capture is joined mid-stream and mid-section, and the first bytes
    /// are not necessarily packet-aligned.
    #[test]
    fn a_capture_joined_mid_section_still_syncs() {
        // The stream itself is aligned; it is our read that starts mid-packet.
        let stream: Vec<u8> = SDT_FIXTURE.repeat(4);
        for skip in [1usize, 57, 187, TS_PACKET, TS_PACKET + 3] {
            let mut r = SdtReader::new();
            r.push(&stream[skip..]);
            assert!(
                r.is_complete(),
                "no section recovered after joining at byte {skip}"
            );
            assert_eq!(r.services().len(), 16, "joined at byte {skip}");
        }
    }

    /// The device streams at ~23 Mbit/s into whatever chunk size the HTTP stack
    /// hands back, which never aligns to 188 bytes.
    #[test]
    fn arbitrary_chunk_boundaries_do_not_lose_a_section() {
        for chunk in [1usize, 7, 100, 188, 189, 1500] {
            let mut r = SdtReader::new();
            for part in SDT_FIXTURE.chunks(chunk) {
                r.push(part);
            }
            assert_eq!(r.services().len(), 16, "chunk size {chunk} lost services");
        }
    }

    #[test]
    fn dvb_text_handles_the_selector_byte_and_control_codes() {
        assert_eq!(dvb_text(b"ABC TV"), "ABC TV");
        assert_eq!(dvb_text(b"\x01ABC TV"), "ABC TV");
        assert_eq!(dvb_text(b"\x15ABC TV"), "ABC TV");
        assert_eq!(dvb_text(b"\x10\x00\x0fABC TV"), "ABC TV");
        assert_eq!(dvb_text(b"ABC\x8aTV"), "ABCTV");
        assert_eq!(dvb_text(b"  ABC  "), "ABC");
        assert_eq!(dvb_text(b""), "");
    }

    #[test]
    fn a_truncated_or_foreign_section_yields_nothing_rather_than_panicking() {
        assert!(parse_section(&[]).is_empty());
        assert!(parse_section(&[0x42, 0x00]).is_empty());
        // pointer_field is byte 4 of a payload-unit-start packet.
        let section = &SDT_FIXTURE[5 + SDT_FIXTURE[4] as usize..];
        assert_eq!(section[0], TABLE_ID_SDT_ACTUAL);
        // 0x46 is the SDT for *other* transport streams — a different mux.
        let mut other = section.to_vec();
        other[0] = 0x46;
        assert!(parse_section(&other).is_empty());
        // A section cut short anywhere must return what it can, never index past.
        for cut in 1..section.len() {
            let _ = parse_section(&section[..cut]);
        }
    }
}
