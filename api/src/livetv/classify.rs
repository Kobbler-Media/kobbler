use super::hdhomerun::LineupEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Tv,
    Radio,
    Dead,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Tv => "tv",
            Kind::Radio => "radio",
            Kind::Dead => "dead",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "tv" => Some(Kind::Tv),
            "radio" => Some(Kind::Radio),
            "dead" => Some(Kind::Dead),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Classification {
    pub kind: Kind,
    pub type_label: String,
    pub enabled_on_first_scan: bool,
}

pub fn kind(onid: Option<i64>, video_codec: Option<&str>, has_guide: bool) -> Kind {
    if onid.is_none() && !has_guide {
        return Kind::Dead;
    }
    if video_codec.is_none() {
        return Kind::Radio;
    }
    Kind::Tv
}

pub fn type_label(kind: Kind, video_codec: Option<&str>, hd: bool) -> String {
    match kind {
        Kind::Dead => "Dead".to_string(),
        Kind::Radio => "Radio".to_string(),
        Kind::Tv if hd => "HD".to_string(),
        Kind::Tv => match video_codec.unwrap_or("").to_ascii_uppercase().as_str() {
            "H264" | "AVC" | "H.264" => "SD H.264".to_string(),
            "MPEG2" | "MPEG-2" => "SD MPEG-2".to_string(),
            "HEVC" | "H265" | "H.265" => "SD HEVC".to_string(),
            _ => "SD".to_string(),
        },
    }
}

pub fn mux_label(frequency_hz: i64) -> String {
    let mhz = frequency_hz as f64 / 1_000_000.0;
    if (mhz - mhz.round()).abs() < 0.05 {
        format!("{mhz:.0} MHz")
    } else {
        format!("{mhz:.1} MHz")
    }
}

pub fn classify(entry: &LineupEntry, has_guide: bool) -> Classification {
    let k = kind(
        entry.original_network_id,
        entry.video_codec.as_deref(),
        has_guide,
    );
    Classification {
        kind: k,
        type_label: type_label(k, entry.video_codec.as_deref(), entry.is_hd()),
        enabled_on_first_scan: k == Kind::Tv,
    }
}

pub fn display_name(
    display_name: Option<&str>,
    scan_name: Option<&str>,
    callsign: Option<&str>,
) -> String {
    [display_name, scan_name, callsign]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or("Unknown")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINEUP_FIXTURE: &str = include_str!("../../tests/fixtures/livetv/lineup.json");

    fn fixture() -> Vec<LineupEntry> {
        serde_json::from_str(LINEUP_FIXTURE).expect("lineup fixture parses")
    }

    #[test]
    fn mux_labels_format_as_megahertz() {
        assert_eq!(mux_label(226_500_000), "226.5 MHz");
        assert_eq!(mux_label(529_500_000), "529.5 MHz");
        assert_eq!(mux_label(530_000_000), "530 MHz");
    }

    #[test]
    fn display_name_falls_through_to_unknown() {
        assert_eq!(display_name(Some("Mine"), Some("Scan"), None), "Mine");
        assert_eq!(display_name(None, Some("Scan"), Some("CS")), "Scan");
        assert_eq!(display_name(None, Some("  "), Some("CS")), "CS");
        assert_eq!(display_name(None, None, None), "Unknown");
    }

    #[test]
    fn real_lineup_classifies_into_the_expected_buckets() {
        let entries = fixture();
        assert_eq!(entries.len(), 107, "fixture size changed");

        let mut hd = 0;
        let mut sd_h264 = 0;
        let mut sd_mpeg2 = 0;
        let mut radio = 0;
        let mut dead = 0;
        let mut enabled = 0;

        for e in &entries {
            let c = classify(e, false);
            if c.enabled_on_first_scan {
                enabled += 1;
            }
            match c.type_label.as_str() {
                "HD" => hd += 1,
                "SD H.264" => sd_h264 += 1,
                "SD MPEG-2" => sd_mpeg2 += 1,
                "Radio" => radio += 1,
                "Dead" => dead += 1,
                other => panic!("unexpected type_label {other} for {}", e.guide_number),
            }
        }

        assert_eq!((hd, sd_h264, sd_mpeg2, radio, dead), (39, 20, 10, 17, 21));
        assert_eq!(enabled, 69);
    }

    #[test]
    fn the_dead_entries_are_the_unnamed_lcn_5000_block() {
        for e in fixture() {
            let is_dead =
                kind(e.original_network_id, e.video_codec.as_deref(), false) == Kind::Dead;
            if is_dead {
                assert_eq!(e.guide_name.as_deref(), Some("Unknown"));
                assert!(e.lcn().unwrap_or(0) >= 5000, "{}", e.guide_number);
            }
        }
    }

    #[test]
    fn guide_data_rescues_a_channel_with_no_onid() {
        let entry: LineupEntry = serde_json::from_str(
            r#"{"GuideNumber":"5001","GuideName":"Unknown","Frequency":177500000,
                "ProgramNumber":1,"VideoCodec":"H264"}"#,
        )
        .unwrap();
        assert_eq!(classify(&entry, false).kind, Kind::Dead);
        assert_eq!(classify(&entry, true).kind, Kind::Tv);
    }

    #[test]
    fn every_frequency_and_program_number_pair_is_unique() {
        let mut seen = std::collections::HashSet::new();
        for e in fixture() {
            assert!(
                seen.insert((e.frequency, e.program_number)),
                "duplicate physical identity for {}",
                e.guide_number
            );
        }
    }
}
