//! Types used by the Kobo handler.

/// Context derived from an incoming proxy request.
///
/// This bundles commonly-needed values that are extracted/computed from the request,
/// avoiding the need to pass multiple parameters through handler functions.
pub struct RequestContext<'a> {
    /// Device token extracted from the request path
    pub device_token: &'a str,

    /// Base URL for building response URLs (preserves the port the device connected to)
    pub base_url: String,

    /// Reference to the request body, if any
    pub body: Option<&'a [u8]>,

    /// The x-kobo-synctoken header value, if present
    pub sync_token_header: Option<&'a str>,
}

/// Brief book summary for log enrichment.
#[derive(Debug, Clone)]
pub struct BookSummary {
    pub title: String,
    pub authors: String,
}

impl BookSummary {
    pub fn to_note(&self) -> String {
        if self.authors.is_empty() {
            self.title.clone()
        } else {
            format!("{} by {}", self.title, self.authors)
        }
    }
}
