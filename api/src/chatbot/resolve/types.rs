use serde::{Deserialize, Serialize};

use super::refs::{MediaKind, MediaRef};

/// The `media_type` argument of `resolve_media`. One enum across books and
/// video on purpose: the model must never learn that they are different
/// subsystems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaTypeHint {
    Book,
    Audiobook,
    Movie,
    Show,
    Season,
    Episode,
    Author,
    Series,
    Sports,
}

impl MediaTypeHint {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "book" => MediaTypeHint::Book,
            "audiobook" => MediaTypeHint::Audiobook,
            "movie" | "film" => MediaTypeHint::Movie,
            "show" | "tv" | "tv_show" => MediaTypeHint::Show,
            "season" => MediaTypeHint::Season,
            "episode" => MediaTypeHint::Episode,
            "author" => MediaTypeHint::Author,
            "series" => MediaTypeHint::Series,
            "sports" | "sport" => MediaTypeHint::Sports,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            MediaTypeHint::Book => "book",
            MediaTypeHint::Audiobook => "audiobook",
            MediaTypeHint::Movie => "movie",
            MediaTypeHint::Show => "show",
            MediaTypeHint::Season => "season",
            MediaTypeHint::Episode => "episode",
            MediaTypeHint::Author => "author",
            MediaTypeHint::Series => "series",
            MediaTypeHint::Sports => "sports",
        }
    }

    /// Books and audiobooks live in one metadata stack, everything else in the
    /// other. This is the only place the two stacks are told apart.
    pub fn is_books_side(self) -> bool {
        matches!(
            self,
            MediaTypeHint::Book | MediaTypeHint::Audiobook | MediaTypeHint::Author
        )
    }

    /// Whether a candidate of this kind satisfies the hint.
    pub fn accepts(self, kind: MediaKind) -> bool {
        match self {
            MediaTypeHint::Book | MediaTypeHint::Audiobook => kind == MediaKind::Book,
            MediaTypeHint::Movie => kind == MediaKind::Movie,
            MediaTypeHint::Show => kind == MediaKind::Show,
            MediaTypeHint::Season => kind == MediaKind::Season,
            MediaTypeHint::Episode => {
                matches!(kind, MediaKind::Episode | MediaKind::SportsEvent)
            }
            MediaTypeHint::Author => kind == MediaKind::Author,
            MediaTypeHint::Series => kind == MediaKind::Series,
            MediaTypeHint::Sports => {
                matches!(
                    kind,
                    MediaKind::SportsEvent | MediaKind::Episode | MediaKind::Show
                )
            }
        }
    }
}

/// `wanted.want_format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WantFormat {
    Ebook,
    Audiobook,
    Both,
}

impl WantFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            WantFormat::Ebook => "ebook",
            WantFormat::Audiobook => "audiobook",
            WantFormat::Both => "both",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "ebook" => WantFormat::Ebook,
            "audiobook" => WantFormat::Audiobook,
            "both" => WantFormat::Both,
            _ => return None,
        })
    }
}

/// The one entry point's arguments.
#[derive(Debug, Clone, Default)]
pub struct ResolveRequest {
    pub query: String,
    pub media_type: Option<MediaTypeHint>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub author: Option<String>,
    pub format: Option<WantFormat>,
    /// What the previous turn resolved to, so "the next one" has a referent.
    pub context: Option<MediaRef>,
    /// Subtract the caller's watched set before ranking.
    pub exclude_watched: bool,
}

impl ResolveRequest {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            ..Default::default()
        }
    }

    pub fn with_media_type(mut self, hint: MediaTypeHint) -> Self {
        self.media_type = Some(hint);
        self
    }

    pub fn with_context(mut self, context: MediaRef) -> Self {
        self.context = Some(context);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateSource {
    Library,
    Provider(String),
}

/// Enough of a provider record for `want_it` to create the row without a
/// second round trip to the provider.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderPayload {
    pub provider: String,
    pub provider_id: String,
    pub title: String,
    pub year: Option<i32>,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub poster_url: Option<String>,
    pub network: Option<String>,
    pub genres: Vec<String>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub identifiers: serde_json::Value,
}

