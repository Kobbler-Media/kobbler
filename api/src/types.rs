//! Common types used across the application.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

// =============================================================================
// Media Scope
// =============================================================================

/// Which kinds of media a library-wide maintenance pass should walk.
///
/// These fan-outs are not uniformly priced. The library holds roughly 600 movie
/// files against 10,000 episode files, and both the skip-detect re-run and the
/// media-analyze backfill run a credit-text classify per file — so a
/// forced run over everything is a full-library GPU pass, ~16× the cost of the
/// movie slice. When a change only moves one kind's detection, only that kind
/// should be queued.
///
/// `None` at a call site means "every kind this pass handles".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaScope {
    Movie,
    Episode,
    Audiobook,
}

impl MediaScope {
    /// Whether `scope` — absent meaning unrestricted — covers this kind.
    pub fn covered_by(self, scope: Option<Self>) -> bool {
        scope.is_none_or(|s| s == self)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Episode => "episode",
            Self::Audiobook => "audiobook",
        }
    }
}

impl fmt::Display for MediaScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// =============================================================================
// Device Types
// =============================================================================

/// Device type — determines sync behavior, preferred formats, and UI features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum DeviceType {
    /// Kobo e-reader (KEPUB format, proxy sync, collections, reading progress).
    #[default]
    Kobo,
    /// Web reader (EPUB format, browser-based reading).
    Web,
    /// Kindle device (EPUB with embedded metadata, Send-to-Kindle delivery).
    Kindle,
    /// Generic device (EPUB format, download only).
    Generic,
}

impl DeviceType {
    /// Preferred ebook format for this device type.
    pub fn preferred_format(&self) -> &'static str {
        match self {
            DeviceType::Kobo => "KEPUB",
            DeviceType::Kindle => "MOBI",
            DeviceType::Web | DeviceType::Generic => "EPUB",
        }
    }

    /// Whether this device type requires format conversion from EPUB.
    pub fn needs_conversion(&self) -> bool {
        matches!(self, DeviceType::Kobo)
    }

    /// Whether this device type uses the Kobo proxy for sync.
    pub fn uses_proxy(&self) -> bool {
        matches!(self, DeviceType::Kobo)
    }
}

impl fmt::Display for DeviceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeviceType::Kobo => write!(f, "kobo"),
            DeviceType::Web => write!(f, "web"),
            DeviceType::Kindle => write!(f, "kindle"),
            DeviceType::Generic => write!(f, "generic"),
        }
    }
}

impl FromStr for DeviceType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "kobo" => Ok(DeviceType::Kobo),
            "web" => Ok(DeviceType::Web),
            "kindle" => Ok(DeviceType::Kindle),
            "generic" => Ok(DeviceType::Generic),
            _ => Err(format!("invalid device type: {}", s)),
        }
    }
}

// =============================================================================
// Entitlement Types
// =============================================================================

/// Entitlement scope type - determines who can access a book.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[sqlx(type_name = "text", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EntitlementScope {
    /// Available to all users.
    #[default]
    AllUsers,
    /// Available to a specific user.
    User,
    /// Available to a specific device.
    Device,
}

impl fmt::Display for EntitlementScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EntitlementScope::AllUsers => write!(f, "ALL_USERS"),
            EntitlementScope::User => write!(f, "USER"),
            EntitlementScope::Device => write!(f, "DEVICE"),
        }
    }
}

impl FromStr for EntitlementScope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "ALL_USERS" => Ok(EntitlementScope::AllUsers),
            "USER" => Ok(EntitlementScope::User),
            "DEVICE" => Ok(EntitlementScope::Device),
            _ => Err(format!("invalid scope type: {}", s)),
        }
    }
}

// =============================================================================
// Want Status Types
// =============================================================================

/// Want status for a book - determines if/how a book should be acquired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum WantStatus {
    /// Not wanted (either owned or not interested).
    #[default]
    None,
    /// Discovered but not yet decided.
    Pending,
    /// Actively wanted - should be acquired.
    Wanted,
    /// Explicitly skipped - don't want this book.
    Skipped,
    /// Search request sent to Kobload, awaiting response.
    Searching,
    /// Kobload is actively downloading this book.
    Downloading,
    /// Kobload found no immediate result, watching RSS for new releases.
    Watching,
    /// Kobload search failed (no results, indexer errors, etc.).
    SearchFailed,
    /// Book was acquired through the want system and is now in the library.
    Acquired,
}

impl WantStatus {
    /// Returns all possible values.
    pub fn all() -> &'static [WantStatus] {
        &[
            WantStatus::None,
            WantStatus::Pending,
            WantStatus::Wanted,
            WantStatus::Skipped,
            WantStatus::Searching,
            WantStatus::Downloading,
            WantStatus::Watching,
            WantStatus::SearchFailed,
            WantStatus::Acquired,
        ]
    }
}

impl fmt::Display for WantStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WantStatus::None => write!(f, "none"),
            WantStatus::Pending => write!(f, "pending"),
            WantStatus::Wanted => write!(f, "wanted"),
            WantStatus::Skipped => write!(f, "skipped"),
            WantStatus::Searching => write!(f, "searching"),
            WantStatus::Downloading => write!(f, "downloading"),
            WantStatus::Watching => write!(f, "watching"),
            WantStatus::SearchFailed => write!(f, "search_failed"),
            WantStatus::Acquired => write!(f, "acquired"),
        }
    }
}

