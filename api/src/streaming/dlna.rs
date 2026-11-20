//! The DLNA response headers a renderer asks for before it decides how to play,
//! seek and pause a resource.

use axum::http::{HeaderMap, HeaderName, HeaderValue};

const CONTENT_FEATURES: HeaderName = HeaderName::from_static("contentfeatures.dlna.org");
const GET_CONTENT_FEATURES: &str = "getcontentfeatures.dlna.org";
const TRANSFER_MODE: HeaderName = HeaderName::from_static("transfermode.dlna.org");

/// Streaming transfer, background transfer, connection stall, DLNA 1.5.
const FLAGS: &str = "01700000000000000000000000000000";

#[derive(Debug, Clone, Copy)]
pub enum Resource {
    /// The file as stored: ranges answered, so the renderer may seek by byte.
    Original,
    /// Made on the fly, no ranges: a seek is a new session.
    Transcoded,
}

impl Resource {
    fn content_features(self) -> String {
        let (op, ci) = match self {
            Self::Original => ("01", 0),
            Self::Transcoded => ("00", 1),
        };
        format!("DLNA.ORG_OP={op};DLNA.ORG_CI={ci};DLNA.ORG_FLAGS={FLAGS}")
    }
}

pub fn add_headers(response: &mut HeaderMap, request: &HeaderMap, resource: Resource) {
    let requested_mode = request
        .get(&TRANSFER_MODE)
        .filter(|m| {
            ["Streaming", "Interactive", "Background"]
                .iter()
                .any(|known| m.as_bytes().eq_ignore_ascii_case(known.as_bytes()))
        })
        .cloned();
    response.insert(
        TRANSFER_MODE,
        requested_mode.unwrap_or(HeaderValue::from_static("Streaming")),
    );
    if request
        .get(GET_CONTENT_FEATURES)
        .is_some_and(|v| v.as_bytes() == b"1")
    {
        response.insert(
            CONTENT_FEATURES,
            HeaderValue::from_str(&resource.content_features()).unwrap(),
        );
    }
}
