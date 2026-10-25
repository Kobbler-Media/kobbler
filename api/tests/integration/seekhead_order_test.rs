use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::UpstreamPlan;
use crate::common::*;
use reqwest::StatusCode;
use uuid::Uuid;

/// Laid out like the release that took 50s to resume: the SeekHead names Cues before
/// Tracks (Info, Cues, Tags, Tracks), the video track alone is cued, and the SeekHead
/// carries a CRC-32. libVLC loads Cues in SeekHead order, finds no track to attach the
/// points to yet, disables every one of them, and over HTTP can then only seek by
/// decoding from the first cluster.
const FIXTURE: &[u8] = include_bytes!("../fixtures/mkv/cues-before-tracks.mkv");

const INFO: u32 = 0x1549_A966;
const TRACKS: u32 = 0x1654_AE6B;
const CUES: u32 = 0x1C53_BB6B;
const SEEKHEAD: u32 = 0x114D_9B74;
const SEEK: u32 = 0x4DBB;
const SEEK_ID: u32 = 0x53AB;
const SEEK_POSITION: u32 = 0x53AC;
const CRC32: u32 = 0xBF;

fn vint(b: &[u8], p: usize, keep_marker: bool) -> (u64, usize) {
    let width = b[p].leading_zeros() as usize + 1;
    let mut v = if keep_marker {
        b[p] as u64
    } else {
        (b[p] & (0xFFu16 >> width) as u8) as u64
    };
    for i in 1..width {
        v = (v << 8) | b[p + i] as u64;
    }
    (v, width)
}

/// (id, body start, body end) for each child of the container body `b[from..to]`.
fn children(b: &[u8], from: usize, to: usize) -> Vec<(u32, usize, usize)> {
    let mut out = Vec::new();
    let mut p = from;
    while p < to {
        let (id, idw) = vint(b, p, true);
        let (size, szw) = vint(b, p + idw, false);
        let body = p + idw + szw;
        out.push((id as u32, body, body + size as usize));
        p = body + size as usize;
    }
    out
}

struct SeekHead {
    span: (usize, usize),
    entries: Vec<(u32, u64)>,
    crc: Option<(u32, (usize, usize))>,
}

fn seekhead(file: &[u8]) -> SeekHead {
    let (_, idw) = vint(file, 0, true);
    let (size, szw) = vint(file, idw, false);
    let segment = idw + szw + size as usize;
    let (_, sidw) = vint(file, segment, true);
    let (_, sszw) = vint(file, segment + sidw, false);
    let segment_data = segment + sidw + sszw;
    let (id, from, to) = children(file, segment_data, segment_data + 4096)
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(id, SEEKHEAD);

    let mut entries = Vec::new();
    let mut crc = None;
    for (id, a, b) in children(file, from, to) {
        if id == CRC32 {
            crc = Some((u32::from_le_bytes(file[a..b].try_into().unwrap()), (b, to)));
        }
        if id != SEEK {
            continue;
        }
        let fields = children(file, a, b);
        let field = |want| {
            let (_, x, y) = fields.iter().find(|f| f.0 == want).copied().unwrap();
            file[x..y]
                .iter()
                .fold(0u64, |acc, &v| (acc << 8) | v as u64)
        };
        entries.push((field(SEEK_ID) as u32, field(SEEK_POSITION)));
    }
    SeekHead {
        span: (from, to),
        entries,
        crc,
    }
}

fn changed(original: &[u8], served: &[u8]) -> Option<(usize, usize)> {
    let first = (0..original.len()).find(|&i| original[i] != served[i])?;
    let last = (0..original.len()).rfind(|&i| original[i] != served[i])?;
    Some((first, last + 1))
}

/// What every route has to hand the player for this file.
fn assert_tracks_listed_before_cues(served: &[u8]) {
    assert_eq!(
        served.len(),
        FIXTURE.len(),
        "the response changed length, which would move every cluster offset"
    );

    let before = seekhead(FIXTURE);
    let after = seekhead(served);
    let order: Vec<u32> = after.entries.iter().map(|e| e.0).collect();
    let at = |id| order.iter().position(|&x| x == id).unwrap();
    assert!(
        at(TRACKS) < at(CUES) && at(INFO) < at(CUES),
        "the SeekHead still sends the demuxer to Cues before Info and Tracks: {order:08X?}"
    );

    let mut want = before.entries.clone();
    let mut got = after.entries.clone();
    want.sort();
    got.sort();
    assert_eq!(got, want, "an element's recorded position changed");

    let (from, to) = changed(FIXTURE, served).expect("the SeekHead was not rewritten");
    assert!(
        from >= before.span.0 && to <= before.span.1,
        "bytes outside the SeekHead changed: {from}..{to}, SeekHead is {:?}",
        before.span
    );

    let (crc, (a, b)) = after.crc.expect("the CRC-32 element went missing");
    assert_eq!(
        crc,
        crc32fast::hash(&served[a..b]),
        "the SeekHead's CRC-32 no longer matches it"
    );
}

/// Library direct play.
async fn direct(server: &TestServer, session_id: Uuid, range: Option<&str>) -> Vec<u8> {
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
    assert!(resp.status().is_success(), "{}", resp.status());
    resp.bytes().await.expect("body").to_vec()
}

const HEAD_RANGES: &[(usize, usize)] = &[(0, 4096), (0, 64), (60, 75), (100, 101)];

#[tokio::test]
async fn library_direct_play_lists_tracks_before_cues() {
    let (server, _upstream, session_id, _dir) =
        proxied_mkv_session(UpstreamPlan::new(FIXTURE.to_vec())).await;

    let whole = direct(&server, session_id, None).await;
    assert_tracks_listed_before_cues(&whole);
    for &(start, end) in HEAD_RANGES {
        let range = format!("bytes={start}-{}", end - 1);
        let got = direct(&server, session_id, Some(&range)).await;
        assert_eq!(
            got,
            whole[start..end],
            "{range} disagrees with the whole file"
        );
    }
}

/// The SeekHead is read in the very first range, long before the Cues are. It must be
/// rewritten there too, and agree with every later read of the session.
async fn assert_debrid_session_lists_tracks_before_cues(server: &TestServer, session_id: Uuid) {
    let mut heads = Vec::new();
    for &(start, end) in HEAD_RANGES {
        let range = format!("bytes={start}-{}", end - 1);
        let got = play(server, session_id, &range).await;
        assert_eq!(got.status, StatusCode::PARTIAL_CONTENT, "{range}");
        heads.push((range, start, got.body));
    }

    let whole = play(server, session_id, "bytes=0-").await;
    assert_eq!(whole.status, StatusCode::PARTIAL_CONTENT);
    assert_tracks_listed_before_cues(&whole.body);
    for (range, start, body) in heads {
        assert_eq!(
            body,
            whole.body[start..start + body.len()],
            "{range} disagrees with the whole file"
        );
    }
}

#[tokio::test]
async fn debrid_play_from_the_local_cache_lists_tracks_before_cues() {
    let rig = Rig::start(UpstreamPlan::new(FIXTURE.to_vec()), "episode.mkv").await;
    let session_id = rig.resolve("seekheadhash").await;
    rig.wait_for_complete("seekheadhash").await;
    assert_debrid_session_lists_tracks_before_cues(&rig.server, session_id).await;
}

#[tokio::test]
async fn debrid_play_proxied_from_upstream_lists_tracks_before_cues() {
    let (server, _upstream, session_id, _dir) =
        proxied_mkv_session(UpstreamPlan::new(FIXTURE.to_vec())).await;
    assert_debrid_session_lists_tracks_before_cues(&server, session_id).await;
}
