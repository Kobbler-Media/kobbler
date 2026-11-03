use std::time::Duration;

use async_nats::jetstream::consumer::pull;
use futures::StreamExt;
use nats_jobs::{Event, NatsClient};
use tracing::{error, info};

const FIRST_RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(30);

pub(crate) struct EventConsumer {
    pub durable: &'static str,
    pub filter_subject: &'static str,
    pub label: &'static str,
}

struct Backoff(Duration);

impl Backoff {
    fn reset(&mut self) {
        self.0 = FIRST_RETRY;
    }

    async fn wait(&mut self) {
        tokio::time::sleep(self.0).await;
        self.0 = (self.0 * 2).min(MAX_RETRY);
    }
}

// Every message is acked once `handle` returns, an undecodable one included, so a bad
// event is never redelivered. The consumer is recreated after a reconnect because the
// server may have lost its state. `handle` returns an owned future, not AsyncFnMut,
// whose borrowing future fails "Send is not general enough" under tokio::spawn.
pub(crate) async fn consume_events<F, Fut>(nats: NatsClient, consumer: EventConsumer, mut handle: F)
where
    F: FnMut(Event) -> Fut,
    Fut: Future<Output = ()>,
{
    let EventConsumer {
        durable,
        filter_subject,
        label,
    } = consumer;
    let config = pull::Config {
        durable_name: Some(durable.to_string()),
        filter_subject: filter_subject.to_string(),
        ..Default::default()
    };
    let mut reconnect_rx = nats.subscribe_reconnect();
    let mut backoff = Backoff(FIRST_RETRY);

    loop {
        let mut messages = match open(&nats, durable, &config, label).await {
            Ok(messages) => messages,
            Err(failure) => {
                error!("{failure}, retrying in {:?}", backoff.0);
                backoff.wait().await;
                continue;
            }
        };
        info!("Subscribed to {label} events from Kobload");
        backoff.reset();

        loop {
            tokio::select! {
                _ = reconnect_rx.recv() => {
                    info!("{label} consumer received reconnect signal, recreating consumer");
                    break;
                }
                next = messages.next() => match next {
                    Some(Ok(msg)) => {
                        match serde_json::from_slice::<Event>(&msg.payload) {
                            Ok(event) => handle(event).await,
                            Err(e) => error!("Failed to deserialize {label} event: {e}"),
                        }
                        if let Err(e) = msg.ack().await {
                            error!("Failed to ack {label} event: {e}");
                        }
                    }
                    Some(Err(e)) => {
                        error!("Error receiving {label} event: {e}, reconnecting");
                        backoff.wait().await;
                        break;
                    }
                    None => {
                        error!("{label} events stream ended, reconnecting");
                        backoff.wait().await;
                        break;
                    }
                },
            }
        }
    }
}

async fn open(
    nats: &NatsClient,
    durable: &str,
    config: &pull::Config,
    label: &str,
) -> Result<pull::Stream, String> {
    let stream = nats
        .jetstream()
        .get_stream("EVENTS")
        .await
        .map_err(|e| format!("Failed to get EVENTS stream for {label} events: {e}"))?;
    let consumer = stream
        .get_or_create_consumer(durable, config.clone())
        .await
        .map_err(|e| format!("Failed to create {label} events consumer: {e}"))?;
    consumer
        .messages()
        .await
        .map_err(|e| format!("Failed to get {label} messages stream: {e}"))
}
