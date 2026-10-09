//! KobloadInterpreter - calls Kobload's interpret/stream NDJSON endpoint.

use super::{
    InterpretItem, InterpretResult, InterpreterError, MediaInterpreter, extract_year, strip_year,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// Interpreter that delegates to Kobload's HTTP API.
pub struct KobloadInterpreter {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl KobloadInterpreter {
    pub fn new(base_url: String, api_key: String) -> Self {
        let client = crate::http::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("Failed to create HTTP client");
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            client,
        }
    }
}

/// Request item matching Kobload's StreamInterpretItem.
#[derive(Serialize)]
struct StreamItem {
    path: String,
    media_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<i64>,
}

/// Response item matching Kobload's StreamInterpretResponse.
#[derive(Deserialize)]
struct StreamResponse {
    path: String,
    parsed: nats_jobs::ParsedRelease,
    #[serde(default)]
    error: Option<String>,
}

#[async_trait]
impl MediaInterpreter for KobloadInterpreter {
    async fn interpret_batch(
        &self,
        items: Vec<InterpretItem>,
    ) -> Result<Vec<InterpretResult>, InterpreterError> {
        if items.is_empty() {
            return Ok(Vec::new());
        }

        // Build NDJSON body
        let mut body = String::new();
        for item in &items {
            let stream_item = StreamItem {
                path: item.relative_path.clone(),
                media_kind: item.media_kind.clone(),
                size: item.file_size,
            };
            let line = serde_json::to_string(&stream_item)
                .map_err(|e| InterpreterError::Parse(e.to_string()))?;
            body.push_str(&line);
            body.push('\n');
        }

        debug!(
            items = items.len(),
            url = %self.base_url,
            "Sending interpret batch to Kobload"
        );

        let response = self
            .client
            .post(format!("{}/api/v1/interpret/stream", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/x-ndjson")
            .body(body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(InterpreterError::Other(format!(
                "Kobload returned {}: {}",
                status, text
            )));
        }

        let response_text = response.text().await?;

        // Parse NDJSON response
        let mut results = Vec::new();
        for line in response_text.lines() {
            if line.trim().is_empty() {
                continue;
            }

            let resp: StreamResponse = match serde_json::from_str(line) {
                Ok(r) => r,
                Err(e) => {
                    warn!("Failed to parse Kobload response line: {}", e);
                    continue;
                }
            };

            results.push(parse_stream_response(resp));
        }

        Ok(results)
    }
}

/// Convert a Kobload StreamResponse into our InterpretResult.
fn parse_stream_response(resp: StreamResponse) -> InterpretResult {
    if let Some(error) = resp.error {
        return InterpretResult {
            path: resp.path,
            success: false,
            error: Some(error),
            media_type: None,
            show_name: None,
            season: None,
            episodes: Vec::new(),
            episode_title: None,
            show_year: None,
            movie_title: None,
            movie_year: None,
            book_title: None,
            book_authors: vec![],
            raw: Some(resp.parsed),
            sidecar_ids: None,
        };
    }

    let parsed = resp.parsed;

    match &parsed.media {
        nats_jobs::ParsedMedia::Tv(tv) => {
            let raw_name = Some(tv.show_name.as_str());
            let show_year = tv.year.or_else(|| raw_name.and_then(extract_year));
            let show_name = raw_name.map(strip_year);
            InterpretResult {
                path: resp.path,
                success: true,
                error: None,
                media_type: Some("tv".to_string()),
                show_name,
                season: tv.season,
                episodes: tv.episodes.clone(),
                episode_title: tv.episode_title.clone(),
                show_year,
                movie_title: None,
                movie_year: None,
                book_title: None,
                book_authors: vec![],
                raw: Some(parsed),
                sidecar_ids: None,
            }
        }
        nats_jobs::ParsedMedia::Movie(movie) => {
            let raw_title = Some(movie.title.as_str());
            let movie_year = movie.year.or_else(|| raw_title.and_then(extract_year));
            let movie_title = raw_title.map(strip_year);
            InterpretResult {
                path: resp.path,
                success: true,
                error: None,
                media_type: Some("movie".to_string()),
                show_name: None,
                season: None,
                episodes: Vec::new(),
                episode_title: None,
                show_year: None,
                movie_title,
                movie_year,
                book_title: None,
                book_authors: vec![],
                raw: Some(parsed),
                sidecar_ids: None,
            }
        }
        nats_jobs::ParsedMedia::Sport(sport) => InterpretResult {
            path: resp.path,
            success: true,
            error: None,
            media_type: Some("sport".to_string()),
            show_name: Some(sport.league_name.clone()),
            season: sport.season,
            episodes: Vec::new(),
            episode_title: None,
            show_year: sport.year,
            movie_title: None,
            movie_year: None,
            book_title: None,
            book_authors: vec![],
            raw: Some(parsed),
            sidecar_ids: None,
        },
        nats_jobs::ParsedMedia::Book(book) => InterpretResult {
            path: resp.path,
            success: true,
            error: None,
            media_type: Some("book".to_string()),
            show_name: None,
            season: None,
            episodes: Vec::new(),
            episode_title: None,
            show_year: None,
            movie_title: None,
            movie_year: None,
            book_title: Some(book.title.clone()),
            book_authors: book.author.iter().cloned().collect(),
            raw: Some(parsed),
            sidecar_ids: None,
        },
        nats_jobs::ParsedMedia::Audiobook(ab) => InterpretResult {
            path: resp.path,
            success: true,
            error: None,
            media_type: Some("audiobook".to_string()),
            show_name: None,
            season: None,
            episodes: Vec::new(),
            episode_title: None,
            show_year: None,
            movie_title: None,
            movie_year: None,
            book_title: Some(ab.title.clone()),
            book_authors: ab.author.iter().cloned().collect(),
            raw: Some(parsed),
            sidecar_ids: None,
        },
        nats_jobs::ParsedMedia::Music(music) => InterpretResult {
            path: resp.path,
            success: true,
            error: None,
            media_type: Some("music".to_string()),
            show_name: None,
            season: None,
            episodes: Vec::new(),
            episode_title: None,
            show_year: None,
            movie_title: None,
            movie_year: None,
            book_title: music.album.clone(),
            book_authors: vec![music.artist.clone()],
            raw: Some(parsed),
            sidecar_ids: None,
        },
        _ => {
            // Game, Software, Xxx, Unknown — extract title
            let title = Some(parsed.title.clone());
            let media_type = match &parsed.media {
                nats_jobs::ParsedMedia::Game(_) => Some("game".to_string()),
                nats_jobs::ParsedMedia::Software(_) => Some("software".to_string()),
                nats_jobs::ParsedMedia::Xxx(_) => Some("xxx".to_string()),
                _ => None,
            };
            InterpretResult {
                path: resp.path,
                success: title.is_some(),
                error: if title.is_none() {
                    Some("Unknown media type".to_string())
                } else {
                    None
                },
                media_type,
                show_name: None,
                season: None,
                episodes: Vec::new(),
                episode_title: None,
                show_year: None,
                movie_title: None,
                movie_year: None,
                book_title: title,
                book_authors: vec![],
                raw: Some(parsed),
                sidecar_ids: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_parsed(title: &str, media: nats_jobs::ParsedMedia) -> nats_jobs::ParsedRelease {
        nats_jobs::ParsedRelease {
            title: title.to_string(),
            media,
            quality: Default::default(),
            flags: Default::default(),
            release_group: None,
            confidence: 0.95,
        }
    }

    #[test]
    fn test_parse_sport_response() {
        let resp = StreamResponse {
            path: "MotoGP.2025.R01.Thailand.Qualifying.1080p.mkv".to_string(),
            parsed: make_parsed(
                "MotoGP",
                nats_jobs::ParsedMedia::Sport(nats_jobs::SportParsed {
                    league_name: "MotoGP".to_string(),
                    season: Some(2025),
                    round: Some(1),
                    location: Some("Thailand".to_string()),
                    session_type: Some("Qualifying".to_string()),
                    year: Some(2025),
                    ..Default::default()
                }),
            ),
            error: None,
        };

        let result = parse_stream_response(resp);
        assert!(result.success);
        assert_eq!(result.media_type.as_deref(), Some("sport"));
        assert_eq!(result.show_name.as_deref(), Some("MotoGP"));
        assert_eq!(result.season, Some(2025));
        assert_eq!(result.show_year, Some(2025));
        assert!(result.raw.is_some());
        // Sport-specific fields should be in raw.media
        let raw = result.raw.unwrap();
        let sport = raw.media.as_sport().expect("should be sport");
        assert_eq!(sport.round, Some(1));
        assert_eq!(sport.location.as_deref(), Some("Thailand"));
        assert_eq!(sport.session_type.as_deref(), Some("Qualifying"));
    }

    #[test]
    fn test_parse_tv_response() {
        let resp = StreamResponse {
            path: "Show/Season 01/S01E05.mkv".to_string(),
            parsed: make_parsed(
                "Show",
                nats_jobs::ParsedMedia::Tv(nats_jobs::TvParsed {
                    show_name: "Show".to_string(),
                    season: Some(1),
                    episodes: vec![5],
                    year: Some(2020),
                    ..Default::default()
                }),
            ),
            error: None,
        };

        let result = parse_stream_response(resp);
        assert!(result.success);
        assert_eq!(result.media_type.as_deref(), Some("tv"));
        assert_eq!(result.show_name.as_deref(), Some("Show"));
        assert_eq!(result.season, Some(1));
        assert_eq!(result.episodes, vec![5]);
    }

    #[test]
    fn test_parse_tv_response_strips_year_from_show_name() {
        let resp = StreamResponse {
            path: "Bluey (2018)/Season 01/S01E05.mkv".to_string(),
            parsed: make_parsed(
                "Bluey (2018)",
                nats_jobs::ParsedMedia::Tv(nats_jobs::TvParsed {
                    show_name: "Bluey (2018)".to_string(),
                    season: Some(1),
                    episodes: vec![5],
                    year: Some(2018),
                    ..Default::default()
                }),
            ),
            error: None,
        };

        let result = parse_stream_response(resp);
        assert!(result.success);
        assert_eq!(result.show_name.as_deref(), Some("Bluey"));
        assert_eq!(result.show_year, Some(2018));
    }

    #[test]
    fn test_parse_tv_response_extracts_year_from_name_when_missing() {
        let resp = StreamResponse {
            path: "Bluey (2018)/Season 01/S01E05.mkv".to_string(),
            parsed: make_parsed(
                "Bluey (2018)",
                nats_jobs::ParsedMedia::Tv(nats_jobs::TvParsed {
                    show_name: "Bluey (2018)".to_string(),
                    season: Some(1),
                    episodes: vec![5],
                    ..Default::default()
                }),
            ),
            error: None,
        };

        let result = parse_stream_response(resp);
        assert_eq!(result.show_name.as_deref(), Some("Bluey"));
        assert_eq!(result.show_year, Some(2018));
    }

    #[test]
    fn test_parse_audiobook_response_singular_author() {
        let resp = StreamResponse {
            path: "Brandon Sanderson - Mistborn/01.mp3".to_string(),
            parsed: make_parsed(
                "Mistborn",
                nats_jobs::ParsedMedia::Audiobook(nats_jobs::AudiobookParsed {
                    title: "Mistborn".to_string(),
                    author: Some("Brandon Sanderson".to_string()),
                    ..Default::default()
                }),
            ),
            error: None,
        };

        let result = parse_stream_response(resp);
        assert!(result.success);
        assert_eq!(result.media_type.as_deref(), Some("audiobook"));
        assert_eq!(result.book_title.as_deref(), Some("Mistborn"));
        assert_eq!(result.book_authors, vec!["Brandon Sanderson"]);
    }

    #[test]
    fn test_parse_audiobook_response_no_author() {
        let resp = StreamResponse {
            path: "Some Book/track01.mp3".to_string(),
            parsed: make_parsed(
                "track01",
                nats_jobs::ParsedMedia::Audiobook(nats_jobs::AudiobookParsed {
                    title: "track01".to_string(),
                    ..Default::default()
                }),
            ),
            error: None,
        };

        let result = parse_stream_response(resp);
        assert!(result.success);
        assert_eq!(result.book_title.as_deref(), Some("track01"));
        assert!(result.book_authors.is_empty());
    }
}
