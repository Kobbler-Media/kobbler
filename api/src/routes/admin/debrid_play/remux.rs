use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio_util::io::ReaderStream;
use tracing::{error, warn};

/// Pipe a reqwest response body through ffmpeg for on-the-fly remux to fMP4.
///
/// Video is copied as-is (zero CPU), audio is transcoded to AAC for browser
/// compatibility. Output is fragmented MP4 with `frag_keyframe+empty_moov`
/// so it's streamable without needing the moov atom at the end.
pub(super) async fn remux_proxy_stream(url: &str, audio_stream: Option<i32>) -> Response {
    let http_client = crate::http::new_client();

    // Fetch full file from debrid — no range for remux (ffmpeg needs sequential input)
    let upstream = match http_client.get(url).send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            error!(status = %r.status(), "debrid upstream returned error for remux");
            return (StatusCode::BAD_GATEWAY, "upstream error").into_response();
        }
        Err(e) => {
            error!(error = %e, "failed to connect to debrid stream for remux");
            return (StatusCode::BAD_GATEWAY, "upstream connection failed").into_response();
        }
    };

    let mut ffmpeg = match tokio::process::Command::new("ffmpeg")
        .args(fmp4_args("pipe:0", audio_stream))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            error!(error = %e, "failed to spawn ffmpeg for remux");
            return (StatusCode::INTERNAL_SERVER_ERROR, "remux unavailable").into_response();
        }
    };

    let mut stdin = ffmpeg.stdin.take().expect("ffmpeg stdin was piped");
    let stdout = ffmpeg.stdout.take().expect("ffmpeg stdout was piped");

    // Spawn a task to pipe the upstream body into ffmpeg stdin
    tokio::spawn(async move {
        use futures::StreamExt;
        let mut stream = upstream.bytes_stream();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    if tokio::io::AsyncWriteExt::write_all(&mut stdin, &bytes)
                        .await
                        .is_err()
                    {
                        break; // ffmpeg closed stdin (e.g. output consumer disconnected)
                    }
                }
                Err(e) => {
                    warn!(error = %e, "upstream read error during remux");
                    break;
                }
            }
        }
        drop(stdin); // Close stdin to signal EOF to ffmpeg
    });

    // Stream ffmpeg stdout back to the client
    let reader_stream = ReaderStream::new(tokio::io::BufReader::new(stdout));

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "video/mp4")
        // No Content-Length — chunked transfer since we don't know remuxed size
        .body(Body::from_stream(reader_stream))
        .unwrap()
        .into_response()
}

/// Remux a local file through ffmpeg to fragmented MP4.
pub(super) async fn remux_local_file(
    path: &std::path::Path,
    audio_stream: Option<i32>,
) -> Response {
    let path_str = path.to_string_lossy().to_string();

    let mut ffmpeg = match tokio::process::Command::new("ffmpeg")
        .args(fmp4_args(&path_str, audio_stream))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            error!(error = %e, "failed to spawn ffmpeg for local remux");
            return (StatusCode::INTERNAL_SERVER_ERROR, "remux unavailable").into_response();
        }
    };

    let stdout = ffmpeg.stdout.take().expect("ffmpeg stdout was piped");
    let reader_stream = ReaderStream::new(tokio::io::BufReader::new(stdout));

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "video/mp4")
        .body(Body::from_stream(reader_stream))
        .unwrap()
        .into_response()
}

/// Without explicit maps ffmpeg keeps whichever audio track it rates best, not the one
/// the viewer's language preference picked.
fn fmp4_args(input: &str, audio_stream: Option<i32>) -> Vec<String> {
    let mut args = vec!["-i".to_string(), input.to_string()];
    if let Some(index) = audio_stream {
        args.extend([
            "-map".to_string(),
            "0:v:0".to_string(),
            "-map".to_string(),
            format!("0:{index}"),
        ]);
    }
    args.extend(
        [
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-f",
            "mp4",
            "-movflags",
            "frag_keyframe+empty_moov",
            "pipe:1",
        ]
        .map(String::from),
    );
    args
}
