use dashmap::DashMap;
use uuid::Uuid;

/// The live sessions of each debrid release file, by release key. A release can be open for
/// several clients at once when one session cannot serve them all, as a phone playing the
/// file as it is and a receiver that needs it transcoded.
#[derive(Default)]
pub struct ReleaseSessions(DashMap<String, Vec<Uuid>>);

impl ReleaseSessions {
    pub fn insert(&self, key: String, session_id: Uuid) {
        let mut ids = self.0.entry(key).or_default();
        if !ids.contains(&session_id) {
            ids.push(session_id);
        }
    }

    pub fn remove(&self, key: &str, session_id: Uuid) {
        self.0.remove_if_mut(key, |_, ids| {
            ids.retain(|id| *id != session_id);
            ids.is_empty()
        });
    }

    pub fn of(&self, key: &str) -> Vec<Uuid> {
        self.0.get(key).map(|ids| ids.clone()).unwrap_or_default()
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }
}
