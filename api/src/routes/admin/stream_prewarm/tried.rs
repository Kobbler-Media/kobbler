use std::collections::HashSet;

use nats_jobs::NzbRef;

use crate::routes::admin::debrid_stream::StreamResolveBody;

/// Releases one run has already had refused. An NZB is named by its ref until an import
/// teaches it a key, and the same NZB is listed once per usenet provider, so either name
/// identifies it.
#[derive(Default)]
pub(crate) struct TriedReleases {
    keys: HashSet<String>,
    nzbs: HashSet<NzbRef>,
}

impl TriedReleases {
    pub(crate) fn add(&mut self, body: &StreamResolveBody) {
        if let Some(key) = &body.release_key {
            self.keys.insert(key.to_lowercase());
        }
        if let Some(nzb) = &body.nzb_ref {
            self.nzbs.insert(nzb.clone());
        }
    }

    pub(super) fn holds(&self, body: &StreamResolveBody) -> bool {
        body.release_key
            .as_ref()
            .is_some_and(|k| self.keys.contains(&k.to_lowercase()))
            || body.nzb_ref.as_ref().is_some_and(|n| self.nzbs.contains(n))
    }

    /// A refused NZB may be listed with the key its import learned, and that key may be
    /// listed again under another indexer's ref.
    pub(super) fn link<'a>(&mut self, listed: impl Iterator<Item = &'a StreamResolveBody>) {
        let listed: Vec<_> = listed.collect();
        // Twice, as a listing can name a release only through one later in the list.
        for body in listed.iter().chain(&listed) {
            if self.holds(body) {
                self.add(body);
            }
        }
    }
}
