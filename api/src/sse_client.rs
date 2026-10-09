//! Minimal SSE (Server-Sent Events) client built on reqwest.
//!
//! Replaces the abandoned `reqwest-eventsource` crate with a hand-rolled parser
//! that wraps a `reqwest` response byte stream.

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::BytesMut;
use futures_util::Stream;
use reqwest::RequestBuilder;

/// A parsed SSE event.
#[derive(Debug)]
pub enum SseEvent {
    /// The connection was successfully opened.
    Open,
    /// A message event with an event name and data payload.
    Message { event: String, data: String },
}

/// Errors that can occur while reading an SSE stream.
#[derive(Debug)]
pub enum SseError {
    /// The underlying HTTP transport failed.
    Transport(reqwest::Error),
    /// The server closed the stream normally.
    StreamEnded,
}

impl std::fmt::Display for SseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SseError::Transport(e) => write!(f, "SSE transport error: {e}"),
            SseError::StreamEnded => write!(f, "SSE stream ended"),
        }
    }
}

impl std::error::Error for SseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SseError::Transport(e) => Some(e),
            SseError::StreamEnded => None,
        }
    }
}

/// An SSE stream that yields [`SseEvent`]s from a server.
pub struct SseStream {
    inner: Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>,
    buf: BytesMut,
    event_name: Option<String>,
    data_buf: String,
    open_sent: bool,
    done: bool,
}

impl SseStream {
    /// Connect to an SSE endpoint using the given request builder.
    ///
    /// Sends the request, validates the response, and returns a stream of SSE events.
    /// The first event yielded is always [`SseEvent::Open`].
    pub async fn connect(request: RequestBuilder) -> Result<Self, SseError> {
        let response = request.send().await.map_err(SseError::Transport)?;
        let response = response.error_for_status().map_err(SseError::Transport)?;

        Ok(SseStream {
            inner: Box::pin(response.bytes_stream()),
            buf: BytesMut::new(),
            event_name: None,
            data_buf: String::new(),
            open_sent: false,
            done: false,
        })
    }

    /// Close the stream, dropping the underlying connection.
    pub fn close(&mut self) {
        self.done = true;
    }

    /// Extract complete lines from the buffer and process them into events.
    fn drain_lines(&mut self, events: &mut Vec<Result<SseEvent, SseError>>) {
        while let Some(newline_pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line_bytes = self.buf.split_to(newline_pos + 1);
            let line = std::str::from_utf8(&line_bytes)
                .unwrap_or("")
                .trim_end_matches('\n')
                .trim_end_matches('\r');

            if line.is_empty() {
                // Empty line = dispatch event
                if !self.data_buf.is_empty() {
                    // Remove trailing newline from data buffer
                    if self.data_buf.ends_with('\n') {
                        self.data_buf.pop();
                    }
                    let event = self.event_name.take().unwrap_or_else(|| "message".into());
                    let data = std::mem::take(&mut self.data_buf);
                    events.push(Ok(SseEvent::Message { event, data }));
                }
                self.event_name = None;
            } else if let Some(rest) = line.strip_prefix(':') {
                // Comment line — ignore
                let _ = rest;
            } else if let Some(rest) = line.strip_prefix("event:") {
                self.event_name = Some(rest.trim_start().to_string());
            } else if let Some(rest) = line.strip_prefix("data:") {
                self.data_buf.push_str(rest.trim_start());
                self.data_buf.push('\n');
            } else if line.starts_with("id:") || line.starts_with("retry:") {
                // Ignored per spec
            } else {
                // Field with no colon — treat as field name with empty value
                // (rarely used, but spec-compliant)
            }
        }
    }
}

impl Stream for SseStream {
    type Item = Result<SseEvent, SseError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if this.done {
            return Poll::Ready(None);
        }

        // Emit Open event first
        if !this.open_sent {
            this.open_sent = true;
            return Poll::Ready(Some(Ok(SseEvent::Open)));
        }

        loop {
            // Try to drain buffered lines into events first
            let mut events = Vec::new();
            this.drain_lines(&mut events);
            if let Some(event) = events.into_iter().next() {
                return Poll::Ready(Some(event));
            }

            // Need more data from the byte stream
            match this.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    this.buf.extend_from_slice(&chunk);
                    // Loop back to try draining lines
                }
                Poll::Ready(Some(Err(e))) => {
                    this.done = true;
                    return Poll::Ready(Some(Err(SseError::Transport(e))));
                }
                Poll::Ready(None) => {
                    this.done = true;
                    return Poll::Ready(Some(Err(SseError::StreamEnded)));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
