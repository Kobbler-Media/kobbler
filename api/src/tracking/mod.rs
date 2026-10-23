//! Author tracking module for monitoring authors and auto-wanting their books.

pub mod cached;
pub mod hardcover;
pub mod reconcile;

pub use cached::CachedAuthorProvider;
pub use hardcover::{
    AuthorBook, AuthorSearchResult, AuthorWithBooks, HardcoverAuthorProvider, HardcoverEntity,
    SeriesBook, SeriesWithBooks, TrackedAuthorProvider,
};

/// Create a `CachedAuthorProvider` for the given provider type.
/// Currently only "hardcover" is supported.
pub fn create_tracking_provider(
    provider_id: &str,
    api_key: String,
    pool: sqlx::PgPool,
) -> Option<CachedAuthorProvider> {
    match provider_id {
        "hardcover" => Some(CachedAuthorProvider::new(
            Box::new(HardcoverAuthorProvider::new(api_key)),
            pool,
        )),
        _ => None,
    }
}
