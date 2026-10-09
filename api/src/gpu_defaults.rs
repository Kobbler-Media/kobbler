//! GPU-scheduler config for **standalone** kobbler.
//!
//! When koblem is the GPU authority, kobbler routes its STT/TTS/vision work
//! through it, koblem owns the live `kob-gpu-gate` config, and kobbler's own
//! [`GpuLock`](kob_gpu_gate::GpuLock) stays a no-op: it seeds no cards, so every
//! `acquire_for_*` returns a no-op guard.
//!
//! Standalone (the default) there is no authority to route to: kobbler talks to
//! the GPU services directly and seeds its local gate from here, so its own GPU
//! jobs are coordinated by VRAM-budget admission + eviction instead of racing.
//! The gate places a service on a card iff its configured peak fits that card's
//! remaining budget; candidates are tried in order and a full card makes the
//! acquire wait rather than overcommit.
//!
//! The card list is discovered from `nvidia-smi` at startup. No NVIDIA GPU means
//! no cards, which means the gate budgets nothing and every acquire is a no-op
//! guard — the correct behaviour for a deployment whose GPU services are absent
//! or CPU-backed.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use kob_gpu_gate::{Card, EngineDesc, ServiceKind};

/// Withheld from every card's budget: the driver, the desktop compositor and
/// each process's CUDA context consume VRAM the gate never sees, so budgeting
/// the full reported figure overcommits the card.
const VRAM_HEADROOM_PCT: u32 = 5;

const DETECT_TIMEOUT: Duration = Duration::from_secs(5);
const DETECT_POLL: Duration = Duration::from_millis(25);

// Conservative starting estimates, not measurements that hold everywhere: peaks
// move with the model, the batch size and the request shape. A deployment that
// OOMs is reserving too little; one that serialises jobs a card could have held
// side by side is reserving too much.
const LLM_PEAK_MB: u32 = 10000;
const TTS_PEAK_MB: u32 = 3000;
const VISION_PEAK_MB: u32 = 2000;
const STT_PEAK_MB: u32 = 1500;

