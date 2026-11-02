//! Reader packs: each book pre-digested once into styled, flattened blocks
//! for the app (built by `kob_ebook::reader_pack`), plus the server-only
//! sidecars that translate between pack positions, epubcfi and KoboSpan.

pub mod file;
pub mod position;
pub mod segments;
mod store;

use std::sync::Arc;

use uuid::Uuid;

use crate::AppState;
pub use file::ReaderFile;
pub use store::{LoadedPack, PackError, ensure, is_built, remove_orphans};

/// The book's pack, converting and building on demand; None when it has
/// nothing readable or the build fails.
pub async fn for_book(state: &AppState, book_id: Uuid) -> Option<Arc<LoadedPack>> {
    let file = file::select(state, book_id, file::PACK_FORMATS).await?;
    match ensure(&state.config.storage_root, &file).await {
        Ok(pack) => Some(pack),
        Err(e) => {
            tracing::warn!(error = %e, book_id = %book_id, "reader pack unavailable");
            None
        }
    }
}
