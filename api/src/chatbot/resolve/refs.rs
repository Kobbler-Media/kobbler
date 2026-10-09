use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What a ref points at, and therefore which acquisition endpoint `want_it`
/// must call. The model never chooses this — the resolver stamps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Book,
    Movie,
    Show,
    Season,
    Episode,
    Author,
    Series,
    SportsEvent,
}

impl MediaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaKind::Book => "book",
            MediaKind::Movie => "movie",
            MediaKind::Show => "show",
            MediaKind::Season => "season",
            MediaKind::Episode => "episode",
            MediaKind::Author => "author",
            MediaKind::Series => "series",
            MediaKind::SportsEvent => "sports_event",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "book" => MediaKind::Book,
            "movie" => MediaKind::Movie,
            "show" => MediaKind::Show,
            "season" => MediaKind::Season,
            "episode" => MediaKind::Episode,
            "author" => MediaKind::Author,
            "series" => MediaKind::Series,
            "sports_event" => MediaKind::SportsEvent,
            _ => return None,
        })
    }

    pub fn is_video(self) -> bool {
        matches!(
            self,
            MediaKind::Movie
                | MediaKind::Show
                | MediaKind::Season
                | MediaKind::Episode
                | MediaKind::SportsEvent
        )
    }
}

impl fmt::Display for MediaKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which slice of a provider work a ref names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderScope {
    /// The work as a whole — a book, a film, a show.
    Whole,
    /// Every season of a show we do not have.
    All,
    Season(u32),
    Episode {
        season: u32,
        episode: u32,
    },
}

/// A thing that is not in the library yet, addressed at the provider that knows
/// about it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderRef {
    pub provider: String,
    pub id: String,
    pub kind: MediaKind,
    pub scope: ProviderScope,
}

/// The contract between `resolve_media` and every acquisition tool.
///
/// Textual forms:
/// ```text
/// book:<uuid>          movie:<uuid>    episode:<uuid>   show:<uuid>
/// season:<uuid>        author:<uuid>   series:<uuid>    sports_event:<uuid>
/// provider:hardcover:<id>:book
/// provider:tmdb:<id>:movie
/// provider:tmdb:<id>:show
/// provider:tmdb:<id>:all            (every season of a show we do not have)
/// provider:tmdb:<id>:s24            (a season of a show we do not have)
/// provider:tmdb:<id>:s24e07         (an episode of a show we do not have)
/// ```
/// The trailing kind segment is an extension over the shape sketched in the
/// design doc: `provider:tmdb:<id>` alone cannot say whether it is a film or a
/// show, and `want_it` has to route on exactly that. Parsing accepts the
/// shorter documented forms and infers the kind from the provider.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MediaRef {
    Local { kind: MediaKind, id: Uuid },
    Provider(ProviderRef),
}

const BOOK_PROVIDERS: &[&str] = &[
    "hardcover",
    "googlebooks",
    "google_books",
    "openlibrary",
    "audnexus",
];

impl MediaRef {
    pub fn local(kind: MediaKind, id: Uuid) -> Self {
        MediaRef::Local { kind, id }
    }

    pub fn book(id: Uuid) -> Self {
        MediaRef::local(MediaKind::Book, id)
    }
    pub fn movie(id: Uuid) -> Self {
        MediaRef::local(MediaKind::Movie, id)
    }
    pub fn show(id: Uuid) -> Self {
        MediaRef::local(MediaKind::Show, id)
    }
    pub fn season(id: Uuid) -> Self {
        MediaRef::local(MediaKind::Season, id)
    }
    pub fn episode(id: Uuid) -> Self {
        MediaRef::local(MediaKind::Episode, id)
    }
    pub fn author(id: Uuid) -> Self {
        MediaRef::local(MediaKind::Author, id)
    }
    pub fn series(id: Uuid) -> Self {
        MediaRef::local(MediaKind::Series, id)
    }
    pub fn sports_event(id: Uuid) -> Self {
        MediaRef::local(MediaKind::SportsEvent, id)
    }

    pub fn provider(provider: impl Into<String>, id: impl Into<String>, kind: MediaKind) -> Self {
        MediaRef::Provider(ProviderRef {
            provider: provider.into(),
            id: id.into(),
            kind,
            scope: ProviderScope::Whole,
        })
    }

