use std::path::Path;
use std::time::UNIX_EPOCH;

use bytes::Bytes;
use uuid::Uuid;

use super::VirtualMp4;

const MAGIC: &[u8; 8] = b"KVMP4\x01\0\0";
const FIELDS: usize = 4;
const PREFIX: usize = MAGIC.len() + FIELDS * 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceStamp {
    size: u64,
    mtime_ns: u64,
}

pub async fn stamp(source: &Path) -> std::io::Result<SourceStamp> {
    let meta = tokio::fs::metadata(source).await?;
    let mtime_ns = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    Ok(SourceStamp {
        size: meta.len(),
        mtime_ns,
    })
}

pub async fn load(index: &Path, current: SourceStamp) -> Option<VirtualMp4> {
    let bytes = tokio::fs::read(index).await.ok()?;
    decode(bytes)
        .filter(|(_, stamp)| *stamp == current)
        .map(|(mp4, _)| mp4)
}

pub async fn save(index: &Path, mp4: &VirtualMp4, stamp: SourceStamp) -> std::io::Result<()> {
    if let Some(dir) = index.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let mut bytes = Vec::with_capacity(PREFIX + mp4.header.len());
    bytes.extend_from_slice(MAGIC);
    for v in [mp4.source_start, mp4.source_len, stamp.size, stamp.mtime_ns] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes.extend_from_slice(&mp4.header);
    let partial = index.with_extension(format!("{}.part", Uuid::now_v7()));
    if let Err(e) = tokio::fs::write(&partial, bytes).await {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(e);
    }
    tokio::fs::rename(&partial, index).await
}

fn decode(bytes: Vec<u8>) -> Option<(VirtualMp4, SourceStamp)> {
    if bytes.len() < PREFIX || &bytes[..MAGIC.len()] != MAGIC {
        return None;
    }
    let field = |i: usize| {
        let at = MAGIC.len() + i * 8;
        u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
    };
    let (source_start, source_len) = (field(0), field(1));
    let stamp = SourceStamp {
        size: field(2),
        mtime_ns: field(3),
    };
    let header = Bytes::from(bytes).slice(PREFIX..);
    Some((
        VirtualMp4 {
            header,
            source_start,
            source_len,
        },
        stamp,
    ))
}
