//! Kodi NFO XML parser and writer.
//!
//! Parses `movie.nfo`, `tvshow.nfo`, and `{filename}.nfo` (episode) files
//! in Kodi XML format. Writes modern Kodi v17+ format with `<uniqueid>` tags.

use super::{CastEntry, NfoType, SidecarFormat, SidecarMetadata, VideoSidecar};
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};
use std::io::Cursor;
use std::path::Path;

/// Parse a Kodi NFO XML string into `SidecarMetadata`.
///
/// Handles three root elements: `<movie>`, `<tvshow>`, `<episodedetails>`.
/// Gracefully skips non-XML content (some NFO files are ASCII art).
pub fn parse_nfo(content: &str) -> Result<SidecarMetadata, String> {
    let trimmed = content.trim();

    // Skip non-XML NFO files (ASCII art, plain text, URLs)
    if !trimmed.starts_with("<?xml") && !trimmed.starts_with('<') {
        return Err("Not an XML NFO file".to_string());
    }

    let mut reader = Reader::from_str(trimmed);
    reader.config_mut().trim_text(true);

    let mut meta = SidecarMetadata {
        source_format: Some(SidecarFormat::Nfo),
        ..Default::default()
    };
    let mut video = VideoSidecar::default();
    let mut has_video = false;

    // Track our current position in the XML tree
    let mut depth: Vec<String> = Vec::new();
    let mut in_actor = false;
    let mut current_actor = CastEntry::default();
    let mut in_uniqueid = false;
    let mut uniqueid_type = String::new();
    let mut in_ids_entry = false;
    let mut ids_key = String::new();
    // Buffer for accumulating text across Text and GeneralRef events
    let mut text_buf = String::new();
    let mut parsed_nfo_type: Option<NfoType> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                text_buf.clear();
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                depth.push(tag.clone());

                match tag.as_str() {
                    "movie" => {
                        has_video = true;
                        parsed_nfo_type = Some(NfoType::Movie);
                    }
                    "tvshow" => {
                        has_video = true;
                        parsed_nfo_type = Some(NfoType::TvShow);
                    }
                    "episodedetails" => {
                        has_video = true;
                        parsed_nfo_type = Some(NfoType::Episode);
                    }
                    "actor" | "producer" => {
                        in_actor = true;
                        current_actor = CastEntry::default();
                    }
                    "uniqueid" => {
                        in_uniqueid = true;
                        uniqueid_type = String::new();
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"type" {
                                uniqueid_type = String::from_utf8_lossy(&attr.value).to_lowercase();
                            }
                        }
                    }
                    "entry" if depth.iter().any(|d| d == "ids") => {
                        in_ids_entry = true;
                        ids_key.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                if let Ok(decoded) = e.decode() {
                    text_buf.push_str(&decoded);
                }
            }
            Ok(Event::GeneralRef(ref e)) => {
                if let Ok(decoded) = e.decode() {
                    text_buf.push_str(&decoded);
                }
            }
            Ok(Event::End(ref e)) => {
                let text = text_buf.trim().to_string();
                text_buf.clear();

                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();

                if !text.is_empty() {
                    let current_tag = depth.last().map(|s| s.as_str()).unwrap_or("");

                    if in_actor {
                        match current_tag {
                            "name" => current_actor.name = text,
                            "role" => current_actor.role = Some(text),
                            "thumb" => current_actor.thumb = Some(text),
                            _ => {}
                        }
                    } else if in_uniqueid {
                        match uniqueid_type.as_str() {
                            "imdb" => meta.identifiers.imdb_id = Some(text),
                            "tmdb" => meta.identifiers.tmdb_id = Some(text),
                            "tvdb" => meta.identifiers.tvdb_id = Some(text),
                            "tvmaze" => meta.identifiers.tvmaze_id = Some(text),
                            "kobbler" => meta.identifiers.kobbler_id = Some(text),
                            other if !other.is_empty() => {
                                meta.identifiers
                                    .extra_uniqueids
                                    .push((other.to_string(), text));
                            }
                            _ => {}
                        }
                    } else if in_ids_entry {
                        match current_tag {
                            "key" => ids_key = text,
                            "value" => match ids_key.as_str() {
                                "imdbId" => meta.identifiers.imdb_id = Some(text),
                                "tmdbId" => {
                                    meta.identifiers.tmdb_id = Some(text);
                                }
                                _ => {}
                            },
                            _ => {}
                        }
                    } else {
                        match current_tag {
                            "title" if depth.len() <= 2 => meta.title = Some(text),
                            "originaltitle" => meta.original_title = Some(text),
                            "year" => {
                                if let Ok(y) = text.parse::<u16>() {
                                    meta.year = Some(y);
                                }
                            }
                            "plot" => meta.plot = Some(text),
                            "rating" => {
                                if let Ok(r) = text.parse::<f64>() {
                                    meta.rating = Some(r);
                                }
                            }
                            "genre" => {
                                meta.genres.push(text);
                            }
                            "mpaa" | "certification" if video.mpaa.is_none() => {
                                video.mpaa = Some(text);
                            }
                            "studio" => video.studio = Some(text),
                            "status" => video.status = Some(text),
                            "premiered" | "aired" if video.aired.is_none() => {
                                video.aired = Some(text);
                            }
                            "season" if depth.len() <= 2 => {
                                if let Ok(s) = text.parse::<u32>() {
                                    video.season = Some(s);
                                }
                            }
                            "episode" if depth.len() <= 2 => {
                                if let Ok(ep) = text.parse::<u32>() {
                                    video.episode = Some(ep);
                                }
                            }
                            // Legacy ID tags (older NFO formats)
                            "id" if depth.len() <= 2 => {
                                if text.starts_with("tt") && meta.identifiers.imdb_id.is_none() {
                                    meta.identifiers.imdb_id = Some(text);
                                } else if meta.identifiers.tvdb_id.is_none()
                                    && text.parse::<u64>().is_ok()
                                {
                                    meta.identifiers.tvdb_id = Some(text);
                                }
                            }
                            "tmdbId" | "tmdbid" if meta.identifiers.tmdb_id.is_none() => {
                                meta.identifiers.tmdb_id = Some(text);
                            }
                            _ => {}
                        }
                    }
                }

                if tag == "actor" || tag == "producer" {
                    if !current_actor.name.is_empty() {
                        video.cast.push(current_actor.clone());
                    }
                    in_actor = false;
                }
                if tag == "uniqueid" {
                    in_uniqueid = false;
                }
                if tag == "entry" {
                    in_ids_entry = false;
                }
                depth.pop();
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML parse error: {e}")),
            _ => {}
        }
    }

    if has_video {
        video.nfo_type = parsed_nfo_type;
        meta.video = Some(video);
    }

    // Must have at least a title or identifiers to be useful
    if meta.title.is_none() && !meta.identifiers.has_any() {
        return Err("NFO has no title or identifiers".to_string());
    }

    Ok(meta)
}

