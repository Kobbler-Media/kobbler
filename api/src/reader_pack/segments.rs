//! The book's text as read-along segments: one per text block, each located
//! by the epubcfi readers resume from, in reading order.

use kob_ebook::reader_pack::BlockKind;

use super::store::{LoadedPack, PackError};

pub struct TextSegment {
    pub index: i32,
    pub cfi: String,
    pub text: String,
    pub chapter_title: Option<String>,
}

pub fn text_segments(pack: &LoadedPack) -> Result<Vec<TextSegment>, PackError> {
    let mut out = Vec::new();
    for (n, sidecar) in pack.sidecars.iter().enumerate() {
        let chapter = pack.chapter(n)?;
        let mut title = pack.titles.get(n).cloned().flatten();
        for (b, block) in chapter.blocks.iter().enumerate() {
            if !matches!(block.kind, BlockKind::Text | BlockKind::Heading) {
                continue;
            }
            let text = block.text();
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }
            if block.kind == BlockKind::Heading && title.is_none() {
                title = Some(trimmed.to_string());
            }
            let leading = text[..text.len() - text.trim_start().len()]
                .encode_utf16()
                .count() as u32;
            let Some(cfi) = sidecar.cfi_at(b as u32, leading) else {
                continue;
            };
            out.push(TextSegment {
                index: out.len() as i32,
                cfi,
                text: trimmed.to_string(),
                chapter_title: title.clone(),
            });
        }
    }
    Ok(out)
}
