//! Text alignment between audiobook transcripts and ebook text.
//!
//! Produces sync points mapping audio timestamps to ebook CFI positions
//! by fuzzy-matching chunks of transcript text against the ebook text.
//! The alignment doesn't need to be perfect — approximate mapping every
//! few minutes of audio is sufficient for "continue where I left off".

use uuid::Uuid;

/// A matched sync point between audio and ebook.
#[derive(Debug, Clone)]
pub struct AlignedSyncPoint {
    pub audio_file_id: Uuid,
    pub audio_seconds: f64,
    pub ebook_file_id: Uuid,
    pub ebook_cfi: String,
    pub confidence: f64,
}

/// Input segment from the audiobook transcript.
#[derive(Debug)]
pub struct AudioSegment {
    pub file_id: Uuid,
    pub start_seconds: f64,
    pub text: String,
}

/// Input segment from the ebook text.
#[derive(Debug)]
pub struct EbookTextSegment {
    pub file_id: Uuid,
    pub cfi: String,
    pub text: String,
}

/// Normalize text for comparison: lowercase, strip punctuation, collapse whitespace.
fn normalize(text: &str) -> String {
    text.chars()
        .filter_map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                Some(c.to_ascii_lowercase())
            } else {
                None
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Count matching words between two normalized texts using a sliding window.
fn word_overlap(needle: &[&str], haystack: &[&str]) -> (usize, usize) {
    if needle.is_empty() || haystack.is_empty() {
        return (0, 0);
    }

    let needle_len = needle.len();
    let mut best_matches = 0;
    let mut best_pos = 0;

    // Slide needle-sized window over haystack
    for start in 0..haystack.len().saturating_sub(needle_len / 2) {
        let end = (start + needle_len * 2).min(haystack.len());
        let window = &haystack[start..end];

        let matches = needle.iter().filter(|word| window.contains(word)).count();

        if matches > best_matches {
            best_matches = matches;
            best_pos = start;
        }
    }

    (best_matches, best_pos)
}

/// Align audiobook transcript segments against ebook text segments.
///
/// Strategy:
/// - Take chunks of ~5 consecutive audio segments (~30 seconds of speech)
/// - Normalize the text and search for word overlap against ebook segments
/// - Keep matches above a confidence threshold
/// - Sample every Nth audio segment group to avoid O(n*m) on huge books
pub fn align_texts(
    audio_segments: &[AudioSegment],
    ebook_segments: &[EbookTextSegment],
    sample_interval: usize,
) -> Vec<AlignedSyncPoint> {
    if audio_segments.is_empty() || ebook_segments.is_empty() {
        return Vec::new();
    }

    // Build normalized ebook word list with segment index references
    let ebook_normalized: Vec<(usize, String)> = ebook_segments
        .iter()
        .enumerate()
        .map(|(i, s)| (i, normalize(&s.text)))
        .collect();

    // Concatenate all ebook words with segment boundary tracking
    let mut all_words: Vec<&str> = Vec::new();
    let mut word_to_segment: Vec<usize> = Vec::new();

    for (seg_idx, (_, norm_text)) in ebook_normalized.iter().enumerate() {
        for word in norm_text.split_whitespace() {
            all_words.push(word);
            word_to_segment.push(seg_idx);
        }
    }

    let mut results = Vec::new();
    let chunk_size = 5; // ~5 segments ≈ 15-30 seconds of speech
    let interval = sample_interval.max(1);

    let mut i = 0;
    while i < audio_segments.len() {
        let chunk_end = (i + chunk_size).min(audio_segments.len());
        let chunk = &audio_segments[i..chunk_end];

        // Build normalized search text from this audio chunk
        let chunk_text: String = chunk
            .iter()
            .map(|s| normalize(&s.text))
            .collect::<Vec<_>>()
            .join(" ");
        let chunk_words: Vec<&str> = chunk_text.split_whitespace().collect();

        if chunk_words.len() >= 3 {
            let (matches, best_pos) = word_overlap(&chunk_words, &all_words);
            let confidence = matches as f64 / chunk_words.len() as f64;

            if confidence >= 0.4 && best_pos < word_to_segment.len() {
                let ebook_seg_idx = word_to_segment[best_pos];
                let ebook_seg = &ebook_segments[ebook_seg_idx];
                let audio_seg = &chunk[0];

                results.push(AlignedSyncPoint {
                    audio_file_id: audio_seg.file_id,
                    audio_seconds: audio_seg.start_seconds,
                    ebook_file_id: ebook_seg.file_id,
                    ebook_cfi: ebook_seg.cfi.clone(),
                    confidence,
                });
            }
        }

        i += chunk_size * interval;
    }

    results
}
