pub const TONE_RATE: usize = 24_000;

pub fn tone_pcm(ms: usize) -> Vec<u8> {
    let samples = TONE_RATE * ms / 1000;
    (0..samples)
        .flat_map(|i| {
            let t = i as f64 / TONE_RATE as f64;
            (((t * 440.0 * std::f64::consts::TAU).sin() * 8000.0) as i16).to_le_bytes()
        })
        .collect()
}

pub fn tone_wav(ms: usize) -> Vec<u8> {
    let pcm = tone_pcm(ms);
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(TONE_RATE as u32).to_le_bytes());
    out.extend_from_slice(&(TONE_RATE as u32 * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(&pcm);
    out
}
