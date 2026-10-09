//! Local repro for vHash intro matching.
//!
//! Reproduces the Leftovers S3 "zero vhash intros" bug from 2026-04-21.
//! Runs the exact V1 pipeline step that failed: extract_descriptors →
//! find_per_episode_vhash.
//!
//! Usage:
//!   cargo run --example vhash_harness --release -- \
//!     "/path/to/ep1.mkv" "/path/to/ep2.mkv" [--region intro|credits]

use std::path::{Path, PathBuf};

use api::skip_detect::tv::find_per_episode_vhash;
use api::skip_detect::vhash::{self, FrameDescriptor};

const MIN_MATCH_DURATION: f64 = 15.0;

fn descriptor_stats(label: &str, d: &FrameDescriptor) {
    let min = *d.values.iter().min().unwrap_or(&0);
    let max = *d.values.iter().max().unwrap_or(&0);
    let mean = d.values.iter().map(|&v| v as f64).sum::<f64>() / d.values.len().max(1) as f64;
    let nonzero = d.values.iter().filter(|&&v| v != 0).count();
    let first8: Vec<u8> = d.values.iter().take(8).copied().collect();
    println!(
        "  {label:>14} t={:>7.2}s  min={min:>3}  max={max:>3}  mean={mean:>6.2}  nonzero={nonzero}/{}  q=0x{:08x}  first8={:?}",
        d.timestamp,
        d.values.len(),
        d.quantized,
        first8,
    );
}