/// Write a `SidecarMetadata` as a Kodi v17+ NFO XML file.
pub async fn write_nfo(path: &Path, meta: &SidecarMetadata) -> Result<(), std::io::Error> {
    let xml = build_nfo_xml(meta).map_err(std::io::Error::other)?;
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || std::fs::write(&path, xml))
        .await
        .map_err(|e| std::io::Error::other(format!("Join error: {e}")))?
}

fn build_nfo_xml(meta: &SidecarMetadata) -> Result<String, String> {
    let mut writer = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);

    // XML declaration
    writer
        .write_event(Event::Decl(quick_xml::events::BytesDecl::new(
            "1.0",
            Some("UTF-8"),
            Some("yes"),
        )))
        .map_err(|e| e.to_string())?;

    // Determine root element: use explicit nfo_type if set, fall back to heuristic
    let nfo_t = meta
        .video
        .as_ref()
        .and_then(|v| v.nfo_type)
        .unwrap_or_else(|| super::infer_nfo_type(meta));
    let root_tag = nfo_t.root_tag();

    writer
        .write_event(Event::Start(BytesStart::new(root_tag)))
        .map_err(|e| e.to_string())?;

    // Title
    if let Some(title) = &meta.title {
        write_text_element(&mut writer, "title", title)?;
    }
    if let Some(original) = &meta.original_title {
        write_text_element(&mut writer, "originaltitle", original)?;
    }
    if let Some(year) = meta.year {
        write_text_element(&mut writer, "year", &year.to_string())?;
    }
    if let Some(plot) = &meta.plot {
        write_text_element(&mut writer, "plot", plot)?;
    }
    if let Some(rating) = meta.rating {
        write_text_element(&mut writer, "rating", &format!("{rating:.1}"))?;
    }

    // Genres
    for genre in &meta.genres {
        write_text_element(&mut writer, "genre", genre)?;
    }

    // Video-specific fields
    if let Some(video) = &meta.video {
        if let Some(mpaa) = &video.mpaa {
            write_text_element(&mut writer, "mpaa", mpaa)?;
        }
        if let Some(studio) = &video.studio {
            write_text_element(&mut writer, "studio", studio)?;
        }
        if let Some(status) = &video.status {
            write_text_element(&mut writer, "status", status)?;
        }
        if let Some(aired) = &video.aired {
            write_text_element(&mut writer, "premiered", aired)?;
        }
        if let Some(season) = video.season {
            write_text_element(&mut writer, "season", &season.to_string())?;
        }
        if let Some(episode) = video.episode {
            write_text_element(&mut writer, "episode", &episode.to_string())?;
        }

        // Unique IDs (Kodi v17+ format)
        write_uniqueid(&mut writer, "imdb", meta.identifiers.imdb_id.as_deref())?;
        write_uniqueid(&mut writer, "tmdb", meta.identifiers.tmdb_id.as_deref())?;
        write_uniqueid(&mut writer, "tvdb", meta.identifiers.tvdb_id.as_deref())?;
        write_uniqueid(&mut writer, "tvmaze", meta.identifiers.tvmaze_id.as_deref())?;
        write_uniqueid(
            &mut writer,
            "kobbler",
            meta.identifiers.kobbler_id.as_deref(),
        )?;

        // Preserve unknown uniqueid types for round-trip
        for (id_type, value) in &meta.identifiers.extra_uniqueids {
            write_uniqueid(&mut writer, id_type, Some(value))?;
        }

        // Cast
        for actor in &video.cast {
            writer
                .write_event(Event::Start(BytesStart::new("actor")))
                .map_err(|e| e.to_string())?;
            write_text_element(&mut writer, "name", &actor.name)?;
            if let Some(role) = &actor.role {
                write_text_element(&mut writer, "role", role)?;
            }
            if let Some(thumb) = &actor.thumb {
                write_text_element(&mut writer, "thumb", thumb)?;
            }
            writer
                .write_event(Event::End(BytesEnd::new("actor")))
                .map_err(|e| e.to_string())?;
        }
    }

    writer
        .write_event(Event::End(BytesEnd::new(root_tag)))
        .map_err(|e| e.to_string())?;

    let result = writer.into_inner().into_inner();
    String::from_utf8(result).map_err(|e| e.to_string())
}

