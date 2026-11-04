use super::scan::FrameTable;

const MAX_SAMPLES_PER_CHUNK: u32 = 1024;
const MATRIX: [u32; 9] = [0x1_0000, 0, 0, 0, 0x1_0000, 0, 0, 0, 0x4000_0000];

pub fn header(table: &FrameTable) -> Vec<u8> {
    let payload = table.end_byte() - table.first_byte();
    let mdat_header = if payload + 8 > u64::from(u32::MAX) {
        16
    } else {
        8
    };
    let ftyp = ftyp();
    let fixed = (ftyp.len() + mdat_header) as u64;
    let co64 = fixed + moov(table, 0, false).len() as u64 + payload > u64::from(u32::MAX);
    let base = fixed + moov(table, 0, co64).len() as u64;

    let mut out = ftyp;
    out.extend(moov(table, base, co64));
    if mdat_header == 16 {
        put32(&mut out, 1);
        out.extend_from_slice(b"mdat");
        put64(&mut out, payload + 16);
    } else {
        put32(&mut out, payload as u32 + 8);
        out.extend_from_slice(b"mdat");
    }
    out
}

fn ftyp() -> Vec<u8> {
    let mut out = Vec::new();
    bx(&mut out, b"ftyp", |b| {
        b.extend_from_slice(b"isom");
        put32(b, 0x200);
        b.extend_from_slice(b"isomiso2mp41");
    });
    out
}

fn moov(t: &FrameTable, base: u64, co64: bool) -> Vec<u8> {
    let rate = t.sample_rate;
    let presented = t.presented_samples();
    let mut out = Vec::new();
    bx(&mut out, b"moov", |b| {
        versioned(b, b"mvhd", rate, presented, |b| {
            put32(b, 0x1_0000);
            put16(b, 0x0100);
            b.extend_from_slice(&[0; 10]);
            MATRIX.iter().for_each(|&m| put32(b, m));
            b.extend_from_slice(&[0; 24]);
            put32(b, 2);
        });
        bx(b, b"trak", |b| {
            tkhd(b, presented);
            bx(b, b"edts", |b| elst(b, presented, t.encoder_delay));
            bx(b, b"mdia", |b| {
                versioned(b, b"mdhd", rate, t.total_samples(), |b| {
                    put16(b, 0x55c4);
                    put16(b, 0);
                });
                full(b, b"hdlr", 0, 0, |b| {
                    put32(b, 0);
                    b.extend_from_slice(b"soun");
                    b.extend_from_slice(&[0; 12]);
                    b.extend_from_slice(b"SoundHandler\0");
                });
                bx(b, b"minf", |b| {
                    full(b, b"smhd", 0, 0, |b| put32(b, 0));
                    bx(b, b"dinf", |b| {
                        full(b, b"dref", 0, 0, |b| {
                            put32(b, 1);
                            full(b, b"url ", 0, 1, |_| {});
                        })
                    });
                    bx(b, b"stbl", |b| stbl(b, t, base, co64));
                });
            });
        });
    });
    out
}

fn tkhd(b: &mut Vec<u8>, duration: u64) {
    let wide = duration > u64::from(u32::MAX);
    full(b, b"tkhd", u8::from(wide), 3, |b| {
        let times = if wide { 16 } else { 8 };
        b.extend(std::iter::repeat_n(0, times));
        put32(b, 1);
        put32(b, 0);
        put_time(b, wide, duration);
        b.extend_from_slice(&[0; 8]);
        put16(b, 0);
        put16(b, 1);
        put16(b, 0x0100);
        put16(b, 0);
        MATRIX.iter().for_each(|&m| put32(b, m));
        put32(b, 0);
        put32(b, 0);
    });
}

// Movie and media share the sample rate as timescale, so the edit list trims exactly the
// encoder delay and end padding the mp3 demuxer reports, as ffmpeg's mp4 muxer does.
fn elst(b: &mut Vec<u8>, presented: u64, delay: u32) {
    let wide = presented > u64::from(u32::MAX);
    full(b, b"elst", u8::from(wide), 0, |b| {
        put32(b, 1);
        put_time(b, wide, presented);
        put_time(b, wide, u64::from(delay));
        put32(b, 0x1_0000);
    });
}

fn stbl(b: &mut Vec<u8>, t: &FrameTable, base: u64, co64: bool) {
    full(b, b"stsd", 0, 0, |b| {
        put32(b, 1);
        bx(b, b"mp4a", |b| {
            b.extend_from_slice(&[0; 6]);
            put16(b, 1);
            b.extend_from_slice(&[0; 8]);
            put16(b, t.channels);
            put16(b, 16);
            put32(b, 0);
            put32(b, t.sample_rate << 16);
            esds(b, t);
        });
    });

    let durations = run_lengths(t.frames.iter().map(|f| f.samples));
    full(b, b"stts", 0, 0, |b| {
        put32(b, durations.len() as u32);
        for (count, delta) in durations {
            put32(b, count);
            put32(b, delta);
        }
    });

    let chunks = chunks(t);
    let per_chunk = run_lengths(chunks.iter().map(|&(_, n)| n));
    full(b, b"stsc", 0, 0, |b| {
        put32(b, per_chunk.len() as u32);
        let mut first = 1;
        for (count, samples) in per_chunk {
            put32(b, first);
            put32(b, samples);
            put32(b, 1);
            first += count;
        }
    });

    full(b, b"stsz", 0, 0, |b| {
        let size = t.frames[0].size;
        let uniform = t.frames.iter().all(|f| f.size == size);
        put32(b, if uniform { size } else { 0 });
        put32(b, t.frames.len() as u32);
        if !uniform {
            t.frames.iter().for_each(|f| put32(b, f.size));
        }
    });

    full(b, if co64 { b"co64" } else { b"stco" }, 0, 0, |b| {
        put32(b, chunks.len() as u32);
        for (pos, _) in &chunks {
            let offset = base + pos - t.first_byte();
            if co64 {
                put64(b, offset);
            } else {
                put32(b, offset as u32);
            }
        }
    });
}

