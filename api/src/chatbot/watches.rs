//! Progress reporting: the part that happens after the turn has already answered.
//!
//! The webhook has ~30 s. A download has minutes to hours. So nothing here runs inside
//! the agent loop — a long-lived task subscribes to the `LibraryEvent` broadcast, joins
//! events against `chat_watches`, and patches the message the act tool already posted.
//!
//! Two rules the rest of this module exists to keep:
//!
//! - **At most one edit per [`COALESCE_WINDOW`] per request.** `KobloadDownloadUpdated`
//!   fires far more often than Google will let you patch a message. Claiming the slot
//!   is a conditional `UPDATE`, so it survives a restart — and it is claimed for every
//!   watch on the same request in the same thread at once, because a season pack is one
//!   download whatever the row count says. Thirteen episodes are thirteen rows, one
//!   message.
//! - **Every watch ends with something said.** Success, failure or expiry — a watch that
//!   goes quiet is the failure mode this whole phase exists to prevent.
//!
//! Terminal verdicts come only from [`LibraryEvent::WantedStatusUpdated`], which is
//! kobbler's own view of the wanted item. Kobload's stream drives progress and nothing
//! else: a download client reporting `failed` is not the same as kobbler giving up, and
//! the expiry sweep is the backstop for a want that never resolves either way.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use sqlx::PgPool;
use tokio::sync::broadcast;
use tracing::{info, warn};

use time::OffsetDateTime;

use super::transport::{ChatTransport, ChatTransportRegistry, OutMessage, config};
use crate::events::LibraryEvent;
use crate::notifications::prefs;
use crate::repositories::chat_notification_prefs;
use crate::repositories::chat_watches::{self, ChatWatch};

pub const COALESCE_WINDOW: Duration = Duration::from_secs(15);

/// Transports that cannot edit get a wider window on top of the transitions-only rule,
/// because every update they deliver is a new message in the thread.
const POST_ONLY_WIDENING: u32 = 4;

const SWEEP_BATCH: i64 = 500;

/// How long a decided-but-undelivered verdict keeps being retried. Long enough to
/// outlast a night of quiet hours or a mute-until-tomorrow, and short enough that a
/// permanently dead space stops being called.
const TERMINAL_RETRY_WINDOW: Duration = Duration::from_secs(12 * 3600);

