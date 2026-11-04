use crate::common::audiobooks::*;
use crate::common::client::{TestClient, TestResponse};
use crate::common::mp3::*;
use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;
use std::path::{Path, PathBuf};
use uuid::Uuid;

type Mp3Writer = fn(&Path, u32);

const FIXTURES: [(&str, Mp3Writer); 2] = [
    ("vbr with id3v2 and cover", write_tagged_vbr_mp3),
    (
        "cbr with junk, ape and id3v1",
        write_cbr_mp3_with_junk_and_trailing_tags,
    ),
];

fn probe(bytes: &[u8]) -> Value {
    let file = temp_copy(bytes);
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-of",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(file.path())
        .output()
        .expect("ffprobe");
    serde_json::from_slice(&out.stdout).unwrap()
}

fn top_level_boxes(bytes: &[u8]) -> Vec<(String, usize)> {
    let mut boxes = Vec::new();
    let mut at = 0usize;
    while at + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        boxes.push((
            String::from_utf8_lossy(&bytes[at + 4..at + 8]).to_string(),
            at,
        ));
        if size < 8 {
            break;
        }
        at += size;
    }
    boxes
}

fn box_offset(bytes: &[u8], kind: &str) -> usize {
    top_level_boxes(bytes)
        .into_iter()
        .find(|(k, _)| k == kind)
        .unwrap_or_else(|| panic!("no {kind} box"))
        .1
}

fn header<'a>(resp: &'a TestResponse, name: &str) -> &'a str {
    resp.headers[name].to_str().unwrap()
}

async fn seed_mp3_with(
    server: &TestServer,
    seconds: u32,
    write: Mp3Writer,
) -> (Uuid, Uuid, PathBuf) {
    let (book_id, ids) = seed_audiobook(
        server,
        "Treehouse",
        &[file("ch1.mp3", seconds as f64, Audio::None)],
    )
    .await;
    let path = audio_dir(server, book_id).join("ch1.mp3");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    write(&path, seconds);
    (book_id, ids[0], path)
}

async fn seed_mp3(server: &TestServer, seconds: u32) -> (Uuid, Uuid, PathBuf) {
    seed_mp3_with(server, seconds, write_tagged_vbr_mp3).await
}

fn audio_url(book_id: Uuid, file_id: Uuid, query: &str) -> String {
    format!("/api/v1/admin/books/{book_id}/audio/{file_id}{query}")
}

fn mp4_url(book_id: Uuid, file_id: Uuid) -> String {
    audio_url(book_id, file_id, "?container=mp4")
}

fn assert_same_audio(mp4: &[u8], source: &Path, label: &str) {
    let copy = temp_copy(mp4);
    assert!(
        decode_pcm(copy.path()) == decode_pcm(source),
        "{label}: decoded audio differs from the source"
    );

    let (ours, theirs) = (packets(copy.path(), None), packets(source, None));
    assert_eq!(ours.len(), theirs.len(), "{label}: packet count");
    let start = seconds(&audio_stream(source), "start_time");
    for (i, (o, t)) in ours.iter().zip(&theirs).enumerate() {
        assert_eq!(o.hash, t.hash, "{label}: packet {i} data");
        assert!(
            (o.pts_time - (t.pts_time - start)).abs() < 1e-4,
            "{label}: packet {i} at {} not {}",
            o.pts_time,
            t.pts_time - start
        );
    }

    let duration = |p: &Path| seconds(&audio_stream(p), "duration");
    let (ours, theirs) = (duration(copy.path()), duration(source));
    assert!(
        (ours - theirs).abs() < 1e-3,
        "{label}: duration {ours} vs source {theirs}"
    );
}

async fn ranged(client: &TestClient, url: &str, range: &str) -> TestResponse {
    let resp = client.get_with_header(url, "range", range).await;
    assert_eq!(resp.status, StatusCode::PARTIAL_CONTENT, "{range}");
    assert_eq!(header(&resp, "accept-ranges"), "bytes", "{range}");
    assert_eq!(header(&resp, "content-type"), "audio/mp4", "{range}");
    assert_eq!(
        header(&resp, "content-length"),
        resp.bytes().len().to_string(),
        "{range}"
    );
    resp
}

fn files_under(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_under(&path));
        } else {
            found.push((path.clone(), entry.metadata().unwrap().len()));
        }
    }
    found
}

