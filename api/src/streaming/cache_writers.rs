use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use dashmap::DashMap;
use tokio::sync::{Mutex, OwnedMutexGuard};

/// A cancelled download can still be inside a write when its restart opens the same file,
/// and both appending duplicates bytes; the restart waits for the old writer to let go.
static WRITERS: LazyLock<DashMap<PathBuf, Arc<Mutex<()>>>> = LazyLock::new(DashMap::new);

pub struct Writer {
    path: PathBuf,
    guard: Option<OwnedMutexGuard<()>>,
}

pub async fn lock(path: &Path) -> Writer {
    let mutex = WRITERS.entry(path.to_path_buf()).or_default().clone();
    Writer {
        path: path.to_path_buf(),
        guard: Some(mutex.lock_owned().await),
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        drop(self.guard.take());
        WRITERS.remove_if(&self.path, |_, m| Arc::strong_count(m) == 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_second_writer_waits_for_the_first_and_nothing_is_left_behind() {
        let path = Path::new("/cache/writers-test/film.mkv");
        let first = lock(path).await;
        let second = tokio::spawn(async move { lock(path).await });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!second.is_finished());
        drop(first);
        drop(second.await.unwrap());
        assert!(!WRITERS.contains_key(path));
    }
}
