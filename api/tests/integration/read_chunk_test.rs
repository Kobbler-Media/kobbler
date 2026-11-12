use api::streaming::byte_range::{FILE_CHUNK, file_stream};
use futures::StreamExt;
use std::path::Path;

#[tokio::test]
async fn a_file_body_is_read_in_large_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("media.bin");
    std::fs::write(&path, vec![7u8; 3 * FILE_CHUNK]).unwrap();
    let file = tokio::fs::File::open(&path).await.unwrap();

    let first = file_stream(file).next().await.unwrap().unwrap();
    assert_eq!(first.len(), FILE_CHUNK);
}

/// ReaderStream's default 4 KiB reads held direct play on the NAS to ~4 MB/s; every body
/// built from a reader goes through `file_stream` or `pipe_stream` instead.
#[test]
fn no_response_body_reads_at_the_default_4k() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    walk(&src, &mut |path, text| {
        for (i, line) in text.lines().enumerate() {
            if line.contains("ReaderStream::new(") {
                offenders.push(format!("{}:{}", path.display(), i + 1));
            }
        }
    });
    assert!(
        offenders.is_empty(),
        "default-sized ReaderStream at {offenders:#?}"
    );
}

fn walk(dir: &Path, visit: &mut dyn FnMut(&Path, &str)) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, visit);
        } else if path.extension().is_some_and(|e| e == "rs") {
            visit(&path, &std::fs::read_to_string(&path).unwrap());
        }
    }
}