#[tokio::test]
async fn mp3_as_mp4_is_an_audio_only_faststart_mp4_of_the_mp3_stream() {
    let server = TestServer::start_api_only().await;
    let (book_id, file_id, _) = seed_mp3(&server, 20).await;

    let resp = server.admin_client().get(&mp4_url(book_id, file_id)).await;
    resp.assert_ok();
    assert_eq!(header(&resp, "content-type"), "audio/mp4");
    assert_eq!(header(&resp, "accept-ranges"), "bytes");
    assert_eq!(
        header(&resp, "content-length"),
        resp.bytes().len().to_string()
    );

    let boxes: Vec<String> = top_level_boxes(resp.bytes())
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(boxes, ["ftyp", "moov", "mdat"]);

    let probed = probe(resp.bytes());
    let streams = probed["streams"].as_array().unwrap();
    assert_eq!(
        streams.len(),
        1,
        "cover art must not become a video track: {streams:?}"
    );
    assert_eq!(streams[0]["codec_name"], "mp3");
    let duration: f64 = probed["format"]["duration"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((duration - 20.0).abs() < 0.01, "duration {duration}");
}

#[tokio::test]
async fn mp3_as_mp4_decodes_to_the_source_audio_frame_for_frame() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    for (label, write) in FIXTURES {
        let (book_id, file_id, source) = seed_mp3_with(&server, 45, write).await;
        let resp = client.get(&mp4_url(book_id, file_id)).await;
        resp.assert_ok();
        assert_same_audio(resp.bytes(), &source, label);
    }
}

#[tokio::test]
async fn mp3_as_mp4_ranges_reassemble_into_the_same_audio() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    for (label, write) in FIXTURES {
        let (book_id, file_id, source) = seed_mp3_with(&server, 8, write).await;
        let url = mp4_url(book_id, file_id);
        let whole = client.get(&url).await;
        whole.assert_ok();
        let (whole, total) = (whole.bytes(), whole.bytes().len());
        let mdat = box_offset(whole, "mdat");

        let mut cuts = vec![0, 17, mdat - 3, mdat + 5, mdat + 9, mdat + 4000];
        cuts.extend((mdat + 4000..total).step_by(9973).skip(1));
        let mut assembled = Vec::new();
        for pair in cuts.windows(2) {
            let range = format!("bytes={}-{}", pair[0], pair[1] - 1);
            let part = ranged(&client, &url, &range).await;
            assert_eq!(
                header(&part, "content-range"),
                format!("bytes {}-{}/{total}", pair[0], pair[1] - 1),
                "{label}"
            );
            assembled.extend_from_slice(part.bytes());
        }
        let tail = ranged(&client, &url, &format!("bytes={}-", cuts.last().unwrap())).await;
        assembled.extend_from_slice(tail.bytes());
        assert!(assembled == whole, "{label}: ranges do not reassemble");
        assert_same_audio(&assembled, &source, label);

        let suffix = ranged(&client, &url, "bytes=-300").await;
        assert_eq!(suffix.bytes(), &whole[total - 300..], "{label}");
        let past_end = ranged(
            &client,
            &url,
            &format!("bytes={}-{}", total - 10, total + 50),
        )
        .await;
        assert_eq!(past_end.bytes(), &whole[total - 10..], "{label}");

        let unsatisfiable = client
            .get_with_header(&url, "range", &format!("bytes={total}-"))
            .await;
        unsatisfiable.assert_ok();
        assert_eq!(unsatisfiable.bytes(), whole, "{label}");
    }
}

#[tokio::test]
async fn mp3_as_mp4_seeks_land_on_the_exact_frame() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    for (label, write) in FIXTURES {
        let (book_id, file_id, source) = seed_mp3_with(&server, 20, write).await;
        let resp = client.get(&mp4_url(book_id, file_id)).await;
        resp.assert_ok();
        let copy = temp_copy(resp.bytes());
        let start = seconds(&audio_stream(&source), "start_time");
        let frames = packets(&source, None);

        let covers = |pts: f64, duration: f64, t: f64| pts <= t + 1e-6 && t < pts + duration;
        for target in [0.5, 3.0, 7.31, 12.777, 19.5] {
            let want = frames
                .iter()
                .find(|f| covers(f.pts_time - start, f.duration_time, target))
                .unwrap_or_else(|| panic!("{label}: no source frame at {target}"));
            // ffprobe's interval seek may stop a frame short of the target even on ffmpeg's
            // own mp4 output, so the frame under the target is looked up among the next few.
            let landed = packets(copy.path(), Some(&format!("{target}%+#3")));
            let first = landed[0].pts_time;
            assert!(
                first <= target + 1e-6 && target - first < 2.0 * want.duration_time,
                "{label}: seek to {target} landed at {first}"
            );
            let hit = landed
                .iter()
                .find(|p| covers(p.pts_time, p.duration_time, target))
                .unwrap_or_else(|| panic!("{label}: seek to {target} missed it: {landed:?}"));
            assert_eq!(hit.hash, want.hash, "{label}: seek to {target}");
            assert!(
                (hit.pts_time - (want.pts_time - start)).abs() < 1e-4,
                "{label}: seek to {target} reported {}",
                hit.pts_time
            );
        }
    }
}

