//! A renderer that streams one long response never heartbeats; the bytes it pulls are
//! the only sign the session is still being watched.

use crate::streaming::session::PlaybackSession;
use dashmap::DashMap;
use futures::Stream;
use futures::StreamExt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

const TOUCH_EVERY: Duration = Duration::from_secs(10);

pub fn touching<S: Stream>(
    stream: S,
    sessions: Arc<DashMap<Uuid, PlaybackSession>>,
    session_id: Uuid,
) -> impl Stream<Item = S::Item> {
    let mut last = Instant::now();
    stream.inspect(move |_| {
        if last.elapsed() >= TOUCH_EVERY {
            last = Instant::now();
            if let Some(mut session) = sessions.get_mut(&session_id) {
                session.touch();
            }
        }
    })
}

pub fn touching_response(
    response: axum::response::Response,
    sessions: Arc<DashMap<Uuid, PlaybackSession>>,
    session_id: Uuid,
) -> axum::response::Response {
    let (parts, body) = response.into_parts();
    let body =
        axum::body::Body::from_stream(touching(body.into_data_stream(), sessions, session_id));
    axum::response::Response::from_parts(parts, body)
}
