//! One transcode start or restart at a time per playback session.
//!
//! Whoever starts, restarts or replaces a session's transcode holds its gate for the
//! duration, so a second starter queues behind the first and finds its work done
//! instead of starting a rival generation. Readers that find the session lifted out
//! of the map mid-restart wait on the gate rather than turning the player away.

use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{Mutex, OwnedMutexGuard};
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct SessionGates(Arc<DashMap<Uuid, Arc<Mutex<()>>>>);

pub struct SessionGateGuard {
    gates: SessionGates,
    id: Uuid,
    held: Option<OwnedMutexGuard<()>>,
}

impl SessionGates {
    pub async fn claim(&self, id: Uuid) -> SessionGateGuard {
        let gate = self.0.entry(id).or_default().clone();
        SessionGateGuard {
            gates: self.clone(),
            id,
            held: Some(gate.lock_owned().await),
        }
    }

    pub fn is_held(&self, id: Uuid) -> bool {
        self.0.get(&id).is_some_and(|g| g.try_lock().is_err())
    }

    /// Waits for whoever holds the gate to let go. False when nobody did.
    pub async fn wait(&self, id: Uuid) -> bool {
        let Some(gate) = self.0.get(&id).map(|g| g.clone()) else {
            return false;
        };
        drop(gate.lock().await);
        drop(gate);
        self.prune(id);
        true
    }

    // Clones are only taken under the shard lock `remove_if` holds, so a count of one
    // means no claimer or waiter can still reach this gate.
    fn prune(&self, id: Uuid) {
        self.0.remove_if(&id, |_, g| Arc::strong_count(g) == 1);
    }
}

impl Drop for SessionGateGuard {
    fn drop(&mut self) {
        drop(self.held.take());
        self.gates.prune(self.id);
    }
}