#[tokio::test]
async fn mp3_as_mp4_stores_only_a_small_index_beside_the_source() {
    let server = TestServer::start_api_only().await;
    let (book_id, file_id, source) = seed_mp3(&server, 30).await;
    server
        .admin_client()
        .get(&mp4_url(book_id, file_id))
        .await
        .assert_ok();

    let root = PathBuf::from(server.storage_root());
    let source_size = std::fs::metadata(&source).unwrap().len();
    let index = root.join("audio_mp4_index").join(file_id.to_string());
    let index_size = std::fs::metadata(&index)
        .unwrap_or_else(|e| panic!("no index at {}: {e}", index.display()))
        .len();
    assert!(
        index_size * 4 < source_size,
        "index {index_size} bytes for a {source_size}-byte source"
    );
    for (path, size) in files_under(&root) {
        assert!(
            path == source || size * 2 < source_size,
            "{} is {size} bytes, near the {source_size}-byte source",
            path.display()
        );
    }
}

#[tokio::test]
async fn mp3_as_mp4_follows_a_replaced_source() {
    let server = TestServer::start_api_only().await;
    let (book_id, file_id, path) = seed_mp3(&server, 10).await;
    let client = server.admin_client();
    let url = mp4_url(book_id, file_id);
    client.get(&url).await.assert_ok();

    std::thread::sleep(std::time::Duration::from_millis(20));
    write_tagged_vbr_mp3(&path, 5);

    let resp = client.get(&url).await;
    resp.assert_ok();
    assert_same_audio(resp.bytes(), &path, "replaced source");
}

#[tokio::test]
async fn mp3_as_mp4_falls_back_to_the_original_when_it_holds_no_mp3_frames() {
    let server = TestServer::start_api_only().await;
    let (book_id, file_id, path) = seed_mp3(&server, 3).await;
    std::fs::write(&path, b"not an mp3 at all, just text in a file").unwrap();

    let resp = server.admin_client().get(&mp4_url(book_id, file_id)).await;
    resp.assert_ok();
    assert_eq!(header(&resp, "content-type"), "audio/mpeg");
    assert_eq!(resp.bytes(), std::fs::read(&path).unwrap());
}

#[tokio::test]
async fn container_param_leaves_other_requests_on_the_original_file() {
    let server = TestServer::start_api_only().await;
    let (book_id, mp3_id, mp3_path) = seed_mp3(&server, 5).await;
    let m4b_id = seed_files(
        &server,
        book_id,
        &[file("whole.m4b", 3.0, Audio::Tone(3.0))],
    )
    .await[0];
    let m4b_path = audio_dir(&server, book_id).join("whole.m4b");
    let client = server.admin_client();

    let cases = [
        (mp3_id, "", "audio/mpeg", &mp3_path),
        (m4b_id, "?container=mp4", "audio/mp4", &m4b_path),
    ];
    for (file_id, query, expected_type, source) in cases {
        let resp = client.get(&audio_url(book_id, file_id, query)).await;
        resp.assert_ok();
        assert_eq!(header(&resp, "content-type"), expected_type, "{query}");
        assert_eq!(resp.bytes(), std::fs::read(source).unwrap(), "{query}");
    }
}

#[tokio::test]
async fn container_param_rejects_unknown_containers_and_anonymous_callers() {
    let server = TestServer::start_api_only().await;
    let (book_id, file_id, _) = seed_mp3(&server, 3).await;

    let unknown = server
        .admin_client()
        .get(&audio_url(book_id, file_id, "?container=webm"))
        .await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);

    let anonymous = server.api_client().get(&mp4_url(book_id, file_id)).await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
}
