use bytes::Bytes;
use std::sync::Mutex;
use tokio::sync::broadcast;

use super::keyframe_cache::KeyframeCache;

/// ~10 MB of 8 KB chunks. A direct client that falls this far behind is resynced to the
/// live edge rather than allowed to stall the tune.
const CAPACITY: usize = 1024;

/// One tuner connection, every direct viewer of it.
pub struct Fanout {
    live: broadcast::Sender<Bytes>,
    keyframes: Mutex<KeyframeCache>,
}

impl Default for Fanout {
    fn default() -> Self {
        Self {
            live: broadcast::channel(CAPACITY).0,
            keyframes: Mutex::default(),
        }
    }
}

impl Fanout {
    /// Hands the next slice of the tuner's stream to every viewer, and back to the caller
    /// cut on packet boundaries.
    pub fn send(&self, chunk: &[u8]) -> Bytes {
        let mut keyframes = self.keyframes.lock().expect("keyframe cache not poisoned");
        let packets = keyframes.push(chunk);
        if !packets.is_empty() {
            let _ = self.live.send(packets.clone());
        }
        packets
    }

    /// What a new viewer plays first, and the live stream that carries on exactly where
    /// it ends. Taken under the lock [send] holds, so nothing falls between the two.
    pub fn join(&self) -> (Bytes, broadcast::Receiver<Bytes>) {
        let keyframes = self.keyframes.lock().expect("keyframe cache not poisoned");
        (keyframes.snapshot(), self.live.subscribe())
    }
}
