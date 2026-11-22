//! A debrid host stand-in that wiremock cannot play: it streams a whole-file GET at a
//! pace the test controls (so the progressive download can be held mid-file) and
//! answers ranged GETs after a fixed delay, the way a far-away CDN does. Its connections
//! pass through a TCP front the test can cut, or take down for a while the way a
//! restarting usenet engine is.

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub enum WholeBody {
    /// `first` bytes at once, then the rest after `pause`.
    Pause { first: usize, pause: Duration },
    /// `first` bytes, then nothing for as long as the connection is held.
    Stall { first: usize },
    /// `chunk` bytes every `every`, for ever (the file is repeated).
    Trickle { chunk: usize, every: Duration },
}

#[derive(Clone)]
pub struct UpstreamPlan {
    pub data: Arc<Vec<u8>>,
    pub range_delay: Duration,
    pub whole: WholeBody,
    /// Answer every request with this status and no body.
    pub refuse: Option<StatusCode>,
    /// Bytes from this offset on can never be served, as a usenet post with missing
    /// articles: every response stops there.
    pub hole: Option<Hole>,
}

#[derive(Clone, Copy)]
pub struct Hole {
    pub at: usize,
    pub answer: HoleAnswer,
}

#[derive(Clone, Copy)]
pub enum HoleAnswer {
    /// The response ends with an error where the hole starts, with no explanation.
    Abort,
    /// A request that starts in the hole is refused with `{"error": code}`.
    Error(StatusCode, &'static str),
}

impl UpstreamPlan {
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            data: Arc::new(data),
            range_delay: Duration::ZERO,
            whole: WholeBody::Pause {
                first: usize::MAX,
                pause: Duration::ZERO,
            },
            refuse: None,
            hole: None,
        }
    }
}

#[derive(Clone)]
struct Shared {
    plan: UpstreamPlan,
    ranges: Arc<Mutex<Vec<String>>>,
    whole_gets: Arc<Mutex<usize>>,
    requests: Arc<Mutex<Vec<Seen>>>,
    revoked: Arc<Mutex<Vec<String>>>,
}

/// One GET the host answered: its path, its Range header if it had one, and its User-Agent
/// (kobbler's own client, ffmpeg's or a probe reader's).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub path: String,
    pub range: Option<String>,
    pub agent: Option<String>,
}

pub struct Upstream {
    pub base: String,
    size: usize,
    ranges: Arc<Mutex<Vec<String>>>,
    whole_gets: Arc<Mutex<usize>>,
    requests: Arc<Mutex<Vec<Seen>>>,
    revoked: Arc<Mutex<Vec<String>>>,
    outages: tokio::sync::mpsc::UnboundedSender<Duration>,
}

impl Upstream {
    pub async fn start(plan: UpstreamPlan) -> Self {
        let size = plan.data.len();
        let shared = Shared {
            plan,
            ranges: Arc::default(),
            whole_gets: Arc::default(),
            requests: Arc::default(),
            revoked: Arc::default(),
        };
        let app = Router::new()
            .route("/{*file}", axum::routing::get(serve))
            .with_state(shared.clone());
        let backend = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend_addr = backend.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(backend, app).await.ok();
        });
        let front = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = front.local_addr().unwrap();
        let (outages, requested) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(run_front(front, backend_addr, requested));
        Self {
            base: format!("http://{addr}"),
            size,
            ranges: shared.ranges,
            whole_gets: shared.whole_gets,
            requests: shared.requests,
            revoked: shared.revoked,
            outages,
        }
    }

    /// Every connection open now is cut, and new ones are refused for `outage`.
    pub fn go_down(&self, outage: Duration) {
        self.outages.send(outage).unwrap();
    }

    /// Requests for paths under `prefix` are answered 403 from now on.
    pub fn revoke(&self, prefix: &str) {
        self.revoked.lock().unwrap().push(prefix.to_string());
    }

    pub fn url(&self, filename: &str) -> String {
        format!("{}/{filename}", self.base)
    }

    pub fn ranges_seen(&self) -> Vec<String> {
        self.ranges.lock().unwrap().clone()
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn whole_gets(&self) -> usize {
        *self.whole_gets.lock().unwrap()
    }

    pub fn requests(&self) -> Vec<Seen> {
        self.requests.lock().unwrap().clone()
    }
}