    pub fn provider_all(provider: impl Into<String>, id: impl Into<String>) -> Self {
        MediaRef::Provider(ProviderRef {
            provider: provider.into(),
            id: id.into(),
            kind: MediaKind::Show,
            scope: ProviderScope::All,
        })
    }

    pub fn provider_season(
        provider: impl Into<String>,
        id: impl Into<String>,
        season: u32,
    ) -> Self {
        MediaRef::Provider(ProviderRef {
            provider: provider.into(),
            id: id.into(),
            kind: MediaKind::Season,
            scope: ProviderScope::Season(season),
        })
    }

    pub fn provider_episode(
        provider: impl Into<String>,
        id: impl Into<String>,
        season: u32,
        episode: u32,
    ) -> Self {
        MediaRef::Provider(ProviderRef {
            provider: provider.into(),
            id: id.into(),
            kind: MediaKind::Episode,
            scope: ProviderScope::Episode { season, episode },
        })
    }

    pub fn kind(&self) -> MediaKind {
        match self {
            MediaRef::Local { kind, .. } => *kind,
            MediaRef::Provider(p) => p.kind,
        }
    }

    /// The library UUID, when this ref names a row we already have.
    pub fn local_id(&self) -> Option<Uuid> {
        match self {
            MediaRef::Local { id, .. } => Some(*id),
            MediaRef::Provider(_) => None,
        }
    }

    pub fn provider_ref(&self) -> Option<&ProviderRef> {
        match self {
            MediaRef::Provider(p) => Some(p),
            MediaRef::Local { .. } => None,
        }
    }

    pub fn in_library(&self) -> bool {
        matches!(self, MediaRef::Local { .. })
    }
}