pub fn detect_cards() -> Vec<Card> {
    let Some(csv) = nvidia_smi_csv() else {
        tracing::info!(
            "GPU detection: nvidia-smi unavailable or failed — no GPU cards; the local GPU gate will not budget-gate anything"
        );
        return Vec::new();
    };
    let cards = cards_from_csv(&csv);
    if cards.is_empty() {
        tracing::info!(
            "GPU detection: nvidia-smi reported no usable cards; the local GPU gate will not budget-gate anything"
        );
    } else {
        let summary = cards
            .iter()
            .map(|c| {
                format!(
                    "{} ({} MB budget{})",
                    c.uuid,
                    c.total_mb,
                    if c.fast { ", fast" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        tracing::info!("GPU detection: {} card(s): {summary}", cards.len());
    }
    cards
}

fn nvidia_smi_csv() -> Option<String> {
    let mut child = Command::new("nvidia-smi")
        .args([
            "--query-gpu=uuid,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + DETECT_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                break;
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(DETECT_POLL),
            // A wedged nvidia-smi (a hung driver) must not hold up startup.
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Err(_) => return None,
        }
    }

    // Draining stdout only after exit is safe: the reply is a handful of CSV
    // lines, far short of the pipe buffer.
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    Some(out)
}

fn budget_mb(total_mb: u32) -> u32 {
    total_mb.saturating_sub(total_mb * VRAM_HEADROOM_PCT / 100)
}

fn cards_from_csv(csv: &str) -> Vec<Card> {
    let mut cards: Vec<Card> = csv
        .lines()
        .filter_map(|line| {
            let (uuid, total) = line.split_once(',')?;
            let uuid = uuid.trim();
            let budget = budget_mb(total.trim().parse().ok()?);
            (!uuid.is_empty() && budget > 0).then(|| Card {
                uuid: uuid.to_string(),
                total_mb: budget,
                fast: false,
                enabled: true,
            })
        })
        .collect();

    // "Fast" drives the gate's spill ranking. Rank by VRAM: the biggest card is
    // the one worth reaching for first, and it is the only ordering that can be
    // read off the hardware.
    if let Some(i) = cards
        .iter()
        .enumerate()
        .max_by_key(|(_, c)| c.total_mb)
        .map(|(i, _)| i)
    {
        cards[i].fast = true;
    }
    cards
}

fn desc(peak_mb: u32, home: &str, cands: Vec<String>, relocatable: bool) -> EngineDesc {
    EngineDesc {
        peak_mb,
        home_gpu: home.to_string(),
        candidate_gpus: cands,
        // Nothing preempts anything: with only these four services there is no
        // renderer that would be worth booting a resident off a card for, so a
        // service whose candidates are all full waits instead of evicting.
        evictable_by: Vec::new(),
        relocatable,
        rules_json: String::new(),
    }
}

fn candidates(cards: [&Card; 2]) -> Vec<String> {
    let mut out = vec![cards[0].uuid.clone()];
    if cards[1].uuid != cards[0].uuid {
        out.push(cards[1].uuid.clone());
    }
    out
}

pub fn default_engines(cards: &[Card]) -> Vec<(ServiceKind, EngineDesc)> {
    let Some(fast) = cards.iter().find(|c| c.fast).or_else(|| cards.first()) else {
        return Vec::new();
    };
    let slow = cards.iter().find(|c| !c.fast).unwrap_or(fast);

    vec![
        // The LLM is the one long-lived resident, so it keeps the smaller card to
        // itself and leaves the roomier one for work that spills.
        (
            ServiceKind::Llm,
            desc(LLM_PEAK_MB, &slow.uuid, vec![slow.uuid.clone()], true),
        ),
        (
            ServiceKind::Tts,
            desc(TTS_PEAK_MB, &fast.uuid, candidates([fast, slow]), false),
        ),
        (
            ServiceKind::Vision,
            desc(VISION_PEAK_MB, &slow.uuid, candidates([slow, fast]), false),
        ),
        (
            ServiceKind::Stt,
            desc(STT_PEAK_MB, &slow.uuid, candidates([slow, fast]), false),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLOW: &str = "GPU-00000000-1111-2222-3333-444444444444";
    const FAST: &str = "GPU-55555555-6666-7777-8888-999999999999";
    const SAMPLE: &str = "GPU-00000000-1111-2222-3333-444444444444, 12288\nGPU-55555555-6666-7777-8888-999999999999, 16311\n";

    fn engine(engines: &[(ServiceKind, EngineDesc)], k: ServiceKind) -> &EngineDesc {
        engines
            .iter()
            .find(|(kk, _)| *kk == k)
            .map(|(_, d)| d)
            .unwrap_or_else(|| panic!("no engine entry for {k:?}"))
    }

    #[test]
    fn parses_multi_card_csv_with_headroom_and_fast_ranking() {
        let cards = cards_from_csv(SAMPLE);
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].uuid, SLOW);
        assert_eq!(cards[1].uuid, FAST);
        assert_eq!(cards[0].total_mb, budget_mb(12288));
        assert_eq!(cards[1].total_mb, budget_mb(16311));
        assert!(
            cards[0].total_mb < 12288,
            "headroom is withheld from the budget"
        );
        assert!(!cards[0].fast);
        assert!(cards[1].fast, "the largest card is the fast one");
        assert!(cards.iter().all(|c| c.enabled));
    }

    #[test]
    fn parser_skips_junk_and_never_panics() {
        for junk in [
            "",
            "   ",
            "\n\n",
            "No devices were found",
            "GPU-x",
            "GPU-x, notanumber",
            ", 12288",
            "GPU-x, [N/A]",
            "GPU-x, 0",
            "NVIDIA-SMI has failed because it couldn't communicate with the NVIDIA driver.",
        ] {
            assert!(
                cards_from_csv(junk).is_empty(),
                "expected no cards from {junk:?}"
            );
        }

        let mixed = "No devices were found\nGPU-aaa, 8192\nGPU-bbb, [N/A]\n\n";
        let cards = cards_from_csv(mixed);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].uuid, "GPU-aaa");
        assert!(cards[0].fast);
    }

    #[test]
    fn no_gpu_yields_no_cards_and_no_engines() {
        assert!(cards_from_csv("").is_empty());
        assert!(default_engines(&[]).is_empty());
    }

    #[test]
    fn detect_cards_never_panics() {
        let _ = detect_cards();
    }

    #[test]
    fn single_card_homes_every_service() {
        let cards = cards_from_csv("GPU-only, 8192\n");
        let engines = default_engines(&cards);
        assert_eq!(engines.len(), 4);
        for k in [
            ServiceKind::Llm,
            ServiceKind::Tts,
            ServiceKind::Stt,
            ServiceKind::Vision,
        ] {
            let d = engine(&engines, k);
            assert_eq!(d.home_gpu, "GPU-only", "{k:?} home");
            assert_eq!(
                d.candidate_gpus,
                vec!["GPU-only".to_string()],
                "{k:?} candidates"
            );
            assert!(d.peak_mb > 0, "{k:?} peak");
            assert!(d.evictable_by.is_empty(), "{k:?} evictable_by");
        }
    }

    #[test]
    fn two_cards_split_fast_and_slow() {
        let engines = default_engines(&cards_from_csv(SAMPLE));
        assert_eq!(engines.len(), 4);

        let tts = engine(&engines, ServiceKind::Tts);
        assert_eq!(tts.home_gpu, FAST);
        assert_eq!(tts.candidate_gpus, vec![FAST.to_string(), SLOW.to_string()]);

        for k in [ServiceKind::Stt, ServiceKind::Vision] {
            let d = engine(&engines, k);
            assert_eq!(d.home_gpu, SLOW, "{k:?} home");
            assert_eq!(
                d.candidate_gpus,
                vec![SLOW.to_string(), FAST.to_string()],
                "{k:?} spills to the fast card"
            );
        }

        let llm = engine(&engines, ServiceKind::Llm);
        assert_eq!(llm.home_gpu, SLOW);
        assert_eq!(llm.candidate_gpus, vec![SLOW.to_string()]);
        assert!(llm.relocatable);

        assert!(engines.iter().all(|(_, d)| d.evictable_by.is_empty()));
    }

    #[test]
    fn engine_table_covers_exactly_the_live_services() {
        let engines = default_engines(&cards_from_csv(SAMPLE));
        let mut kinds: Vec<String> = engines.iter().map(|(k, _)| format!("{k:?}")).collect();
        kinds.sort();
        assert_eq!(kinds, vec!["Llm", "Stt", "Tts", "Vision"]);
    }
}