async fn serve(State(s): State<Shared>, uri: Uri, headers: HeaderMap) -> Response {
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    s.requests.lock().unwrap().push(Seen {
        path: uri.path().to_string(),
        range: range.map(str::to_string),
        agent: headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
    });
    if let Some(status) = s.plan.refuse {
        return status.into_response();
    }
    if s.revoked
        .lock()
        .unwrap()
        .iter()
        .any(|p| uri.path().starts_with(p.as_str()))
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let data = s.plan.data.clone();
    let total = data.len() as u64;
    let Some(range) = range else {
        *s.whole_gets.lock().unwrap() += 1;
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_LENGTH, total.to_string())
            .body(holed(
                whole_body(data, s.plan.whole.clone()),
                0,
                s.plan.hole,
            ))
            .unwrap();
    };
    s.ranges.lock().unwrap().push(range.to_string());
    tokio::time::sleep(s.plan.range_delay).await;
    let spec = range.trim_start_matches("bytes=");
    let (a, b) = spec.split_once('-').unwrap();
    let start: u64 = a.parse().unwrap();
    let end: u64 = if b.is_empty() {
        total - 1
    } else {
        b.parse::<u64>().unwrap().min(total - 1)
    };
    if let Some(Hole {
        at,
        answer: HoleAnswer::Error(status, code),
    }) = s.plan.hole
        && start as usize >= at
    {
        return (status, axum::Json(serde_json::json!({ "error": code }))).into_response();
    }
    let slice = data[start as usize..=end as usize].to_vec();
    Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(header::CONTENT_LENGTH, slice.len().to_string())
        .header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{total}"),
        )
        .body(holed(Body::from(slice), start as usize, s.plan.hole))
        .unwrap()
}

/// `body`, which starts at `offset` in the file, cut short with an error where `hole`
/// begins.
fn holed(body: Body, offset: usize, hole: Option<Hole>) -> Body {
    use futures::StreamExt;
    let Some(hole) = hole else {
        return body;
    };
    let mut at = offset;
    let stream = body.into_data_stream();
    Body::from_stream(async_stream::stream! {
        futures::pin_mut!(stream);
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(std::io::Error::other)?;
            if at + chunk.len() > hole.at {
                let keep = hole.at.saturating_sub(at);
                if keep > 0 {
                    yield Ok::<_, std::io::Error>(chunk.slice(..keep));
                }
                Err(std::io::Error::other("missing articles"))?;
            }
            at += chunk.len();
            yield Ok(chunk);
        }
    })
}

/// Pipes each connection to the backend. An outage cuts every connection piped so far and
/// leaves the port closed, so connecting is refused, until it is over.
async fn run_front(
    front: tokio::net::TcpListener,
    backend: std::net::SocketAddr,
    mut outages: tokio::sync::mpsc::UnboundedReceiver<Duration>,
) {
    let addr = front.local_addr().unwrap();
    let mut front = front;
    let mut cut = tokio_util::sync::CancellationToken::new();
    loop {
        tokio::select! {
            accepted = front.accept() => {
                let Ok((mut client, _)) = accepted else { continue };
                let cut = cut.clone();
                tokio::spawn(async move {
                    let Ok(mut server) = tokio::net::TcpStream::connect(backend).await else {
                        return;
                    };
                    tokio::select! {
                        _ = tokio::io::copy_bidirectional(&mut client, &mut server) => {}
                        _ = cut.cancelled() => {}
                    }
                });
            }
            outage = outages.recv() => {
                let Some(outage) = outage else { return };
                drop(front);
                cut.cancel();
                cut = tokio_util::sync::CancellationToken::new();
                tokio::time::sleep(outage).await;
                front = tokio::net::TcpListener::bind(addr).await.unwrap();
            }
        }
    }
}

fn whole_body(data: Arc<Vec<u8>>, plan: WholeBody) -> Body {
    let stream = async_stream::stream! {
        match plan {
            WholeBody::Pause { first, pause } => {
                let first = first.min(data.len());
                yield Ok::<_, std::io::Error>(Bytes::copy_from_slice(&data[..first]));
                if first < data.len() {
                    tokio::time::sleep(pause).await;
                    yield Ok(Bytes::copy_from_slice(&data[first..]));
                }
            }
            WholeBody::Stall { first } => {
                yield Ok(Bytes::copy_from_slice(&data[..first.min(data.len())]));
                std::future::pending::<()>().await;
            }
            WholeBody::Trickle { chunk, every } => {
                let mut at = 0usize;
                loop {
                    let end = (at + chunk).min(data.len());
                    yield Ok(Bytes::copy_from_slice(&data[at..end]));
                    at = if end == data.len() { 0 } else { end };
                    tokio::time::sleep(every).await;
                }
            }
        }
    };
    Body::from_stream(stream)
}