impl fmt::Display for MediaRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MediaRef::Local { kind, id } => write!(f, "{}:{}", kind.as_str(), id),
            MediaRef::Provider(p) => {
                write!(f, "provider:{}:{}:", p.provider, p.id)?;
                match p.scope {
                    ProviderScope::Whole => f.write_str(p.kind.as_str()),
                    ProviderScope::All => f.write_str("all"),
                    ProviderScope::Season(s) => write!(f, "s{s:02}"),
                    ProviderScope::Episode { season, episode } => {
                        write!(f, "s{season:02}e{episode:02}")
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MediaRefParseError {
    #[error("empty ref")]
    Empty,
    #[error("unknown media kind: {0}")]
    UnknownKind(String),
    #[error("not a uuid: {0}")]
    BadUuid(String),
    #[error("malformed ref: {0}")]
    Malformed(String),
}

fn parse_scope_suffix(suffix: &str) -> Option<(MediaKind, ProviderScope)> {
    if let Some(kind) = MediaKind::parse(suffix) {
        return Some((kind, ProviderScope::Whole));
    }
    // "all" is a show shape, not a kind: every season of a show we do not have.
    if suffix == "all" {
        return Some((MediaKind::Show, ProviderScope::All));
    }
    let rest = suffix.strip_prefix('s')?;
    match rest.split_once('e') {
        Some((s, e)) => {
            let season: u32 = s.parse().ok()?;
            let episode: u32 = e.parse().ok()?;
            Some((
                MediaKind::Episode,
                ProviderScope::Episode { season, episode },
            ))
        }
        None => {
            let season: u32 = rest.parse().ok()?;
            Some((MediaKind::Season, ProviderScope::Season(season)))
        }
    }
}

impl FromStr for MediaRef {
    type Err = MediaRefParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(MediaRefParseError::Empty);
        }
        let parts: Vec<&str> = s.split(':').collect();
        if parts[0] == "provider" {
            let (provider, id) = match parts.len() {
                3 | 4 => (parts[1], parts[2]),
                _ => return Err(MediaRefParseError::Malformed(s.to_string())),
            };
            if provider.is_empty() || id.is_empty() {
                return Err(MediaRefParseError::Malformed(s.to_string()));
            }
            let (kind, scope) = match parts.get(3) {
                Some(suffix) => parse_scope_suffix(suffix)
                    .ok_or_else(|| MediaRefParseError::Malformed(s.to_string()))?,
                None if BOOK_PROVIDERS.contains(&provider) => {
                    (MediaKind::Book, ProviderScope::Whole)
                }
                None => (MediaKind::Show, ProviderScope::Whole),
            };
            return Ok(MediaRef::Provider(ProviderRef {
                provider: provider.to_string(),
                id: id.to_string(),
                kind,
                scope,
            }));
        }

        if parts.len() != 2 {
            return Err(MediaRefParseError::Malformed(s.to_string()));
        }
        let kind = MediaKind::parse(parts[0])
            .ok_or_else(|| MediaRefParseError::UnknownKind(parts[0].to_string()))?;
        let id = Uuid::parse_str(parts[1])
            .map_err(|_| MediaRefParseError::BadUuid(parts[1].to_string()))?;
        Ok(MediaRef::Local { kind, id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(s: &str) {
        let parsed: MediaRef = s.parse().expect(s);
        assert_eq!(parsed.to_string(), s, "round trip of {s}");
        let reparsed: MediaRef = parsed.to_string().parse().unwrap();
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn local_refs_round_trip() {
        let id = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";
        for kind in [
            "book",
            "movie",
            "show",
            "season",
            "episode",
            "author",
            "series",
            "sports_event",
        ] {
            round_trip(&format!("{kind}:{id}"));
        }
    }

    #[test]
    fn provider_refs_round_trip() {
        for s in [
            "provider:hardcover:99001:book",
            "provider:tmdb:95396:show",
            "provider:tmdb:95396:all",
            "provider:tmdb:549509:movie",
            "provider:tmdb:95396:s03",
            "provider:tmdb:95396:s03e04",
            "provider:tmdb:1396:s24e07",
            "provider:thesportsdb:4370:sports_event",
        ] {
            round_trip(s);
        }
        let all: MediaRef = "provider:tmdb:95396:all".parse().unwrap();
        assert_eq!(all.kind(), MediaKind::Show);
        assert_eq!(all.provider_ref().unwrap().scope, ProviderScope::All);
    }

    #[test]
    fn documented_short_provider_forms_parse() {
        let book: MediaRef = "provider:hardcover:12345".parse().unwrap();
        assert_eq!(book.kind(), MediaKind::Book);
        assert_eq!(book.to_string(), "provider:hardcover:12345:book");

        let show: MediaRef = "provider:tmdb:1396".parse().unwrap();
        assert_eq!(show.kind(), MediaKind::Show);

        let ep: MediaRef = "provider:tmdb:1396:s24e07".parse().unwrap();
        assert_eq!(ep.kind(), MediaKind::Episode);
        assert_eq!(
            ep.provider_ref().unwrap().scope,
            ProviderScope::Episode {
                season: 24,
                episode: 7
            }
        );
    }

    #[test]
    fn season_and_episode_numbers_are_padded_to_two_digits() {
        assert_eq!(
            MediaRef::provider_episode("tmdb", "95396", 3, 4).to_string(),
            "provider:tmdb:95396:s03e04"
        );
        assert_eq!(
            MediaRef::provider_season("tmdb", "95396", 3).to_string(),
            "provider:tmdb:95396:s03"
        );
        assert_eq!(
            MediaRef::provider_all("tmdb", "95396").to_string(),
            "provider:tmdb:95396:all"
        );
        // Three-digit seasons still parse back to the same value.
        let long = MediaRef::provider_episode("tmdb", "1", 120, 300);
        assert_eq!(long.to_string().parse::<MediaRef>().unwrap(), long);
    }

    #[test]
    fn rejects_garbage() {
        assert!("".parse::<MediaRef>().is_err());
        assert!("nonsense".parse::<MediaRef>().is_err());
        assert!("book:not-a-uuid".parse::<MediaRef>().is_err());
        assert!(
            "widget:3f2504e0-4f89-41d3-9a0c-0305e82c3301"
                .parse::<MediaRef>()
                .is_err()
        );
        assert!("provider:tmdb".parse::<MediaRef>().is_err());
        assert!("provider:tmdb:1:s0x".parse::<MediaRef>().is_err());
        assert!("provider:tmdb:1:show:extra".parse::<MediaRef>().is_err());
    }

    #[test]
    fn in_library_is_carried_by_the_ref() {
        let local: MediaRef = "movie:3f2504e0-4f89-41d3-9a0c-0305e82c3301"
            .parse()
            .unwrap();
        assert!(local.in_library());
        assert!(local.local_id().is_some());
        let remote: MediaRef = "provider:tmdb:1:movie".parse().unwrap();
        assert!(!remote.in_library());
        assert!(remote.local_id().is_none());
    }
}