fn write_text_element(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    tag: &str,
    text: &str,
) -> Result<(), String> {
    writer
        .write_event(Event::Start(BytesStart::new(tag)))
        .map_err(|e| e.to_string())?;
    writer
        .write_event(Event::Text(BytesText::new(text)))
        .map_err(|e| e.to_string())?;
    writer
        .write_event(Event::End(BytesEnd::new(tag)))
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn write_uniqueid(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    id_type: &str,
    value: Option<&str>,
) -> Result<(), String> {
    if let Some(val) = value {
        let mut elem = BytesStart::new("uniqueid");
        elem.push_attribute(("type", id_type));
        writer
            .write_event(Event::Start(elem))
            .map_err(|e| e.to_string())?;
        writer
            .write_event(Event::Text(BytesText::new(val)))
            .map_err(|e| e.to_string())?;
        writer
            .write_event(Event::End(BytesEnd::new("uniqueid")))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::SidecarIdentifiers;

    #[test]
    fn test_parse_movie_nfo() {
        let nfo = include_str!("test_fixtures/movie.nfo");
        let meta = parse_nfo(nfo).expect("Failed to parse movie NFO");

        assert_eq!(
            meta.title.as_deref(),
            Some("Anchorman 2: The Legend Continues")
        );
        assert_eq!(meta.year, Some(2013));
        assert_eq!(meta.rating, Some(3.1));
        assert!(meta.plot.is_some());
        assert!(meta.genres.contains(&"Comedy".to_string()));

        // Identifiers
        assert_eq!(meta.identifiers.imdb_id.as_deref(), Some("tt1229340"));
        assert_eq!(meta.identifiers.tmdb_id.as_deref(), Some("109443"));

        // Cast
        let video = meta.video.expect("Should have video metadata");
        assert!(!video.cast.is_empty());
        assert_eq!(video.cast[0].name, "Will Ferrell");
        assert_eq!(video.cast[0].role.as_deref(), Some("Ron Burgundy"));
    }

    #[test]
    fn test_parse_tvshow_nfo() {
        let nfo = include_str!("test_fixtures/tvshow.nfo");
        let meta = parse_nfo(nfo).expect("Failed to parse tvshow NFO");

        assert_eq!(meta.title.as_deref(), Some("Barry"));
        assert_eq!(meta.rating, Some(8.3));
        assert_eq!(meta.identifiers.tvdb_id.as_deref(), Some("333072"));
        assert_eq!(meta.identifiers.imdb_id.as_deref(), Some("tt5348176"));
        assert_eq!(meta.identifiers.tmdb_id.as_deref(), Some("73107"));
        assert_eq!(meta.identifiers.tvmaze_id.as_deref(), Some("11538"));

        let video = meta.video.expect("Should have video metadata");
        assert_eq!(video.status.as_deref(), Some("Ended"));
        assert_eq!(video.studio.as_deref(), Some("HBO"));
        assert!(video.cast.len() >= 5);
    }

    #[test]
    fn test_parse_episode_nfo() {
        let nfo = include_str!("test_fixtures/episode.nfo");
        let meta = parse_nfo(nfo).expect("Failed to parse episode NFO");

        assert_eq!(meta.title.as_deref(), Some("Chapter One: Make Your Mark"));
        assert_eq!(meta.identifiers.tvdb_id.as_deref(), Some("6493154"));

        let video = meta.video.expect("Should have video metadata");
        assert_eq!(video.season, Some(1));
        assert_eq!(video.episode, Some(1));
        assert_eq!(video.aired.as_deref(), Some("2018-03-25"));
    }

    #[test]
    fn test_skip_non_xml_nfo() {
        let result = parse_nfo("This is not XML, just a text NFO file\nRelease info here");
        assert!(result.is_err());
    }

    #[test]
    fn test_roundtrip_kobbler_and_extra_uniqueids() {
        let original = SidecarMetadata {
            source_format: Some(SidecarFormat::Nfo),
            title: Some("Test Show".to_string()),
            identifiers: SidecarIdentifiers {
                tmdb_id: Some("99999".to_string()),
                kobbler_id: Some("550e8400-e29b-41d4-a716-446655440000".to_string()),
                extra_uniqueids: vec![
                    ("sonarr".to_string(), "42".to_string()),
                    ("radarr".to_string(), "7".to_string()),
                ],
                ..Default::default()
            },
            video: Some(VideoSidecar {
                status: Some("Continuing".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let xml = build_nfo_xml(&original).expect("Failed to build XML");
        let parsed = parse_nfo(&xml).expect("Failed to parse written XML");

        assert_eq!(
            parsed.identifiers.kobbler_id.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        assert_eq!(parsed.identifiers.tmdb_id.as_deref(), Some("99999"));
        assert_eq!(parsed.identifiers.extra_uniqueids.len(), 2);
        assert!(
            parsed
                .identifiers
                .extra_uniqueids
                .contains(&("sonarr".to_string(), "42".to_string()))
        );
        assert!(
            parsed
                .identifiers
                .extra_uniqueids
                .contains(&("radarr".to_string(), "7".to_string()))
        );
    }

    #[test]
    fn test_roundtrip_movie_nfo() {
        let original = SidecarMetadata {
            source_format: Some(SidecarFormat::Nfo),
            title: Some("Test Movie".to_string()),
            year: Some(2024),
            plot: Some("A test plot".to_string()),
            rating: Some(7.5),
            genres: vec!["Action".to_string(), "Comedy".to_string()],
            identifiers: SidecarIdentifiers {
                tmdb_id: Some("12345".to_string()),
                imdb_id: Some("tt0000001".to_string()),
                ..Default::default()
            },
            video: Some(VideoSidecar {
                mpaa: Some("PG-13".to_string()),
                studio: Some("Test Studio".to_string()),
                cast: vec![CastEntry {
                    name: "Actor One".to_string(),
                    role: Some("Hero".to_string()),
                    thumb: None,
                }],
                ..Default::default()
            }),
            ..Default::default()
        };

        let xml = build_nfo_xml(&original).expect("Failed to build XML");
        let parsed = parse_nfo(&xml).expect("Failed to parse written XML");

        assert_eq!(parsed.title, original.title);
        assert_eq!(parsed.year, original.year);
        assert_eq!(parsed.rating, original.rating);
        assert_eq!(parsed.genres, original.genres);
        assert_eq!(parsed.identifiers.tmdb_id, original.identifiers.tmdb_id);
        assert_eq!(parsed.identifiers.imdb_id, original.identifiers.imdb_id);
    }

    #[test]
    fn test_parse_nfo_sets_nfo_type() {
        let movie_nfo = include_str!("test_fixtures/movie.nfo");
        let movie = parse_nfo(movie_nfo).unwrap();
        assert_eq!(movie.video.unwrap().nfo_type, Some(NfoType::Movie));

        let tvshow_nfo = include_str!("test_fixtures/tvshow.nfo");
        let tvshow = parse_nfo(tvshow_nfo).unwrap();
        assert_eq!(tvshow.video.unwrap().nfo_type, Some(NfoType::TvShow));

        let ep_nfo = include_str!("test_fixtures/episode.nfo");
        let ep = parse_nfo(ep_nfo).unwrap();
        assert_eq!(ep.video.unwrap().nfo_type, Some(NfoType::Episode));
    }

    #[test]
    fn test_build_nfo_xml_tvshow_root() {
        let meta = SidecarMetadata {
            title: Some("Test Show".to_string()),
            video: Some(VideoSidecar {
                nfo_type: Some(NfoType::TvShow),
                status: Some("Returning Series".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let xml = build_nfo_xml(&meta).unwrap();
        assert!(xml.contains("<tvshow>"));
        assert!(xml.contains("</tvshow>"));
    }

    #[test]
    fn test_build_nfo_xml_episode_root() {
        let meta = SidecarMetadata {
            title: Some("Pilot".to_string()),
            video: Some(VideoSidecar {
                nfo_type: Some(NfoType::Episode),
                season: Some(1),
                episode: Some(1),
                ..Default::default()
            }),
            ..Default::default()
        };
        let xml = build_nfo_xml(&meta).unwrap();
        assert!(xml.contains("<episodedetails>"));
        assert!(xml.contains("</episodedetails>"));
    }

    #[test]
    fn test_explicit_nfo_type_overrides_heuristic() {
        // TvShow with no status — heuristic would say Movie, but explicit type wins
        let meta = SidecarMetadata {
            title: Some("Statusless Show".to_string()),
            video: Some(VideoSidecar {
                nfo_type: Some(NfoType::TvShow),
                ..Default::default()
            }),
            ..Default::default()
        };
        let xml = build_nfo_xml(&meta).unwrap();
        assert!(xml.contains("<tvshow>"));

        // Movie with status — heuristic would say TvShow, but explicit type wins
        let meta2 = SidecarMetadata {
            title: Some("Movie With Status".to_string()),
            video: Some(VideoSidecar {
                nfo_type: Some(NfoType::Movie),
                status: Some("Released".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let xml2 = build_nfo_xml(&meta2).unwrap();
        assert!(xml2.contains("<movie>"));
    }

    #[test]
    fn test_nfo_type_roundtrip() {
        for nfo_t in [NfoType::Movie, NfoType::TvShow, NfoType::Episode] {
            let meta = SidecarMetadata {
                title: Some("Roundtrip Test".to_string()),
                video: Some(VideoSidecar {
                    nfo_type: Some(nfo_t),
                    season: if nfo_t == NfoType::Episode {
                        Some(1)
                    } else {
                        None
                    },
                    episode: if nfo_t == NfoType::Episode {
                        Some(1)
                    } else {
                        None
                    },
                    status: if nfo_t == NfoType::TvShow {
                        Some("Ended".to_string())
                    } else {
                        None
                    },
                    ..Default::default()
                }),
                identifiers: SidecarIdentifiers {
                    tmdb_id: Some("123".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            };
            let xml = build_nfo_xml(&meta).unwrap();
            let parsed = parse_nfo(&xml).unwrap();
            assert_eq!(
                parsed.video.as_ref().unwrap().nfo_type,
                Some(nfo_t),
                "NfoType roundtrip failed for {nfo_t:?}"
            );
        }
    }

    #[test]
    fn test_infer_nfo_type_agrees_with_build_nfo_xml() {
        use super::super::infer_nfo_type;

        let cases = [
            (
                "movie",
                SidecarMetadata {
                    title: Some("M".to_string()),
                    video: Some(VideoSidecar::default()),
                    identifiers: SidecarIdentifiers {
                        tmdb_id: Some("1".to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                NfoType::Movie,
            ),
            (
                "tvshow",
                SidecarMetadata {
                    title: Some("S".to_string()),
                    video: Some(VideoSidecar {
                        status: Some("Returning".to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                NfoType::TvShow,
            ),
            (
                "episode",
                SidecarMetadata {
                    title: Some("E".to_string()),
                    video: Some(VideoSidecar {
                        season: Some(1),
                        episode: Some(1),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                NfoType::Episode,
            ),
        ];

        for (label, meta, expected) in &cases {
            assert_eq!(
                infer_nfo_type(meta),
                *expected,
                "infer_nfo_type wrong for {label}"
            );
            let xml = build_nfo_xml(meta).unwrap();
            assert!(
                xml.contains(&format!("<{}>", expected.root_tag())),
                "build_nfo_xml root tag wrong for {label}"
            );
        }
    }
}
