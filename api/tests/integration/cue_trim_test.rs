use crate::common::*;
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

/// A release carrying 27 subtitle languages, cued the way mkvmerge cues them: one
/// point per line per language. 11,200 cue points, of which 400 are the video track.
const FIXTURE: &[u8] = include_bytes!("../fixtures/cue-trim/fat-cues.mkv");

struct Served {
    original: Vec<u8>,
    body: Vec<u8>,
}

impl Served {
    /// The byte range the response differs from the file in, if any.
    fn changed(&self) -> Option<(usize, usize)> {
        let first = (0..self.original.len()).find(|&i| self.original[i] != self.body[i])?;
        let last = (0..self.original.len())
            .rev()
            .find(|&i| self.original[i] != self.body[i])?;
        Some((first, last + 1))
    }
}

struct Harness {
    served: Served,
    server: TestServer,
    session_id: Uuid,
    /// Deleting the file out from under the handler is not the test; hold the guard.
    _dir: tempfile::TempDir,
}

async fn serve(sidecars: bool, enabled: bool) -> Harness {
    let server = TestServer::start_api_only().await;
    if !enabled {
        server
            .admin_client()
            .patch_json(
                "/api/v1/admin/config",
                &json!({ "mkv_cue_trim_enabled": false }),
            )
            .await
            .assert_ok();
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("film.mkv");
    std::fs::write(&path, FIXTURE).expect("write fixture");

    let session_id = Uuid::new_v4();
    let mut session = api::streaming::session::PlaybackSession::test_direct_play(
        session_id,
        Uuid::new_v4(),
        path.clone(),
    );
    if sidecars {
        session.subtitle_files = vec![api::streaming::session::SubtitleFile {
            track_id: Uuid::new_v4(),
            stream_index: 2,
            codec: "subrip".into(),
            language: Some("eng".into()),
            title: None,
            file_path: dir.path().join("sub.vtt"),
            extracted: false,
        }];
    }
    server
        .app_state
        .playback_sessions
        .insert(session_id, session);

    let body = fetch(&server, session_id, None).await.bytes;
    Harness {
        served: Served {
            original: FIXTURE.to_vec(),
            body,
        },
        server,
        session_id,
        _dir: dir,
    }
}

struct Fetched {
    status: StatusCode,
    content_range: Option<String>,
    bytes: Vec<u8>,
}

async fn fetch(server: &TestServer, session_id: Uuid, range: Option<&str>) -> Fetched {
    let mut req = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/playback/sessions/{session_id}/direct",
            server.api_url
        ))
        .bearer_auth(&server.admin_token);
    if let Some(r) = range {
        req = req.header(reqwest::header::RANGE, r);
    }
    let resp = req.send().await.expect("request");
    let status = resp.status();
    let content_range = resp
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = resp.bytes().await.expect("body").to_vec();
    Fetched {
        status,
        content_range,
        bytes,
    }
}

#[tokio::test]
async fn the_served_bytes_are_trimmed_without_changing_the_files_length() {
    let served = serve(false, true).await.served;

    assert_eq!(
        served.body.len(),
        FIXTURE.len(),
        "the response changed length, which would move every cluster offset"
    );

    let (from, to) = served.changed().expect("nothing was trimmed");
    // Only the cue index may differ, and it lives after the track headers.
    assert!(
        from > 4096,
        "a byte before the cue index changed, at {from}"
    );
    assert!(
        to <= FIXTURE.len() - 10,
        "the element after the cue index was disturbed"
    );

    // The tail of the replaced region is the Void payload, which is zeros.
    let tail = &served.body[to.saturating_sub(1024)..to];
    assert!(
        tail.iter().all(|&b| b == 0),
        "the trimmed index is not padded out with zeros"
    );
}

#[tokio::test]
async fn every_range_agrees_with_the_whole_response() {
    let h = serve(false, true).await;
    let (served, server, session_id) = (&h.served, &h.server, h.session_id);
    let (from, to) = served.changed().expect("nothing was trimmed");
    let total = FIXTURE.len();

    // The edges of the replaced region are where offset arithmetic goes wrong.
    let cases: Vec<(usize, usize)> = vec![
        (0, total),
        (from, to),
        (from - 10, from + 10),
        (to - 10, (to + 10).min(total)),
        (from - 1, to + 1),
        (from + 100, to - 100),
        (0, from),
        (to, total),
        (total - 1, total),
    ];

    for (start, end) in cases {
        let last = end - 1;
        let got = fetch(server, session_id, Some(&format!("bytes={start}-{last}"))).await;
        assert_eq!(
            got.status,
            StatusCode::PARTIAL_CONTENT,
            "range {start}-{last}"
        );
        assert_eq!(
            got.content_range.as_deref(),
            Some(format!("bytes {start}-{last}/{total}").as_str()),
            "range {start}-{last}"
        );
        assert_eq!(got.bytes.len(), end - start, "range {start}-{last} length");
        assert_eq!(
            got.bytes,
            served.body[start..end],
            "range {start}-{last} disagrees with the whole response"
        );
    }
}

#[tokio::test]
async fn a_suffix_range_is_served_correctly() {
    let h = serve(false, true).await;
    let got = fetch(&h.server, h.session_id, Some("bytes=-500")).await;
    assert_eq!(got.bytes, h.served.body[FIXTURE.len() - 500..]);
}

#[tokio::test]
async fn extracted_sidecars_drop_more_of_the_index() {
    let with_sidecars = serve(true, true).await.served;
    let without = serve(false, true).await.served;

    let live = |s: &Served| {
        let (from, to) = s.changed().expect("nothing was trimmed");
        // How much of the replaced region is still real bytes rather than Void.
        s.body[from..to]
            .iter()
            .rposition(|&b| b != 0)
            .map(|i| i + 1)
            .unwrap_or(0)
    };

    assert!(
        live(&with_sidecars) < live(&without),
        "sidecars should let every subtitle index go: {} vs {}",
        live(&with_sidecars),
        live(&without)
    );
}

#[tokio::test]
async fn the_file_is_served_untouched_when_the_setting_is_off() {
    let served = serve(false, false).await.served;
    assert_eq!(
        served.changed(),
        None,
        "bytes were altered with mkv_cue_trim_enabled = false"
    );
    assert_eq!(served.body, FIXTURE);
}