const TERMINAL_RETRY_BACKOFF: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepOutcome {
    /// Verdicts that had failed to land and did this time.
    pub retried: usize,
    /// Watches that ran out of time without a verdict.
    pub expired: usize,
    /// Verdicts nobody was ever told, given up on.
    pub abandoned: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Delivery {
    Sent,
    /// The requester has standing instructions not to hear this.
    Refused,
    /// Held back by a mute or quiet hours; owed later, not dropped.
    Deferred,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Permission {
    Send,
    Refuse,
    Defer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Searching,
    Grabbing,
    Downloading,
    Importing,
}

impl Stage {
    fn key(self) -> &'static str {
        match self {
            Self::Searching => "searching",
            Self::Grabbing => "grabbing",
            Self::Downloading => "downloading",
            Self::Importing => "importing",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Terminal {
    Done,
    Failed,
    NeedsReview,
    /// Stopped on purpose. Distinct from `Failed` because she is the one who
    /// stopped it, and "that didn't work" reads as a bug in what she just did.
    Cancelled,
    Expired,
}

impl Terminal {
    fn from_key(key: &str) -> Option<Self> {
        match key {
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            "needs_review" => Some(Self::NeedsReview),
            "cancelled" => Some(Self::Cancelled),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }
}

/// The verdict a claimed watch is still trying to deliver. A row can only carry a
/// terminal `last_state` by having been claimed with one, so the fallback never fires
/// in practice — and if it somehow did, expiry is the honest thing to say.
fn terminal_of(watch: &ChatWatch) -> Terminal {
    watch
        .last_state
        .as_deref()
        .and_then(Terminal::from_key)
        .unwrap_or(Terminal::Expired)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Update {
    Progress {
        stage: Stage,
        detail: Option<String>,
    },
    Terminal(Terminal),
}

impl Update {
    fn progress(stage: Stage) -> Self {
        Self::Progress {
            stage,
            detail: None,
        }
    }

    fn with_detail(stage: Stage, detail: Option<String>) -> Self {
        Self::Progress { stage, detail }
    }

    fn terminal(&self) -> Option<Terminal> {
        match self {
            Self::Terminal(t) => Some(*t),
            Self::Progress { .. } => None,
        }
    }

    fn state_key(&self) -> &'static str {
        match self {
            Self::Progress { stage, .. } => stage.key(),
            Self::Terminal(Terminal::Done) => "done",
            Self::Terminal(Terminal::Failed) => "failed",
            Self::Terminal(Terminal::NeedsReview) => "needs_review",
            Self::Terminal(Terminal::Cancelled) => "cancelled",
            Self::Terminal(Terminal::Expired) => "expired",
        }
    }

    fn render(&self, label: &str) -> String {
        match self {
            Self::Progress {
                stage: Stage::Searching,
                ..
            } => format!("Searching for {label}…"),
            Self::Progress {
                stage: Stage::Grabbing,
                detail,
            } => match detail {
                Some(release) => format!("Grabbing {label} — {release}"),
                None => format!("Found something for {label}, grabbing it now."),
            },
            Self::Progress {
                stage: Stage::Downloading,
                detail,
            } => match detail {
                Some(pct) => format!("Downloading {label} — {pct}"),
                None => format!("Downloading {label}…"),
            },
            Self::Progress {
                stage: Stage::Importing,
                ..
            } => format!("Downloaded {label}, adding it to the library…"),
            Self::Terminal(Terminal::Done) => format!("Done, {label} is in the library."),
            Self::Terminal(Terminal::Failed) => {
                format!(
                    "That didn't work — I couldn't get {label}, and nothing's downloading for it now."
                )
            }
            Self::Terminal(Terminal::NeedsReview) => {
                format!(
                    "{label} downloaded, but the files need a look before it lands — it's waiting in Downloads."
                )
            }
            Self::Terminal(Terminal::Cancelled) => {
                format!("Stopped — I'm not getting {label} any more.")
            }
            Self::Terminal(Terminal::Expired) => {
                format!(
                    "I've stopped watching {label} — no news for a long while, so I won't keep you hanging. Downloads will say more if it's still going."
                )
            }
        }
    }
}

/// Kobbler's own verdict on a wanted item. This is the only source of a terminal state.
fn wanted_update(status: &str) -> Option<Update> {
    let update = match status {
        "pending" | "wanted" | "searching" => Update::progress(Stage::Searching),
        "queued" | "grabbed" | "snatched" => Update::progress(Stage::Grabbing),
        "downloading" | "watching" => Update::progress(Stage::Downloading),
        "ingesting" | "processing" => Update::progress(Stage::Importing),
        "acquired" | "completed" | "complete" | "downloaded" | "imported" => {
            Update::Terminal(Terminal::Done)
        }
        "failed" | "search_failed" | "kobload_error" | "error" => {
            Update::Terminal(Terminal::Failed)
        }
        "cancelled" => Update::Terminal(Terminal::Cancelled),
        "needs_review" => Update::Terminal(Terminal::NeedsReview),
        _ => return None,
    };
    Some(update)
}

/// Progress only. A Kobload status never ends a watch — see the module header.
fn download_update(download: &serde_json::Value) -> Option<(String, Update)> {
    let external_id = download.get("id")?.as_str()?.to_string();
    let status = download.get("status")?.as_str()?;

    let update = match status {
        "searching" => Update::progress(Stage::Searching),
        "pending" | "queued" => Update::with_detail(
            Stage::Grabbing,
            download
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        ),
        "downloading" => Update::with_detail(
            Stage::Downloading,
            download
                .get("progress")
                .and_then(|v| v.as_f64())
                .map(|pct| format!("{}%", pct.clamp(0.0, 100.0).round())),
        ),
        "processing" | "completed" | "imported" => Update::progress(Stage::Importing),
        _ => return None,
    };

    Some((external_id, update))
}

/// Where a live [`ChatTransport`] comes from. Behind a trait so tests can record
/// `post`/`edit` calls without a Google credential.
#[async_trait]
pub trait TransportSource: Send + Sync {
    async fn get(&self, transport: &str) -> Option<Arc<dyn ChatTransport>>;
}

pub struct DbTransports {
    pool: PgPool,
    registry: ChatTransportRegistry,
}

impl DbTransports {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            registry: ChatTransportRegistry::new(),
        }
    }
}

#[async_trait]
impl TransportSource for DbTransports {
    async fn get(&self, transport: &str) -> Option<Arc<dyn ChatTransport>> {
        let config = match config::load(&self.pool, transport).await {
            Ok(config) => config,
            Err(e) => {
                warn!(transport, error = %e, "No chat transport config for a live watch");
                return None;
            }
        };
        self.registry.create(transport, &config).map(Arc::from)
    }
}

/// The instant [`WatchDispatcher::permission`] judges wall-clock preferences at.
///
/// Quiet hours are the only behaviour in this module that depends on what time it
/// is — so without a seam, whether a watch delivers or defers depends on the hour the
/// test suite happens to run at, and a green run before dinner says nothing about the
/// same code after it. Production is always [`Clock::System`]; [`Clock::pinned`] is
/// how a test owns the night.
#[derive(Clone)]
pub enum Clock {
    System,
    /// Shared so one dispatcher can be walked from midnight to morning.
    Pinned(Arc<Mutex<OffsetDateTime>>),
}

impl Clock {
    pub fn pinned(at: OffsetDateTime) -> Self {
        Self::Pinned(Arc::new(Mutex::new(at)))
    }

    pub fn advance_to(&self, at: OffsetDateTime) {
        if let Self::Pinned(cell) = self {
            *cell.lock().expect("clock not poisoned") = at;
        }
    }

    fn now(&self) -> OffsetDateTime {
        match self {
            Self::System => OffsetDateTime::now_utc(),
            Self::Pinned(cell) => *cell.lock().expect("clock not poisoned"),
        }
    }
}

pub struct WatchDispatcher {
    pool: PgPool,
    transports: Arc<dyn TransportSource>,
    coalesce: Duration,
    clock: Clock,
}

impl WatchDispatcher {
    pub fn new(pool: PgPool) -> Self {
        let transports = Arc::new(DbTransports::new(pool.clone()));
        Self {
            pool,
            transports,
            coalesce: COALESCE_WINDOW,
            clock: Clock::System,
        }
    }

    /// Injectable window, transport source and clock, so a test proves coalescing
    /// without sitting through fifteen real seconds and proves quiet hours without
    /// waiting for one.
    pub fn with_transports(
        pool: PgPool,
        transports: Arc<dyn TransportSource>,
        coalesce: Duration,
        clock: Clock,
    ) -> Self {
        Self {
            pool,
            transports,
            coalesce,
            clock,
        }
    }

    pub async fn handle_event(&self, event: &LibraryEvent) {
        match event {
            // `announce` is the notification dispatcher's business, not this one's:
            // a watch is a reply to something she asked for by name.
            LibraryEvent::WantedStatusUpdated {
                wanted_id, status, ..
            } => {
                let Some(update) = wanted_update(status) else {
                    return;
                };
                match chat_watches::for_entities(&self.pool, &[*wanted_id]).await {
                    Ok(watches) => self.deliver_all(watches, &update).await,
                    Err(e) => warn!(error = %e, "Failed to load chat watches for a wanted update"),
                }
            }
            LibraryEvent::KobloadDownloadCreated { download }
            | LibraryEvent::KobloadDownloadUpdated { download } => {
                let Some((external_id, update)) = download_update(download) else {
                    return;
                };
                match chat_watches::for_download(&self.pool, &external_id).await {
                    Ok(watches) => self.deliver_all(watches, &update).await,
                    Err(e) => {
                        warn!(error = %e, "Failed to load chat watches for a download update")
                    }
                }
            }
            _ => {}
        }
    }

    /// Retry undelivered verdicts, then close off watches that ran out of time.
    ///
    /// Retries come first on purpose: a watch whose verdict is already decided is
    /// owed *that* message, not an apology for going quiet.
    pub async fn sweep(&self) -> SweepOutcome {
        let mut outcome = SweepOutcome::default();

        match chat_watches::claim_terminal_retries(&self.pool, SWEEP_BATCH, TERMINAL_RETRY_BACKOFF)
            .await
        {
            Ok(rows) => {
                for watch in &rows {
                    if self.send_terminal(watch, terminal_of(watch)).await {
                        outcome.retried += 1;
                    }
                }
            }
            Err(e) => warn!(error = %e, "Failed to load undelivered chat watch verdicts"),
        }

        match chat_watches::claim_expired(&self.pool, SWEEP_BATCH, TERMINAL_RETRY_WINDOW).await {
            Ok(rows) => {
                outcome.expired = rows.len();
                for watch in &rows {
                    self.send_terminal(watch, Terminal::Expired).await;
                }
            }
            Err(e) => warn!(error = %e, "Failed to sweep expired chat watches"),
        }

        match chat_watches::abandon_undelivered(&self.pool, SWEEP_BATCH).await {
            Ok(rows) => {
                outcome.abandoned = rows.len();
                for watch in &rows {
                    warn!(
                        watch_id = %watch.id,
                        transport = %watch.transport,
                        label = %watch.label,
                        verdict = watch.last_state.as_deref().unwrap_or("unknown"),
                        "Giving up on a chat watch's final message; nobody was told",
                    );
                }
            }
            Err(e) => warn!(error = %e, "Failed to abandon undelivered chat watch verdicts"),
        }

        outcome
    }

    async fn deliver_all(&self, watches: Vec<ChatWatch>, update: &Update) {
        for watch in watches {
            self.deliver(&watch, update).await;
        }
    }

    async fn deliver(&self, watch: &ChatWatch, update: &Update) {
        // A watch carrying a verdict is finished; the sweep owns what happens to it.
        if chat_watches::is_terminal_state(watch.last_state.as_deref()) {
            return;
        }

        if let Some(terminal) = update.terminal() {
            match chat_watches::claim_terminal_group(
                &self.pool,
                watch.id,
                update.state_key(),
                TERMINAL_RETRY_WINDOW,
            )
            .await
            {
                Ok(true) => {}
                // Someone else already reached a verdict for this watch, or a
                // sibling watching the same request has already spoken for it.
                Ok(false) => return,
                Err(e) => {
                    warn!(error = %e, "Failed to claim a chat watch for its final message");
                    return;
                }
            }
            self.send_terminal(watch, terminal).await;
            return;
        }

        let Some(transport) = self.transports.get(&watch.transport).await else {
            return;
        };
        let edits = transport.capabilities().edit_in_place;

        let changed = watch.last_state.as_deref() != Some(update.state_key());
        // A transport that can only post says nothing about a percentage moving.
        if !changed && !edits {
            return;
        }
        let window = if edits {
            self.coalesce
        } else {
            self.coalesce * POST_ONLY_WIDENING
        };
        let slot = match chat_watches::claim_edit_group(&self.pool, watch.id, window).await {
            Ok(Some(slot)) => slot,
            Ok(None) => return,
            Err(e) => {
                warn!(error = %e, "Failed to claim the coalescing slot for a chat watch");
                return;
            }
        };

        self.send_with(&*transport, watch, update, slot.message_id.as_deref())
            .await;
    }

    /// Send a claimed verdict and drop the watch once it has landed. A row left
    /// behind is retried by the next sweep — including one held back by quiet hours,
    /// which is a deferral rather than a refusal.
    async fn send_terminal(&self, watch: &ChatWatch, terminal: Terminal) -> bool {
        let update = Update::Terminal(terminal);
        let Some(transport) = self.transports.get(&watch.transport).await else {
            return false;
        };

        match self
            .send_with(&*transport, watch, &update, watch.message_id.as_deref())
            .await
        {
            delivery @ (Delivery::Sent | Delivery::Refused) => {
                if let Err(e) = chat_watches::take(&self.pool, watch.id).await {
                    warn!(error = %e, "Failed to close a chat watch after its final message");
                }
                delivery == Delivery::Sent
            }
            Delivery::Deferred | Delivery::Failed => false,
        }
    }

    /// `message_id` is the message this request is already showing in the thread
    /// — the group's, not the row's, because a season's episodes are all watching
    /// the one message.
    async fn send_with(
        &self,
        transport: &dyn ChatTransport,
        watch: &ChatWatch,
        update: &Update,
        message_id: Option<&str>,
    ) -> Delivery {
        match self.permission(watch, update).await {
            Permission::Send => {}
            Permission::Refuse => return Delivery::Refused,
            Permission::Defer => return Delivery::Deferred,
        }

        let edits = transport.capabilities().edit_in_place;
        let msg = OutMessage::text(update.render(&watch.label));

        let posted = match (message_id, edits) {
            (Some(message_id), true) => transport
                .edit(&watch.target, message_id, &msg)
                .await
                .map(|()| None),
            _ => transport.post(&watch.target, &msg).await.map(Some),
        };

        match posted {
            Ok(new_message_id) => {
                if update.terminal().is_none() {
                    // Only worth remembering when there is something to edit later.
                    let store = if edits {
                        new_message_id.as_deref()
                    } else {
                        None
                    };
                    if let Err(e) = chat_watches::record_state_group(
                        &self.pool,
                        watch.id,
                        update.state_key(),
                        store,
                    )
                    .await
                    {
                        warn!(error = %e, "Failed to record chat watch state");
                    }
                }
                Delivery::Sent
            }
            Err(e) => {
                warn!(
                    transport = %watch.transport,
                    terminal = update.terminal().is_some(),
                    error = %e,
                    "Chat watch update could not be delivered",
                );
                Delivery::Failed
            }
        }
    }

    /// What the requester has said they want, via Agent G's preference gate.
    ///
    /// **`triggers` is deliberately not consulted here.** That list defends against
    /// unsolicited noise, and a watch is not unsolicited: she asked for this thing by
    /// name in this thread thirty seconds ago and is looking at the message. "Stop
    /// telling me about downloads" means stop volunteering things, not stop answering
    /// me. The relevance-scoped fan-out is the path that genuinely is unsolicited, and
    /// it keeps its trigger check.
    ///
    /// What is honoured: `enabled = false` is a standing "stop talking to me" and
    /// outranks everything. A mute or quiet hours is a time-bounded "not now", so a
    /// verdict is held for later rather than lost — while a progress edit is simply
    /// dropped, since by morning it is stale and the verdict has superseded it.
    async fn permission(&self, watch: &ChatWatch, update: &Update) -> Permission {
        let Some(user_id) = watch.user_id else {
            return Permission::Send;
        };

        let prefs = match chat_notification_prefs::get(&self.pool, user_id).await {
            Ok(prefs) => prefs,
            Err(e) => {
                warn!(error = %e, "Failed to read chat notification prefs; sending anyway");
                return Permission::Send;
            }
        };

        if !prefs.enabled {
            return Permission::Refuse;
        }

        let now = self.clock.now();
        let local_now = prefs::zone_for_user(&self.pool, user_id)
            .await
            .local_time(now);
        let muted = prefs.muted_until.is_some_and(|until| until > now);
        let quiet = prefs
            .quiet_window()
            .is_some_and(|(from, to)| prefs::in_quiet_hours(from, to, local_now));

        if muted || quiet {
            if update.terminal().is_some() {
                Permission::Defer
            } else {
                Permission::Refuse
            }
        } else {
            Permission::Send
        }
    }
}

/// Long-lived subscriber over the `LibraryEvent` broadcast.
///
/// Events are handled in order rather than fanned out, so a completion can never
/// overtake the progress update before it. A lagged receiver costs dropped progress
/// pings — the next event re-states the stage — and never ends the task; only a closed
/// bus does, which happens at shutdown.
pub async fn run_subscriber(
    dispatcher: WatchDispatcher,
    mut rx: broadcast::Receiver<LibraryEvent>,
) {
    loop {
        match rx.recv().await {
            Ok(event) => dispatcher.handle_event(&event).await,
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                warn!(
                    skipped,
                    "Chat watch subscriber fell behind the event bus; progress pings dropped"
                );
            }
            Err(broadcast::error::RecvError::Closed) => {
                info!("Library event bus closed, chat watch subscriber stopping");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_kobbler_decides_a_watch_is_over() {
        assert_eq!(
            wanted_update("acquired"),
            Some(Update::Terminal(Terminal::Done))
        );
        assert_eq!(
            wanted_update("search_failed"),
            Some(Update::Terminal(Terminal::Failed))
        );
        assert!(wanted_update("something_new").is_none());

        // Kobload reporting a failure is not kobbler giving up, so it says nothing.
        assert!(download_update(&json!({"id": "d", "status": "failed"})).is_none());
        assert!(download_update(&json!({"id": "d", "status": "needs_review"})).is_none());
    }

    #[test]
    fn download_progress_reads_as_a_percentage() {
        let (id, update) = download_update(&json!({
            "id": "abc",
            "status": "downloading",
            "progress": 34.4,
        }))
        .expect("mapped");
        assert_eq!(id, "abc");
        assert_eq!(update.state_key(), "downloading");
        assert_eq!(
            update.render("someShow S24E07"),
            "Downloading someShow S24E07 — 34%"
        );
    }

    #[test]
    fn the_grab_names_the_release() {
        let (_, update) = download_update(&json!({
            "id": "abc",
            "status": "queued",
            "name": "someShow.S24E07.1080p.WEB-DL",
        }))
        .expect("mapped");
        assert_eq!(
            update.render("someShow S24E07"),
            "Grabbing someShow S24E07 — someShow.S24E07.1080p.WEB-DL"
        );
    }

    #[test]
    fn every_terminal_names_the_thing_and_says_it_is_over() {
        for terminal in [
            Terminal::Done,
            Terminal::Failed,
            Terminal::NeedsReview,
            Terminal::Expired,
        ] {
            let text = Update::Terminal(terminal).render("Mistborn");
            assert!(text.contains("Mistborn"), "{terminal:?} lost the label");
        }
    }
}
