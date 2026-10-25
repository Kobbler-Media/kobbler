//! Length-preserving rewrites spliced into a Matroska file's bytes as they are served.

use super::mkv_cue_trim::CuePatch;

/// A replacement for `region_len` bytes at `offset`. Only `prefix` is held; the rest of
/// the region is zeros, which is what an EBML Void payload is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Splice {
    pub offset: u64,
    prefix: Vec<u8>,
    region_len: usize,
}

impl Splice {
    pub fn new(offset: u64, prefix: Vec<u8>, region_len: usize) -> Self {
        debug_assert!(prefix.len() <= region_len);
        Self {
            offset,
            prefix,
            region_len,
        }
    }

    pub fn end(&self) -> u64 {
        self.offset + self.region_len as u64
    }

    pub fn region_len(&self) -> usize {
        self.region_len
    }

    pub fn stored_bytes(&self) -> usize {
        self.prefix.len()
    }

    fn fill(&self, from: usize, out: &mut [u8]) {
        // `from` runs past the prefix once the range starts inside the Void payload,
        // and even an empty slice at an out-of-bounds start panics.
        let start = from.min(self.prefix.len());
        let real = (self.prefix.len() - start).min(out.len());
        out[..real].copy_from_slice(&self.prefix[start..start + real]);
        out[real..].fill(0);
    }

    pub fn overlaps(&self, start: u64, len: usize) -> bool {
        start < self.end() && self.offset < start + len as u64
    }

    /// The replaced bytes for a range lying wholly inside the region.
    ///
    /// The demuxer fetches the cue index as one range; answering it from here skips
    /// the read (or, on debrid, the upstream request) for bytes that would be
    /// overwritten in full anyway.
    pub fn whole_range(&self, start: u64, len: usize) -> Option<Vec<u8>> {
        let end = start.checked_add(len as u64)?;
        if start < self.offset || end > self.end() {
            return None;
        }
        let mut out = vec![0u8; len];
        self.fill((start - self.offset) as usize, &mut out);
        Some(out)
    }

    /// Overwrite the part of an already-read chunk that falls inside the region.
    pub fn apply(&self, chunk_start: u64, chunk: &mut [u8]) {
        let chunk_end = chunk_start + chunk.len() as u64;
        let from = chunk_start.max(self.offset);
        let to = chunk_end.min(self.end());
        if from >= to {
            return;
        }
        let ci = (from - chunk_start) as usize;
        let n = (to - from) as usize;
        self.fill((from - self.offset) as usize, &mut chunk[ci..ci + n]);
    }
}

/// Everything served differently from the file on disk: a reordered SeekHead and/or a
/// trimmed Cues element. The two never overlap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexPatch {
    pub seekhead: Option<Splice>,
    pub cues: Option<CuePatch>,
}

impl IndexPatch {
    pub fn new(seekhead: Option<Splice>, cues: Option<CuePatch>) -> Option<Self> {
        (seekhead.is_some() || cues.is_some()).then_some(Self { seekhead, cues })
    }

    fn splices(&self) -> impl Iterator<Item = &Splice> {
        self.seekhead
            .iter()
            .chain(self.cues.as_ref().map(|c| &c.splice))
    }

    pub fn overlaps(&self, start: u64, len: usize) -> bool {
        self.splices().any(|s| s.overlaps(start, len))
    }

    pub fn whole_range(&self, start: u64, len: usize) -> Option<Vec<u8>> {
        self.splices().find_map(|s| s.whole_range(start, len))
    }

    pub fn apply(&self, chunk_start: u64, chunk: &mut [u8]) {
        for s in self.splices() {
            s.apply(chunk_start, chunk);
        }
    }
}
