use std::io::{BufReader, Read};
use std::path::Path;

use kob_ffmpeg::MediaType;
use kob_ffmpeg::format::input::InputContext;
use kob_ffmpeg::packet::Packet;

use super::VirtualMp4Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub pos: u64,
    pub size: u32,
    pub samples: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameTable {
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: Vec<Frame>,
    pub encoder_delay: u32,
    pub end_padding: u32,
}

impl FrameTable {
    pub fn first_byte(&self) -> u64 {
        self.frames[0].pos
    }

    pub fn end_byte(&self) -> u64 {
        self.frames.last().map_or(0, |f| f.pos + u64::from(f.size))
    }

    pub fn total_samples(&self) -> u64 {
        self.frames.iter().map(|f| u64::from(f.samples)).sum()
    }

    pub fn presented_samples(&self) -> u64 {
        self.total_samples()
            .saturating_sub(u64::from(self.encoder_delay) + u64::from(self.end_padding))
    }
}

// The table mirrors the packets ffmpeg's own mp3 demuxer emits, which fold any junk between
// frames into the following packet, so stream-copying those packets into an mp4 and pointing
// a sample table at the source bytes produce the same samples. Every packet is checked against
// the source because the virtual file serves the source bytes, not the demuxer's copy.
pub fn scan(source: &Path) -> Result<FrameTable, VirtualMp4Error> {
    let mut input = InputContext::open_path(source)?;
    input.find_stream_info()?;
    let audio = input
        .find_best_stream(MediaType::Audio)
        .ok_or_else(|| VirtualMp4Error::Unsupported("no audio stream".into()))?;
    input.discard_all_except(audio);
    let stream = input.stream(audio).expect("best stream exists");
    if stream.codec_id() != kob_ffmpeg::ffi::AV_CODEC_ID_MP3 {
        return Err(VirtualMp4Error::Unsupported("audio is not mp3".into()));
    }
    let sample_rate = u32::try_from(stream.sample_rate())
        .ok()
        .filter(|r| (1..=u32::from(u16::MAX)).contains(r))
        .ok_or_else(|| VirtualMp4Error::Unsupported("bad sample rate".into()))?;
    let channels = u16::try_from(stream.channel_layout().channels())
        .map_err(|_| VirtualMp4Error::Unsupported("bad channel count".into()))?;
    let tb = stream.time_base();

    let mut verify = SourceCheck::open(source)?;
    let mut table = FrameTable {
        sample_rate,
        channels,
        frames: Vec::new(),
        encoder_delay: 0,
        end_padding: 0,
    };
    let mut packet = Packet::new()?;
    while input.read_packet(&mut packet)? {
        if packet.stream_index() != audio as i32 {
            continue;
        }
        let frame = Frame {
            pos: u64::try_from(packet.pos())
                .map_err(|_| VirtualMp4Error::Unsupported("packet without position".into()))?,
            size: packet.size() as u32,
            samples: samples(packet.duration(), tb, sample_rate)?,
        };
        if frame.pos < table.end_byte() {
            return Err(VirtualMp4Error::Unsupported("packets overlap".into()));
        }
        verify.matches(frame.pos, packet.data())?;
        let (skip, padding) = packet.skip_samples().unwrap_or_default();
        if table.frames.is_empty() {
            table.encoder_delay = skip;
        }
        table.end_padding = padding;
        table.frames.push(frame);
    }
    if table.frames.is_empty() {
        return Err(VirtualMp4Error::Unsupported("no mp3 frames".into()));
    }
    Ok(table)
}

fn samples(
    duration: i64,
    tb: kob_ffmpeg::ffi::AVRational,
    sample_rate: u32,
) -> Result<u32, VirtualMp4Error> {
    let scaled = i128::from(duration) * i128::from(tb.num) * i128::from(sample_rate);
    let den = i128::from(tb.den);
    u32::try_from((scaled + den / 2) / den)
        .ok()
        .filter(|&s| s > 0)
        .ok_or_else(|| VirtualMp4Error::Unsupported("packet without duration".into()))
}

struct SourceCheck {
    reader: BufReader<std::fs::File>,
    at: u64,
    buf: Vec<u8>,
}

impl SourceCheck {
    fn open(path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            reader: BufReader::with_capacity(1 << 16, std::fs::File::open(path)?),
            at: 0,
            buf: Vec::new(),
        })
    }

    fn matches(&mut self, pos: u64, data: &[u8]) -> Result<(), VirtualMp4Error> {
        self.reader.seek_relative(pos as i64 - self.at as i64)?;
        self.buf.resize(data.len(), 0);
        self.reader.read_exact(&mut self.buf)?;
        self.at = pos + data.len() as u64;
        if self.buf != data {
            return Err(VirtualMp4Error::Unsupported(format!(
                "packet at byte {pos} is not the source's bytes"
            )));
        }
        Ok(())
    }
}
