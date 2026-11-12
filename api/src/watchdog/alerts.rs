use super::{Incident, IncidentLog};
use crate::DbPool;
use crate::notifications::{NotificationPayload, NotificationTrigger, notify_agents};
use tokio::sync::mpsc::UnboundedReceiver;

#[derive(Debug, Clone)]
pub enum Alert {
    Ongoing(Incident),
    Recovered(Incident),
    RestartedAfterKill(Incident),
}

impl Alert {
    fn title(&self) -> &'static str {
        match self {
            Self::Ongoing(_) => "Kobbler is stalling",
            Self::Recovered(_) => "Kobbler stalled and recovered",
            Self::RestartedAfterKill(_) => "Kobbler was restarted by its watchdog",
        }
    }

    fn incident(&self) -> &Incident {
        match self {
            Self::Ongoing(i) | Self::Recovered(i) | Self::RestartedAfterKill(i) => i,
        }
    }
}

/// Forwards watchdog alerts to the notification agents, starting with any kill the previous
/// process recorded before it went down.
pub async fn send_alerts(
    pool: DbPool,
    publisher: nats_jobs::Publisher,
    log: IncidentLog,
    mut alerts: UnboundedReceiver<Alert>,
) {
    if let Some(killed) = log.take_pending_kill() {
        send(&pool, &publisher, &Alert::RestartedAfterKill(killed)).await;
    }
    while let Some(alert) = alerts.recv().await {
        send(&pool, &publisher, &alert).await;
    }
}

async fn send(pool: &DbPool, publisher: &nats_jobs::Publisher, alert: &Alert) {
    let message = format!(
        "{}. History: watchdog/incidents.jsonl under the storage root, and `[watchdog]` lines in the API log.",
        alert.incident().summary()
    );
    notify_agents(
        pool,
        publisher,
        &NotificationPayload {
            trigger: NotificationTrigger::OnServerHealth,
            title: alert.title().to_string(),
            message,
            entity_type: None,
            entity_id: None,
            entity_name: None,
            url: None,
            public_url: None,
            show_id: None,
            show_name: None,
            season_number: None,
            show_year: None,
            upgraded: false,
            details: None,
        },
    )
    .await;
}
