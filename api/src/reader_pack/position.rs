//! One reading position, three dialects.
//!
//! The app speaks pack [`Position`]s, foliate (web) speaks epubcfi, a Kobo
//! speaks `KoboSpan` + document. Progress is stored as epubcfi and every
//! other form is derived from it through the pack's sidecars, so all three
//! clients land on the same sentence.

use kob_ebook::reader_pack::{self, Position};

use super::store::LoadedPack;

pub fn to_cfi(pack: &LoadedPack, position: &Position) -> Option<String> {
    pack.sidecars
        .get(position.chapter as usize)?
        .cfi_at(position.block, position.offset)
}

pub fn from_cfi(pack: &LoadedPack, cfi: &str) -> Option<Position> {
    reader_pack::resolve_cfi(&pack.sidecars, cfi)
}

/// Reading order: earlier in the book sorts first.
pub fn order(position: &Position) -> (u32, u32, u32) {
    (position.chapter, position.block, position.offset)
}

pub struct KoboLocation {
    pub source: String,
    pub span: String,
}

pub fn to_kobo(pack: &LoadedPack, position: &Position) -> Option<KoboLocation> {
    let chapter = position.chapter as usize;
    let span = pack
        .sidecars
        .get(chapter)?
        .kobo_span_at(position.block, position.offset)?;
    Some(KoboLocation {
        source: pack.sidecars.get(chapter)?.source.clone(),
        span,
    })
}

pub fn from_kobo(pack: &LoadedPack, source: &str, span: &str) -> Option<Position> {
    let chapter = chapter_for_source(&pack.hrefs, source)?;
    let (block, offset) = pack.sidecars.get(chapter)?.position_of_kobo_span(span)?;
    Some(Position {
        chapter: chapter as u32,
        block,
        offset,
    })
}

/// A Kobo names the document as the OPF does, which is not always the full
/// zip entry path; zip entry names are unique, so a suffix match on a path
/// boundary cannot pick the wrong one.
fn chapter_for_source(hrefs: &[String], source: &str) -> Option<usize> {
    let wanted = source.trim_start_matches('/');
    hrefs
        .iter()
        .position(|h| h == wanted || h.ends_with(&format!("/{wanted}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kobo_sources_match_on_path_boundaries() {
        let hrefs = vec![
            "OEBPS/text/part0001.html".into(),
            "OEBPS/text/art0001.html".into(),
        ];
        assert_eq!(chapter_for_source(&hrefs, "text/part0001.html"), Some(0));
        assert_eq!(chapter_for_source(&hrefs, "art0001.html"), Some(1));
        assert_eq!(chapter_for_source(&hrefs, "rt0001.html"), None);
        assert_eq!(
            chapter_for_source(&hrefs, "OEBPS/text/part0001.html"),
            Some(0)
        );
    }
}