impl FromStr for WantStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" => Ok(WantStatus::None),
            "pending" => Ok(WantStatus::Pending),
            "wanted" => Ok(WantStatus::Wanted),
            "skipped" => Ok(WantStatus::Skipped),
            "searching" => Ok(WantStatus::Searching),
            "downloading" => Ok(WantStatus::Downloading),
            "watching" => Ok(WantStatus::Watching),
            "search_failed" => Ok(WantStatus::SearchFailed),
            "acquired" => Ok(WantStatus::Acquired),
            // Legacy mapping from old tracked_author_books.want_status
            "owned" => Ok(WantStatus::None),
            _ => Err(format!("invalid want status: {}", s)),
        }
    }
}

// =============================================================================
// Auto-Want Mode Types
// =============================================================================

/// Auto-want mode for tracked authors/series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum AutoWantMode {
    /// Inherit from parent (series inherits from author). Default for series.
    Inherit,
    /// Don't automatically want any books.
    #[default]
    None,
    /// Only want books released after tracking started.
    NewOnly,
    /// Want all books from this author/series.
    All,
    /// Series only: want books to fill gaps in the collection.
    FillGaps,
}

impl AutoWantMode {
    /// Returns all values valid for authors.
    pub fn author_modes() -> &'static [AutoWantMode] {
        &[AutoWantMode::None, AutoWantMode::NewOnly, AutoWantMode::All]
    }

    /// Returns all values valid for series (including inherit as default).
    pub fn series_modes() -> &'static [AutoWantMode] {
        &[
            AutoWantMode::Inherit,
            AutoWantMode::None,
            AutoWantMode::NewOnly,
            AutoWantMode::All,
            AutoWantMode::FillGaps,
        ]
    }
}

impl fmt::Display for AutoWantMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AutoWantMode::Inherit => write!(f, "inherit"),
            AutoWantMode::None => write!(f, "none"),
            AutoWantMode::NewOnly => write!(f, "new_only"),
            AutoWantMode::All => write!(f, "all"),
            AutoWantMode::FillGaps => write!(f, "fill_gaps"),
        }
    }
}

impl FromStr for AutoWantMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "inherit" => Ok(AutoWantMode::Inherit),
            "none" => Ok(AutoWantMode::None),
            "new_only" => Ok(AutoWantMode::NewOnly),
            "all" => Ok(AutoWantMode::All),
            "fill_gaps" => Ok(AutoWantMode::FillGaps),
            _ => Err(format!("invalid auto want mode: {}", s)),
        }
    }
}

// =============================================================================
// Download Attempt Status
// =============================================================================

/// Status of a download attempt (formerly from wanted_attempts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum AttemptStatus {
    /// Attempt created, not yet started.
    #[default]
    New,
    /// Download queued with download client.
    Queued,
    /// Currently downloading.
    Downloading,
    /// Error from Kobload/download client.
    KobloadError,
    /// Download complete, needs manual review.
    NeedsReview,
    /// Successfully completed and ingested.
    Completed,
    /// Failed permanently.
    Failed,
}

impl AttemptStatus {
    /// Returns true if this is a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, AttemptStatus::Completed | AttemptStatus::Failed)
    }

    /// Returns true if this attempt is still in progress.
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            AttemptStatus::New
                | AttemptStatus::Queued
                | AttemptStatus::Downloading
                | AttemptStatus::NeedsReview
        )
    }
}

impl fmt::Display for AttemptStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AttemptStatus::New => write!(f, "new"),
            AttemptStatus::Queued => write!(f, "queued"),
            AttemptStatus::Downloading => write!(f, "downloading"),
            AttemptStatus::KobloadError => write!(f, "kobload_error"),
            AttemptStatus::NeedsReview => write!(f, "needs_review"),
            AttemptStatus::Completed => write!(f, "completed"),
            AttemptStatus::Failed => write!(f, "failed"),
        }
    }
}

impl FromStr for AttemptStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "new" => Ok(AttemptStatus::New),
            "queued" => Ok(AttemptStatus::Queued),
            "downloading" => Ok(AttemptStatus::Downloading),
            "kobload_error" => Ok(AttemptStatus::KobloadError),
            "needs_review" => Ok(AttemptStatus::NeedsReview),
            "completed" => Ok(AttemptStatus::Completed),
            "failed" => Ok(AttemptStatus::Failed),
            _ => Err(format!("invalid attempt status: {}", s)),
        }
    }
}

#[cfg(test)]
mod media_scope_tests {
    use super::*;

    #[test]
    fn absent_scope_covers_every_kind() {
        assert!(MediaScope::Movie.covered_by(None));
        assert!(MediaScope::Episode.covered_by(None));
        assert!(MediaScope::Audiobook.covered_by(None));
    }

    #[test]
    fn a_scope_covers_only_itself() {
        let movies = Some(MediaScope::Movie);
        assert!(MediaScope::Movie.covered_by(movies));
        assert!(!MediaScope::Episode.covered_by(movies));
        assert!(!MediaScope::Audiobook.covered_by(movies));
    }

    #[test]
    fn deserializes_from_snake_case() {
        assert_eq!(
            serde_json::from_str::<MediaScope>("\"movie\"").unwrap(),
            MediaScope::Movie
        );
        assert_eq!(
            serde_json::from_str::<Option<MediaScope>>("null").unwrap(),
            None
        );
    }
}