/// One season of a provider show, attached to a single resolution so the
/// model can answer "which seasons exist?" from what the tool said instead of
/// from memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShowSeason {
    pub season: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episodes: Option<u32>,
    /// Air date, `YYYY-MM-DD`. A future date means announced, not out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    #[serde(rename = "ref", with = "media_ref_string")]
    pub media_ref: MediaRef,
    pub title: String,
    /// One line of disambiguation: the year, the episode code, the author.
    pub subtitle: Option<String>,
    pub year: Option<i32>,
    pub description: Option<String>,
    pub in_library: bool,
    /// The show an episode or season belongs to. `want_it` routes a fixture to
    /// the sports endpoints and a TV episode to the TV ones.
    pub show_id: Option<uuid::Uuid>,
    /// True when the thing is a sports fixture rather than a TV episode. Same
    /// rails, different label.
    pub is_sports: bool,
    pub score: f64,
    pub source: CandidateSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<ProviderPayload>,
}

impl Candidate {
    pub fn kind(&self) -> MediaKind {
        self.media_ref.kind()
    }
}

mod media_ref_string {
    use std::str::FromStr;

    use serde::{Deserialize, Deserializer, Serializer};

    use super::MediaRef;

    pub fn serialize<S: Serializer>(v: &MediaRef, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<MediaRef, D::Error> {
        let raw = String::deserialize(d)?;
        MediaRef::from_str(&raw).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    /// One candidate, confidently.
    Single,
    /// Two or more plausible candidates. Ask; never pick.
    Ambiguous,
    /// A relative phrase ("the new one") with nothing to be relative to.
    NeedsContext,
    /// Not in the library and not at any provider.
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidates {
    pub resolution: Resolution,
    /// Ranked, best first, truncated. A wide list drowns a 12B model.
    pub candidates: Vec<Candidate>,
    /// The question to put to the user when the resolution is not `Single`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// Resolved book format, when the answer is a book.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<WantFormat>,
    /// Set when the requester's history is genuinely split and the format has
    /// to be asked rather than guessed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_question: Option<String>,
    /// Anything the answer should be hedged with — an unreliable air date, a
    /// fallback that was taken.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// What a tap on one of these candidates would otherwise throw away.
    ///
    /// A pick replies with the candidate's title, because a tap has to mean the
    /// sentence she would have typed. A title on its own is not that sentence
    /// when the question was about season 3: the reply resolves back to the same
    /// two shows and asks again, forever. The number rides along so the tap is
    /// the whole request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pick_qualifier: Option<String>,
    /// The seasons a single provider show has, when the provider can say.
    ///
    /// This list is all that exists: a season absent from it is not out, and
    /// the model must not predict one into being.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seasons: Option<Vec<ShowSeason>>,
}

impl Candidates {
    pub fn empty(resolution: Resolution) -> Self {
        Self {
            resolution,
            candidates: Vec::new(),
            question: None,
            format: None,
            format_question: None,
            note: None,
            pick_qualifier: None,
            seasons: None,
        }
    }

    pub fn not_found(note: impl Into<String>) -> Self {
        Self {
            note: Some(note.into()),
            ..Self::empty(Resolution::NotFound)
        }
    }

    pub fn needs_context(question: impl Into<String>) -> Self {
        Self {
            question: Some(question.into()),
            ..Self::empty(Resolution::NeedsContext)
        }
    }

    /// The resolved ref, and only when there is exactly one.
    pub fn single(&self) -> Option<&Candidate> {
        match self.resolution {
            Resolution::Single => self.candidates.first(),
            _ => None,
        }
    }

    pub fn is_ambiguous(&self) -> bool {
        self.resolution == Resolution::Ambiguous
    }

    pub fn refs(&self) -> Vec<MediaRef> {
        self.candidates
            .iter()
            .map(|c| c.media_ref.clone())
            .collect()
    }
}

/// Per-user format history. Counted, not guessed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FormatSkew {
    pub ebook: i64,
    pub audiobook: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("search index error: {0}")]
    Index(String),
    #[error("store error: {0}")]
    Store(String),
}

impl From<sqlx::Error> for ResolveError {
    fn from(e: sqlx::Error) -> Self {
        ResolveError::Store(e.to_string())
    }
}

impl From<crate::search::SearchError> for ResolveError {
    fn from(e: crate::search::SearchError) -> Self {
        ResolveError::Index(e.to_string())
    }
}
