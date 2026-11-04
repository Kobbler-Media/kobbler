use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures::StreamExt;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
use tracing::error;

/// An inclusive byte range already clamped to the file it addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

impl ByteRange {
    pub fn length(&self) -> u64 {
        self.end - self.start + 1
    }

    pub fn content_range(&self, file_size: u64) -> String {
        format!("bytes {}-{}/{}", self.start, self.end, file_size)
    }
}

/// Parse a single-range `Range` header value against a file of `file_size` bytes.
pub fn parse_byte_range(header_val: &str, file_size: u64) -> Option<ByteRange> {
    // No byte of an empty file is satisfiable, and every branch below indexes the
    // last one.
    if file_size == 0 {
        return None;
    }
    let range = header_val.strip_prefix("bytes=")?;
    let (start_str, end_str) = range.split_once('-')?;

    if start_str.is_empty() {
        let suffix: u64 = end_str.parse().ok()?;
        // bytes=-0 is semantically empty per RFC 7233.
        if suffix == 0 {
            return None;
        }
        Some(ByteRange {
            start: file_size.saturating_sub(suffix),
            end: file_size - 1,
        })
    } else {
        let start: u64 = start_str.parse().ok()?;
        let end = if end_str.is_empty() {
            file_size - 1
        } else {
            end_str.parse().ok()?
        };
        (start <= end && start < file_size).then(|| ByteRange {
            start,
            end: end.min(file_size - 1),
        })
    }
}

/// First byte offset of a `bytes 123-456/789` response.
pub fn content_range_start(value: &str) -> Option<u64> {
    value
        .trim()
        .strip_prefix("bytes ")?
        .split('-')
        .next()?
        .trim()
        .parse()
        .ok()
}

/// A whole-file response, or the single range a `Range` header asks for. An unsatisfiable
/// or malformed range gets the whole file.
pub async fn file_response(
    file: tokio::fs::File,
    file_size: u64,
    content_type: &str,
    headers: &HeaderMap,
) -> Response {
    let whole = Part::File {
        file,
        offset: 0,
        len: file_size,
    };
    spliced_response(vec![whole], content_type, headers).await
}

pub enum Part {
    Bytes(Bytes),
    File {
        file: tokio::fs::File,
        offset: u64,
        len: u64,
    },
}

impl Part {
    fn len(&self) -> u64 {
        match self {
            Part::Bytes(b) => b.len() as u64,
            Part::File { len, .. } => *len,
        }
    }
}

/// [`file_response`] over a body that is the parts laid end to end.
pub async fn spliced_response(
    parts: Vec<Part>,
    content_type: &str,
    headers: &HeaderMap,
) -> Response {
    let total: u64 = parts.iter().map(Part::len).sum();
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_byte_range(v, total));
    let (start, length) = range.map_or((0, total), |r| (r.start, r.length()));

    let mut pieces = Vec::new();
    let mut at = 0u64;
    for part in parts {
        let len = part.len();
        let (from, to) = (start.max(at), (start + length).min(at + len));
        if from < to {
            match piece(part, from - at, to - from).await {
                Ok(p) => pieces.push(p),
                Err(e) => {
                    error!(error = %e, "failed to seek in file");
                    return (StatusCode::INTERNAL_SERVER_ERROR, "seek error").into_response();
                }
            }
        }
        at += len;
    }
    let body = Body::from_stream(futures::stream::iter(pieces).flatten());

    let builder = Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, length.to_string())
        .header(header::ACCEPT_RANGES, "bytes");
    match range {
        Some(r) => builder
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, r.content_range(total)),
        None => builder.status(StatusCode::OK),
    }
    .body(body)
    .unwrap()
}

type Piece = futures::stream::BoxStream<'static, std::io::Result<Bytes>>;

async fn piece(part: Part, skip: u64, take: u64) -> std::io::Result<Piece> {
    Ok(match part {
        Part::Bytes(b) => {
            let slice = b.slice(skip as usize..(skip + take) as usize);
            futures::stream::once(async move { Ok(slice) }).boxed()
        }
        Part::File {
            mut file, offset, ..
        } => {
            file.seek(std::io::SeekFrom::Start(offset + skip)).await?;
            ReaderStream::new(file.take(take)).boxed()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `filesize` on a debrid session row is zero until the resolve has reported one —
    /// and a suffix range then underflowed `file_size - 1`.
    #[test]
    fn a_zero_length_file_has_no_satisfiable_range() {
        for header in ["bytes=0-", "bytes=-500", "bytes=0-99", "bytes=10-20"] {
            assert!(
                parse_byte_range(header, 0).is_none(),
                "{header} parsed against an empty file"
            );
        }
    }

    #[test]
    fn ordinary_ranges_parse() {
        let cases: &[(&str, Option<(u64, u64)>)] = &[
            ("bytes=0-99", Some((0, 99))),
            ("bytes=500-", Some((500, 999))),
            ("bytes=-100", Some((900, 999))),
            ("bytes=900-99999", Some((900, 999))),
            ("bytes=1000-", None),
            ("bytes=-0", None),
            ("nonsense", None),
        ];
        for (header, want) in cases {
            assert_eq!(
                parse_byte_range(header, 1000).map(|r| (r.start, r.end)),
                *want,
                "{header}"
            );
        }
    }

    #[test]
    fn content_range_start_reads_the_first_offset() {
        assert_eq!(content_range_start("bytes 123-456/789"), Some(123));
        assert_eq!(content_range_start("bytes */789"), None);
    }
}
