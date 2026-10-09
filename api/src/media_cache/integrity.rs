//! Cheap "is this file actually all there?" check.
//!
//! An unfinished download is not a broken file in any way a container parser
//! notices. It has a valid header, a plausible duration, and decodes — it just
//! has holes where pieces never arrived, and those holes read as zeros. One
//! file in this library was probed, analyzed and skip-detected without
//! complaint while carrying zero blocks scattered through it.
//!
//! Sampling a handful of blocks separates the two cleanly. Compressed video
//! does not produce a run of 64 KB of zeros; a partial download is full of
//! them. Measured over this library: 150 healthy files and 6 files flagged by a
//! cruder head/tail heuristic all scored 0, while the three known-bad files
//! scored 9, 12 and 5 out of 12. One zero block is enough to be suspicious.

use std::path::Path;

/// Big enough that a run this long is not something a compressed stream emits
/// by accident.
pub const SAMPLE_BLOCK: u64 = 65536;
/// Blocks sampled per file. 12 × 64 KB ≈ 768 KB read.
pub const SAMPLE_COUNT: u64 = 12;
/// Skipped at each end. Containers legitimately pad there — an all-zero
/// trailing block is normal for AVI, and cost six false positives when a
/// tail-only check was tried.
pub const EDGE_MARGIN: u64 = 1024 * 1024;
/// Below this there is not enough middle to sample meaningfully.
pub const MIN_SAMPLE_SIZE: u64 = 8 * 1024 * 1024;

/// How many sampled blocks were entirely zero.
///
/// `None` means the file could not be read, which is a different problem from
/// "read fine and looks incomplete" and is left for the caller to interpret.
/// Blocking I/O — call it from the blocking pool.
pub fn count_zero_blocks(path: &Path, size: u64) -> Option<u64> {
    use std::io::{Read, Seek, SeekFrom};

    if size < MIN_SAMPLE_SIZE {
        return Some(0);
    }
    let lo = EDGE_MARGIN;
    let hi = size.checked_sub(EDGE_MARGIN + SAMPLE_BLOCK)?;
    if hi <= lo {
        return Some(0);
    }

    let mut f = std::fs::File::open(path).ok()?;
    let span = hi - lo;
    let mut buf = vec![0u8; SAMPLE_BLOCK as usize];
    let mut zeros = 0;
    for i in 0..SAMPLE_COUNT {
        let off = lo + span * i / (SAMPLE_COUNT - 1);
        if f.seek(SeekFrom::Start(off)).is_err() {
            return None;
        }
        let mut filled = 0usize;
        while filled < buf.len() {
            match f.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(_) => return None,
            }
        }
        if filled > 0 && buf[..filled].iter().all(|b| *b == 0) {
            zeros += 1;
        }
    }
    Some(zeros)
}

/// `count_zero_blocks` off the runtime. The media sits on a network mount, so
/// these reads can stall for as long as the link takes to answer.
pub async fn count_zero_blocks_async(path: &Path, size: u64) -> Option<u64> {
    let p = path.to_path_buf();
    tokio::task::spawn_blocking(move || count_zero_blocks(&p, size))
        .await
        .unwrap_or(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    #[test]
    fn small_files_are_not_sampled() {
        let p = write_temp("integ_small.bin", &[0u8; 1024]);
        assert_eq!(count_zero_blocks(&p, 1024), Some(0));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn an_all_zero_file_scores_every_block() {
        let size = 16 * 1024 * 1024;
        let p = write_temp("integ_zero.bin", &vec![0u8; size]);
        let n = count_zero_blocks(&p, size as u64).unwrap();
        assert_eq!(n, SAMPLE_COUNT, "every sampled block is zero");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn dense_data_scores_nothing() {
        let size = 16 * 1024 * 1024;
        // Non-zero everywhere, cycling so no 64KB window is uniform.
        let data: Vec<u8> = (0..size).map(|i| ((i % 255) + 1) as u8).collect();
        let p = write_temp("integ_dense.bin", &data);
        assert_eq!(count_zero_blocks(&p, size as u64), Some(0));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn zero_padding_at_the_edges_is_ignored() {
        // Zeros only inside EDGE_MARGIN at both ends; the middle is dense.
        let size = 16 * 1024 * 1024usize;
        let mut data: Vec<u8> = (0..size).map(|i| ((i % 255) + 1) as u8).collect();
        for b in data.iter_mut().take(EDGE_MARGIN as usize) {
            *b = 0;
        }
        for b in data.iter_mut().skip(size - EDGE_MARGIN as usize) {
            *b = 0;
        }
        let p = write_temp("integ_edges.bin", &data);
        assert_eq!(count_zero_blocks(&p, size as u64), Some(0));
        let _ = std::fs::remove_file(p);
    }
}
