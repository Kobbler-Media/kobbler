//! The shared `LibraryEvent` subscriber loop.
//!
//! No database, no HTTP: this drives `api::events::run_subscriber` directly over a
//! real broadcast channel, because the failure it guards against — a burst of events
//! silently ending a long-lived subscriber — is a property of the loop alone.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use api::events::{LibraryEvent, run_subscriber};
use tokio::sync::broadcast;
use uuid::Uuid;

macro_rules! recorder {
    ($seen:ident) => {{
        let sink = $seen.clone();
        async move |event: LibraryEvent| {
            if let LibraryEvent::AuthorUpdated { author_id } = event {
                sink.lock().unwrap().push(author_id);
            }
        }
    }};
}

async fn wait_for(seen: &Arc<Mutex<Vec<Uuid>>>, n: usize) {
    for _ in 0..200 {
        if seen.lock().unwrap().len() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_lag_does_not_end_the_subscriber() {
    let (tx, rx) = broadcast::channel::<LibraryEvent>(2);

    // Overflow before anything consumes, so the receiver's first recv is Lagged.
    for _ in 0..8 {
        let _ = tx.send(LibraryEvent::library_updated(None));
    }
    let after_the_lag = Uuid::new_v4();
    let _ = tx.send(LibraryEvent::author_updated(after_the_lag));

    let seen: Arc<Mutex<Vec<Uuid>>> = Arc::new(Mutex::new(Vec::new()));
    let handle = tokio::spawn(run_subscriber("test", rx, recorder!(seen)));

    wait_for(&seen, 1).await;
    assert_eq!(
        *seen.lock().unwrap(),
        vec![after_the_lag],
        "a lagged receiver must keep consuming: Lagged is recoverable, only Closed ends the task"
    );

    // Still live afterwards, not merely limping through one event.
    let later = Uuid::new_v4();
    let _ = tx.send(LibraryEvent::author_updated(later));
    wait_for(&seen, 2).await;
    assert_eq!(*seen.lock().unwrap(), vec![after_the_lag, later]);

    drop(tx);
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("a closed bus must end the subscriber")
        .expect("subscriber panicked");
}

#[tokio::test]
async fn events_are_handled_in_order_until_the_bus_closes() {
    let (tx, rx) = broadcast::channel::<LibraryEvent>(16);
    let ids: Vec<Uuid> = (0..5).map(|_| Uuid::new_v4()).collect();
    for id in &ids {
        let _ = tx.send(LibraryEvent::author_updated(*id));
    }

    let seen: Arc<Mutex<Vec<Uuid>>> = Arc::new(Mutex::new(Vec::new()));
    let handle = tokio::spawn(run_subscriber("test", rx, recorder!(seen)));

    wait_for(&seen, ids.len()).await;
    assert_eq!(*seen.lock().unwrap(), ids);

    drop(tx);
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("a closed bus must end the subscriber")
        .expect("subscriber panicked");
}
