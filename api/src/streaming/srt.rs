//! SubRip for renderers that take no other sidecar: every text track, whatever it was at
//! source, flattened to timed plain lines.

use crate::streaming::subtitle::parse_vtt_time;

#[derive(Debug, Clone, PartialEq)]
struct Cue {
    start: f64,
    end: f64,
    text: String,
}

/// The track as SRT, moved `offset` seconds earlier for a player whose zero is that far
/// into the title. Cues that end before the new zero are dropped.
pub fn to_srt(content: &str, codec: &str, offset: f64) -> String {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut cues = match codec {
        "ass" | "ssa" => ass_cues(content),
        _ => timed_block_cues(content),
    };
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    let offset = offset.max(0.0);
    cues.iter()
        .filter(|c| c.end - offset > 0.0)
        .enumerate()
        .map(|(i, c)| {
            format!(
                "{}\n{} --> {}\n{}\n",
                i + 1,
                format_srt_time((c.start - offset).max(0.0)),
                format_srt_time(c.end - offset),
                c.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// SRT and WebVTT alike: blank-line separated blocks, each with one `-->` timing line.
fn timed_block_cues(content: &str) -> Vec<Cue> {
    let normalized = content.replace("\r\n", "\n");
    normalized
        .split("\n\n")
        .filter_map(|block| {
            let mut lines = block.lines().skip_while(|l| !l.contains(" --> "));
            let timing = lines.next()?;
            let (start, rest) = timing.split_once(" --> ")?;
            let end = rest.split_whitespace().next()?;
            let text = lines
                .map(strip_markup)
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string();
            Some(Cue {
                start: parse_vtt_time(&start.replace(',', "."))?,
                end: parse_vtt_time(&end.replace(',', "."))?,
                text,
            })
            .filter(|c| !c.text.is_empty())
        })
        .collect()
}

/// WebVTT tags other than the `<i>`, `<b>` and `<u>` SRT shares, and its entities.
fn strip_markup(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('>') else {
            rest = &rest[open..];
            break;
        };
        let tag = &rest[open..open + close + 1];
        if matches!(tag, "<i>" | "</i>" | "<b>" | "</b>" | "<u>" | "</u>") {
            out.push_str(tag);
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

fn ass_cues(content: &str) -> Vec<Cue> {
    let mut fields: Vec<String> = Vec::new();
    let mut in_events = false;
    let mut cues = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_events = line.eq_ignore_ascii_case("[events]");
            continue;
        }
        if !in_events {
            continue;
        }
        if let Some(format) = line.strip_prefix("Format:") {
            fields = format
                .split(',')
                .map(|f| f.trim().to_ascii_lowercase())
                .collect();
            continue;
        }
        let Some(dialogue) = line.strip_prefix("Dialogue:") else {
            continue;
        };
        if let Some(cue) = ass_dialogue(&fields, dialogue) {
            cues.push(cue);
        }
    }
    cues
}

fn ass_dialogue(fields: &[String], dialogue: &str) -> Option<Cue> {
    let values: Vec<&str> = dialogue.splitn(fields.len().max(1), ',').collect();
    let field = |name: &str| {
        fields
            .iter()
            .position(|f| f == name)
            .and_then(|i| values.get(i))
            .map(|v| v.trim())
    };
    let raw = fields
        .iter()
        .position(|f| f == "text")
        .and_then(|i| values.get(i))?;
    let text = ass_text(raw)?;
    Some(Cue {
        start: parse_vtt_time(field("start")?)?,
        end: parse_vtt_time(field("end")?)?,
        text,
    })
}

/// Override blocks dropped, line breaks kept. A vector drawing (`\p1`) has no text to show.
fn ass_text(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        let block = &rest[open..open + close];
        if block.split('\\').any(|tag| {
            tag.strip_prefix('p')
                .is_some_and(|n| n.parse::<u32>().is_ok_and(|n| n > 0))
        }) {
            return None;
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    let text = out
        .replace("\\N", "\n")
        .replace("\\n", "\n")
        .replace("\\h", " ");
    let text = text
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    (!text.is_empty()).then_some(text)
}

fn format_srt_time(secs: f64) -> String {
    let ms = (secs * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_at_source_is_renumbered_and_shifted() {
        let srt = "\u{feff}5\r\n00:00:01,000 --> 00:00:02,000\r\nGone\r\n\r\n9\r\n00:01:00,500 --> 00:01:02,000\r\n<i>Kept</i>\r\n";
        assert_eq!(
            to_srt(srt, "subrip", 30.0),
            "1\n00:00:30,500 --> 00:00:32,000\n<i>Kept</i>\n"
        );
    }

    #[test]
    fn vtt_short_timestamps_and_voice_tags() {
        let vtt = "WEBVTT\n\nNOTE skip me\n\ncue-1\n01:02.500 --> 01:04.000 align:start\n<v.loud Ann>Hi</v> &lt;3\n";
        assert_eq!(
            to_srt(vtt, "webvtt", 0.0),
            "1\n00:01:02,500 --> 00:01:04,000\nHi <3\n"
        );
    }

    #[test]
    fn ass_drawings_and_comments_are_left_out() {
        let ass = "[Events]\nFormat: Layer, Start, End, Style, Text\nComment: 0,0:00:01.00,0:00:02.00,Default,note\nDialogue: 0,0:00:03.00,0:00:04.00,Default,{\\p1}m 0 0 l 100 0{\\p0}\nDialogue: 0,0:00:05.00,0:00:06.00,Default,a, b\\hc\n";
        assert_eq!(
            to_srt(ass, "ass", 0.0),
            "1\n00:00:05,000 --> 00:00:06,000\na, b c\n"
        );
    }
}