fn esds(b: &mut Vec<u8>, t: &FrameTable) {
    let object_type = if t.sample_rate > 24_000 { 0x6B } else { 0x69 };
    let (max, avg) = bitrates(t);
    full(b, b"esds", 0, 0, |b| {
        descriptor(b, 0x03, |b| {
            put16(b, 1);
            b.push(0);
            descriptor(b, 0x04, |b| {
                b.push(object_type);
                b.push(0x15);
                b.extend_from_slice(&[0; 3]);
                put32(b, max);
                put32(b, avg);
            });
            descriptor(b, 0x06, |b| b.push(0x02));
        });
    });
}

fn bitrates(t: &FrameTable) -> (u32, u32) {
    let rate = u64::from(t.sample_rate);
    let mut seconds: Vec<u64> = Vec::new();
    let mut clock = 0u64;
    for f in &t.frames {
        let second = (clock / rate) as usize;
        if seconds.len() <= second {
            seconds.resize(second + 1, 0);
        }
        seconds[second] += u64::from(f.size) * 8;
        clock += u64::from(f.samples);
    }
    let bytes: u64 = t.frames.iter().map(|f| u64::from(f.size)).sum();
    let avg = bytes * 8 * rate / t.total_samples().max(1);
    let max = seconds.into_iter().max().unwrap_or(0).max(avg);
    (clamp32(max), clamp32(avg))
}

fn chunks(t: &FrameTable) -> Vec<(u64, u32)> {
    let mut chunks: Vec<(u64, u32)> = Vec::new();
    let mut end = None;
    for f in &t.frames {
        match chunks.last_mut() {
            Some((_, n)) if end == Some(f.pos) && *n < MAX_SAMPLES_PER_CHUNK => *n += 1,
            _ => chunks.push((f.pos, 1)),
        }
        end = Some(f.pos + u64::from(f.size));
    }
    chunks
}

fn run_lengths(values: impl Iterator<Item = u32>) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for v in values {
        match runs.last_mut() {
            Some((count, value)) if *value == v => *count += 1,
            _ => runs.push((1, v)),
        }
    }
    runs
}

fn versioned(
    b: &mut Vec<u8>,
    kind: &[u8; 4],
    timescale: u32,
    duration: u64,
    rest: impl FnOnce(&mut Vec<u8>),
) {
    let wide = duration > u64::from(u32::MAX);
    full(b, kind, u8::from(wide), 0, |b| {
        put_time(b, wide, 0);
        put_time(b, wide, 0);
        put32(b, timescale);
        put_time(b, wide, duration);
        rest(b);
    });
}

fn descriptor(b: &mut Vec<u8>, tag: u8, body: impl FnOnce(&mut Vec<u8>)) {
    let mut inner = Vec::new();
    body(&mut inner);
    b.push(tag);
    let len = inner.len() as u32;
    b.extend_from_slice(&[
        0x80 | ((len >> 21) as u8 & 0x7f),
        0x80 | ((len >> 14) as u8 & 0x7f),
        0x80 | ((len >> 7) as u8 & 0x7f),
        len as u8 & 0x7f,
    ]);
    b.extend(inner);
}

fn full(b: &mut Vec<u8>, kind: &[u8; 4], version: u8, flags: u32, body: impl FnOnce(&mut Vec<u8>)) {
    bx(b, kind, |b| {
        put32(b, u32::from(version) << 24 | flags);
        body(b);
    });
}

fn bx(b: &mut Vec<u8>, kind: &[u8; 4], body: impl FnOnce(&mut Vec<u8>)) {
    let start = b.len();
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(kind);
    body(b);
    let size = (b.len() - start) as u32;
    b[start..start + 4].copy_from_slice(&size.to_be_bytes());
}

fn put_time(b: &mut Vec<u8>, wide: bool, v: u64) {
    if wide {
        put64(b, v);
    } else {
        put32(b, v as u32);
    }
}

fn clamp32(v: u64) -> u32 {
    u32::try_from(v).unwrap_or(u32::MAX)
}

fn put16(b: &mut Vec<u8>, v: u16) {
    b.extend_from_slice(&v.to_be_bytes());
}

fn put32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_be_bytes());
}

fn put64(b: &mut Vec<u8>, v: u64) {
    b.extend_from_slice(&v.to_be_bytes());
}
