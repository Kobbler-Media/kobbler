//! A debrid host stand-in that wiremock cannot play: it streams a whole-file GET at a
//! pace the test controls (so the progressive download can be held mid-file) and
//! answers ranged GETs after a fixed delay, the way a far-away CDN does.

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
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
        }
    }
}

#[derive(Clone)]
struct Shared {
    plan: UpstreamPlan,
    ranges: Arc<Mutex<Vec<String>>>,
    whole_gets: Arc<Mutex<usize>>,
}

pub struct Upstream {
    pub base: String,
    size: usize,
    ranges: Arc<Mutex<Vec<String>>>,
    whole_gets: Arc<Mutex<usize>>,
}

impl Upstream {
    pub async fn start(plan: UpstreamPlan) -> Self {
        let size = plan.data.len();
        let shared = Shared {
            plan,
            ranges: Arc::default(),
            whole_gets: Arc::default(),
        };
        let app = Router::new()
            .route("/{*file}", axum::routing::get(serve))
            .with_state(shared.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        Self {
            base: format!("http://{addr}"),
            size,
            ranges: shared.ranges,
            whole_gets: shared.whole_gets,
        }
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
}

async fn serve(State(s): State<Shared>, headers: HeaderMap) -> Response {
    if let Some(status) = s.plan.refuse {
        return status.into_response();
    }
    let data = s.plan.data.clone();
    let total = data.len() as u64;
    let Some(range) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) else {
        *s.whole_gets.lock().unwrap() += 1;
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_LENGTH, total.to_string())
            .body(whole_body(data, s.plan.whole.clone()))
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
    let slice = data[start as usize..=end as usize].to_vec();
    Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(header::CONTENT_LENGTH, slice.len().to_string())
        .header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{total}"),
        )
        .body(Body::from(slice))
        .unwrap()
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