fn summarise(seq: &[FrameDescriptor]) {
    if seq.is_empty() {
        println!("  (empty sequence — extraction produced no descriptors)");
        return;
    }
    println!("  descriptors: {} frames", seq.len());
    println!(
        "  timestamp range: {:.2}s .. {:.2}s",
        seq.first().unwrap().timestamp,
        seq.last().unwrap().timestamp,
    );

    // Check how many descriptors are all-zero (black frames / failed extraction)
    let all_zero = seq
        .iter()
        .filter(|d| d.values.iter().all(|&v| v == 0))
        .count();
    let all_same = seq
        .iter()
        .filter(|d| {
            let first = d.values.first().copied().unwrap_or(0);
            d.values.iter().all(|&v| v == first)
        })
        .count();
    let unique_q = seq
        .iter()
        .map(|d| d.quantized)
        .collect::<std::collections::HashSet<_>>()
        .len();
    println!(
        "  all-zero descriptors: {all_zero}/{}  all-same (flat): {all_same}/{}  unique-q: {unique_q}",
        seq.len(),
        seq.len(),
    );

    descriptor_stats("first", &seq[0]);
    descriptor_stats("mid", &seq[seq.len() / 2]);
    descriptor_stats("last", &seq[seq.len() - 1]);

    // Self-similarity check — a descriptor vs itself should be 1.0.
    let self_sim = vhash::frame_similarity(&seq[0].values, &seq[0].values);
    println!("  frame_similarity(a,a) for seq[0] = {self_sim:.4} (expected ~1.0)");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "Usage: {} <ep1.mkv> <ep2.mkv> [--region intro|credits] [--window SECS] [--dump-raw]",
            args[0]
        );
        std::process::exit(2);
    }
    let ep1: PathBuf = args[1].clone().into();
    let ep2: PathBuf = args[2].clone().into();

    let mut region = "intro".to_string();
    let mut window: f64 = 180.0; // extraction_windows default is ~min(180s, 25% of runtime) for TV
    let mut dump_raw = false;
    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--region" => {
                region = args[i + 1].clone();
                i += 2;
            }
            "--window" => {
                window = args[i + 1].parse()?;
                i += 2;
            }
            "--dump-raw" => {
                dump_raw = true;
                i += 1;
            }
            other => {
                eprintln!("unknown arg: {other}");
                std::process::exit(2);
            }
        }
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    println!("== vhash_harness ==");
    println!("ep1      = {}", ep1.display());
    println!("ep2      = {}", ep2.display());
    println!("region   = {region}");
    println!("window   = {window:.1}s");

    // Probe file durations via ffprobe so we can place the credits window.
    let (dur1, dur2) = (probe_duration(&ep1).await?, probe_duration(&ep2).await?);
    println!("ep1 duration = {dur1:.2}s");
    println!("ep2 duration = {dur2:.2}s");

    let (start1, len1, start2, len2) = match region.as_str() {
        "intro" => (0.0, window, 0.0, window),
        "credits" => (
            dur1 * 0.80,
            dur1 - dur1 * 0.80,
            dur2 * 0.80,
            dur2 - dur2 * 0.80,
        ),
        other => {
            eprintln!("--region must be intro or credits, got {other}");
            std::process::exit(2);
        }
    };

    println!("\n-- extracting descriptors for ep1 ({region}, {start1:.1}s + {len1:.1}s) --");
    let t0 = std::time::Instant::now();
    let d1 = vhash::extract_descriptors(&ep1, start1, len1).await?;
    println!(
        "extracted {} descriptors in {:.1}s",
        d1.len(),
        t0.elapsed().as_secs_f64()
    );
    summarise(&d1);

    println!("\n-- extracting descriptors for ep2 ({region}, {start2:.1}s + {len2:.1}s) --");
    let t0 = std::time::Instant::now();
    let d2 = vhash::extract_descriptors(&ep2, start2, len2).await?;
    println!(
        "extracted {} descriptors in {:.1}s",
        d2.len(),
        t0.elapsed().as_secs_f64()
    );
    summarise(&d2);

    if dump_raw {
        dump_sequence(Path::new("/tmp/vhash_ep1.bin"), &d1)?;
        dump_sequence(Path::new("/tmp/vhash_ep2.bin"), &d2)?;
        println!("wrote /tmp/vhash_ep{{1,2}}.bin");
    }

    if d1.is_empty() || d2.is_empty() {
        println!("\n❌ at least one descriptor sequence is empty — extraction is the bug");
        std::process::exit(1);
    }

    // Direct pairwise call
    println!("\n-- find_visual_matches(d1, d2, min_dur=15.0, min_sim=0.70) --");
    let pair = vhash::find_visual_matches(&d1, &d2, MIN_MATCH_DURATION, 0.70);
    match &pair {
        Some(p) => {
            println!(
                "✅ matched: lhs={:.2}..{:.2}s (dens={:.3})  rhs={:.2}..{:.2}s (dens={:.3})",
                p.lhs.start_seconds,
                p.lhs.end_seconds,
                p.lhs.density,
                p.rhs.start_seconds,
                p.rhs.end_seconds,
                p.rhs.density,
            );
        }
        None => println!("❌ find_visual_matches returned None"),
    }

    // Full per-episode call (matches the production code path exactly).
    println!("\n-- find_per_episode_vhash([ep1, ep2], min_dur=15.0) --");
    let seqs = vec![(0usize, d1.clone()), (1usize, d2.clone())];
    let map = find_per_episode_vhash(&seqs, MIN_MATCH_DURATION);
    if map.is_empty() {
        println!("❌ find_per_episode_vhash returned EMPTY map");
    } else {
        for (ep, detail) in &map {
            println!(
                "  ep={ep}: {:.2}..{:.2}s density={:.3} pair_count={} pairs={}",
                detail.start_seconds,
                detail.end_seconds,
                detail.density,
                detail.pair_count,
                detail.pairwise.len(),
            );
        }
    }

    // Also try with the stricter production threshold (MIN_MATCH_DENSITY = 0.70)
    // vs lower thresholds to see where behaviour bifurcates.
    println!("\n-- find_visual_matches at different min_similarity thresholds --");
    for &thresh in &[0.50, 0.60, 0.65, 0.67, 0.68, 0.69, 0.70, 0.72, 0.75, 0.80] {
        let r = vhash::find_visual_matches(&d1, &d2, MIN_MATCH_DURATION, thresh);
        match r {
            Some(p) => println!(
                "  thresh={thresh:.2}  lhs={:.1}..{:.1}s dens={:.3}  rhs={:.1}..{:.1}s",
                p.lhs.start_seconds,
                p.lhs.end_seconds,
                p.lhs.density,
                p.rhs.start_seconds,
                p.rhs.end_seconds
            ),
            None => println!("  thresh={thresh:.2}  None"),
        }
    }

    // Manual phase-1 emulation: run the loop ourselves to see per-shift stats.
    println!("\n-- phase-1 per-shift (threshold 0.70) --");
    {
        // Reimplement find_alignment_shifts + validate_at_shift to expose stats.
        let mut index: std::collections::HashMap<u32, Vec<usize>> =
            std::collections::HashMap::new();
        for (i, d) in d2.iter().enumerate() {
            index.entry(d.quantized).or_default().push(i);
        }
        let mut votes: std::collections::HashMap<i32, usize> = std::collections::HashMap::new();
        for (i, a) in d1.iter().enumerate() {
            if let Some(js) = index.get(&a.quantized) {
                for &j in js {
                    let s = vhash::frame_similarity(&a.values, &d2[j].values);
                    if s >= 0.80 {
                        *votes.entry(j as i32 - i as i32).or_default() += 1;
                    }
                }
            }
        }
        let mut shifts: Vec<(i32, usize)> = votes.into_iter().filter(|&(_, c)| c >= 2).collect();
        shifts.sort_by_key(|b| std::cmp::Reverse(b.1));
        shifts.truncate(10);
        println!("  top shifts (capped 10): {shifts:?}");

        for &(shift, _) in &shifts {
            let mut matched: Vec<(f64, f64, f64)> = Vec::new();
            for (i, a) in d1.iter().enumerate() {
                let j = i as i32 + shift;
                if j < 0 || j as usize >= d2.len() {
                    continue;
                }
                let sim = vhash::frame_similarity(&a.values, &d2[j as usize].values);
                if sim >= 0.70 {
                    matched.push((a.timestamp, d2[j as usize].timestamp, sim));
                }
            }
            if matched.is_empty() {
                println!("  shift={shift} pairs=0");
                continue;
            }
            // Find longest contiguous run (gap_a <= 5.5)
            let max_gap = 15.0 / 3.0 + 0.5;
            let mut best_len = 1usize;
            let mut best_span = 0.0f64;
            let mut run_len = 1usize;
            let mut run_start = matched[0].0;
            let mut best_start = matched[0].0;
            let mut best_end = matched[0].0;
            for i in 1..matched.len() {
                let ga = matched[i].0 - matched[i - 1].0;
                let gb = matched[i].1 - matched[i - 1].1;
                if ga <= max_gap && gb >= 0.0 && gb <= max_gap {
                    run_len += 1;
                } else {
                    let span = matched[i - 1].0 - run_start;
                    if run_len > best_len {
                        best_len = run_len;
                        best_span = span;
                        best_start = run_start;
                        best_end = matched[i - 1].0;
                    }
                    run_len = 1;
                    run_start = matched[i].0;
                }
            }
            let span = matched.last().unwrap().0 - run_start;
            if run_len > best_len {
                best_len = run_len;
                best_span = span;
                best_start = run_start;
                best_end = matched.last().unwrap().0;
            }
            let expected = (best_span * 3.0) as usize + 1;
            let density = (best_len as f64 / expected as f64).min(1.0);
            println!(
                "  shift={shift:>3} pairs={:>3} longest_run={best_len} span={best_span:.1}s density={density:.3} ({best_start:.1}..{best_end:.1}s)",
                matched.len()
            );
        }
    }

    // Similarity histogram at the dominant shift for the first 100s of ep1/ep2
    // (roughly where the intro lives). Helps diagnose whether frames are
    // borderline or cliff-falling around the production 0.70 threshold.
    println!("\n-- similarity distribution along best shift (first 270 frames ≈ 90s) --");
    let index_b: std::collections::HashMap<u32, Vec<usize>> = {
        let mut m = std::collections::HashMap::new();
        for (i, d) in d2.iter().enumerate() {
            m.entry(d.quantized).or_insert_with(Vec::new).push(i);
        }
        m
    };
    // Compute most-voted shift across all frames
    let mut shift_votes: std::collections::HashMap<i32, usize> = std::collections::HashMap::new();
    for (i, a) in d1.iter().enumerate() {
        if let Some(matches) = index_b.get(&a.quantized) {
            for &j in matches {
                let sim = vhash::frame_similarity(&a.values, &d2[j].values);
                if sim >= 0.80 {
                    *shift_votes.entry(j as i32 - i as i32).or_insert(0) += 1;
                }
            }
        }
    }
    let best_shift = shift_votes
        .iter()
        .max_by_key(|&(_, c)| *c)
        .map(|(&s, _)| s)
        .unwrap_or(0);
    println!("  dominant shift = {best_shift} frames");

    // Bucket similarities at the dominant shift
    let mut buckets = [0usize; 11]; // 0.0-0.1, 0.1-0.2, …, 1.0
    let mut lo_samples: Vec<(f64, f64)> = Vec::new();
    for (i, a) in d1.iter().enumerate().take(270) {
        let j = i as i32 + best_shift;
        if j < 0 || j as usize >= d2.len() {
            continue;
        }
        let b = &d2[j as usize];
        let sim = vhash::frame_similarity(&a.values, &b.values);
        let idx = ((sim * 10.0) as usize).min(10);
        buckets[idx] += 1;
        if sim < 0.70 && a.values.iter().any(|&v| v != 0) {
            lo_samples.push((a.timestamp, sim));
        }
    }
    for (k, c) in buckets.iter().enumerate() {
        println!(
            "  sim {:.1}..{:.1}: {c}",
            k as f64 * 0.1,
            (k as f64 + 1.0) * 0.1
        );
    }
    println!(
        "  sub-0.70 (non-black) sample timestamps (first 15): {:?}",
        lo_samples.iter().take(15).collect::<Vec<_>>()
    );

    Ok(())
}

async fn probe_duration(path: &Path) -> Result<f64, Box<dyn std::error::Error>> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || -> Result<f64, String> {
        kob_ffmpeg::init();
        let ctx =
            kob_ffmpeg::format::input::InputContext::open_path(&path).map_err(|e| e.to_string())?;
        ctx.duration_secs()
            .ok_or_else(|| "duration unavailable".to_string())
    })
    .await?
    .map_err(|e| e.into())
}

fn dump_sequence(path: &Path, seq: &[FrameDescriptor]) -> std::io::Result<()> {
    use std::io::Write;
    let bytes = vhash::serialize_descriptors(seq);
    let mut f = std::fs::File::create(path)?;
    f.write_all(&bytes)?;
    Ok(())
}
