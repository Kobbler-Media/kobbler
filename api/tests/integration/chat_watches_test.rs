//! Progress reporting: coalescing, terminal messages, expiry and fan-out.
//!
//! The transport is a fake that records every `post`/`edit`. Nothing here talks to
//! Google, and the coalescing window is injected so no test waits fifteen seconds.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use crate::common::*;
use api::chatbot::transport::{
    Capabilities, ChatConfigField, ChatError, ChatTransport, ChatTransportInfo, Inbound,
    OutMessage, Target,
};
use api::chatbot::watches::{Clock, TransportSource, WatchDispatcher, run_subscriber};
use api::events::LibraryEvent;
use api::notifications::{NotificationTrigger, prefs};
use api::repositories::chat_notification_prefs::{self, ChatNotificationPrefs};
use api::repositories::chat_watches::{self, EntityKind, NewWatch};
use async_trait::async_trait;
use axum::http::HeaderMap;
use serde_json::json;
use sqlx::PgPool;
use time::{Duration as TimeDuration, OffsetDateTime, Time, UtcOffset};
use uuid::Uuid;

// =============================================================================
// A transport that only remembers what it was asked to say
// =============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Post { space: String, text: String },
    Edit { message_id: String, text: String },
}

impl Call {
    fn text(&self) -> &str {
        match self {
            Call::Post { text, .. } | Call::Edit { text, .. } => text,
        }
    }

    fn space(&self) -> Option<&str> {
        match self {
            Call::Post { space, .. } => Some(space),
            Call::Edit { .. } => None,
        }
    }
}

static FAKE_INFO: ChatTransportInfo = ChatTransportInfo {
    id: "fake",
    display_name: "Fake",
    description: "records calls",
    config_fields: &[] as &[ChatConfigField],
    capabilities: Capabilities {
        edit_in_place: true,
        cards: false,
        buttons: false,
        files: false,
    },
};

#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<Call>>,
    fail_remaining: Mutex<usize>,
}

impl Recorder {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    fn fail_next(&self, n: usize) {
        *self.fail_remaining.lock().unwrap() = n;
    }

    /// Returns true when this attempt should fail — and records nothing, exactly as a
    /// transport that never delivered would not.
    fn should_fail(&self) -> bool {
        let mut remaining = self.fail_remaining.lock().unwrap();
        if *remaining == 0 {
            return false;
        }
        *remaining -= 1;
        true
    }

    fn texts(&self) -> Vec<String> {
        self.calls()
            .iter()
            .map(|c| c.text().to_string())
            .collect::<Vec<_>>()
    }
}

struct FakeTransport {
    recorder: Arc<Recorder>,
    edit_in_place: bool,
}

#[async_trait]
impl ChatTransport for FakeTransport {
    fn info(&self) -> &'static ChatTransportInfo {
        &FAKE_INFO
    }

    async fn verify(
        &self,
        _headers: &HeaderMap,
        _body: &[u8],
    ) -> Result<Option<Inbound>, ChatError> {
        Ok(None)
    }

    async fn post(&self, target: &Target, msg: &OutMessage) -> Result<String, ChatError> {
        if self.recorder.should_fail() {
            return Err(ChatError::Transport("space is on fire".to_string()));
        }
        let mut calls = self.recorder.calls.lock().unwrap();
        calls.push(Call::Post {
            space: target.space.clone(),
            text: msg.plain_text(),
        });
        Ok(format!("messages/{}", calls.len()))
    }

    async fn edit(
        &self,
        _target: &Target,
        message_id: &str,
        msg: &OutMessage,
    ) -> Result<(), ChatError> {
        if self.recorder.should_fail() {
            return Err(ChatError::Transport("space is on fire".to_string()));
        }
        self.recorder.calls.lock().unwrap().push(Call::Edit {
            message_id: message_id.to_string(),
            text: msg.plain_text(),
        });
        Ok(())
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            edit_in_place: self.edit_in_place,
            ..Capabilities::default()
        }
    }
}

struct FakeTransports {
    recorder: Arc<Recorder>,
    edit_in_place: bool,
}

#[async_trait]
impl TransportSource for FakeTransports {
    async fn get(&self, _transport: &str) -> Option<Arc<dyn ChatTransport>> {
        Some(Arc::new(FakeTransport {
            recorder: self.recorder.clone(),
            edit_in_place: self.edit_in_place,
        }))
    }
}

// =============================================================================
// Fixtures
// =============================================================================

struct Harness {
    dispatcher: WatchDispatcher,
    recorder: Arc<Recorder>,
    clock: Clock,
}

fn harness(pool: &PgPool, coalesce: Duration, edit_in_place: bool) -> Harness {
    harness_at(pool, coalesce, edit_in_place, Clock::System)
}

/// The dispatcher with its clock pinned, so a test that touches the notification
/// gate reads the same clock every run rather than the wall clock.
fn harness_at(pool: &PgPool, coalesce: Duration, edit_in_place: bool, clock: Clock) -> Harness {
    let recorder = Arc::new(Recorder::default());
    let transports = Arc::new(FakeTransports {
        recorder: recorder.clone(),
        edit_in_place,
    });
    Harness {
        dispatcher: WatchDispatcher::with_transports(
            pool.clone(),
            transports,
            coalesce,
            clock.clone(),
        ),
        recorder,
        clock,
    }
}

/// A wall-clock time today in the server's zone, as the UTC instant it happens at.
/// The watch users have no timezone of their own, so the gate reads them against the
/// server's zone — which is the clock these times have to be built in.
fn local_at(hour: u8, minute: u8) -> OffsetDateTime {
    let offset = prefs::server_local_offset();
    OffsetDateTime::now_utc()
        .to_offset(offset)
        .replace_time(Time::from_hms(hour, minute, 0).expect("valid wall clock"))
        .to_offset(UtcOffset::UTC)
}

/// Mid-afternoon: outside any window these tests choose, in either direction.
fn daytime() -> Clock {
    Clock::pinned(local_at(14, 0))
}

fn target(space: &str) -> Target {
    Target {
        transport: "fake".to_string(),
        space: space.to_string(),
        thread: Some(format!("{space}/threads/1")),
    }
}

async fn register(pool: &PgPool, target: &Target, entity_id: Uuid, ttl: Duration) -> Uuid {
    register_with_message(pool, target, entity_id, Some("messages/original"), ttl).await
}

async fn register_with_message(
    pool: &PgPool,
    target: &Target,
    entity_id: Uuid,
    message_id: Option<&str>,
    ttl: Duration,
) -> Uuid {
    register_full(pool, target, entity_id, message_id, None, ttl).await
}

async fn register_for_user(pool: &PgPool, target: &Target, entity_id: Uuid, user_id: Uuid) -> Uuid {
    register_full(
        pool,
        target,
        entity_id,
        Some("messages/original"),
        Some(user_id),
        Duration::from_secs(3600),
    )
    .await
}

async fn register_full(
    pool: &PgPool,
    target: &Target,
    entity_id: Uuid,
    message_id: Option<&str>,
    user_id: Option<Uuid>,
    ttl: Duration,
) -> Uuid {
    register_labelled(
        pool,
        target,
        entity_id,
        message_id,
        user_id,
        ttl,
        "someShow S24E07",
    )
    .await
}

/// A season want registers one watch per episode, every one of them carrying the
/// *season's* label — so the label is what makes them siblings.
async fn register_labelled(
    pool: &PgPool,
    target: &Target,
    entity_id: Uuid,
    message_id: Option<&str>,
    user_id: Option<Uuid>,
    ttl: Duration,
    label: &str,
) -> Uuid {
    chat_watches::watch(
        pool,
        NewWatch {
            target,
            message_id,
            entity_kind: EntityKind::Episode,
            entity_id,
            label,
            user_id,
            ttl,
        },
    )
    .await
    .expect("watch registered")
}

/// The retry backoff has elapsed. Cheaper and more deterministic than waiting for it.
async fn age_last_attempt(pool: &PgPool) {
    sqlx::query("UPDATE chat_watches SET last_edit_at = NOW() - INTERVAL '1 hour'")
        .execute(pool)
        .await
        .expect("aged");
}

async fn last_state(pool: &PgPool) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT last_state FROM chat_watches LIMIT 1")
        .fetch_optional(pool)
        .await
        .expect("queried")
        .flatten()
}

async fn set_prefs(pool: &PgPool, prefs: &ChatNotificationPrefs) {
    chat_notification_prefs::upsert(pool, prefs)
        .await
        .expect("prefs saved");
}

/// The 22:00-07:00 window, chosen explicitly.
///
/// It used to be what a user with no prefs row got for free. Quiet hours are opt-in
/// now, so the tests whose subject *is* that window have to ask for it — the deferral,
/// the dropped progress ping and the midnight wrap are all still real behaviour.
async fn choose_nightly_window(pool: &PgPool, user_id: Uuid) {
    set_prefs(
        pool,
        &ChatNotificationPrefs {
            quiet_from: Some(Time::from_hms(22, 0, 0).expect("22:00")),
            quiet_to: Some(Time::from_hms(7, 0, 0).expect("07:00")),
            ..ChatNotificationPrefs::defaults(user_id)
        },
    )
    .await;
}

/// A quiet window that is either in force at `at`, or definitely is not.
///
/// The watch users have no timezone set, so the gate falls back to the server's
/// zone — which is the clock this window has to be built against.
fn quiet_window(at: OffsetDateTime, active_now: bool) -> (Time, Time) {
    let now = prefs::Zone::ServerLocal.local_time(at);
    // `Time` arithmetic wraps at midnight, which is exactly the behaviour a quiet
    // window needs — 22:00-07:00 is a night, not an empty range.
    if active_now {
        (now - TimeDuration::hours(1), now + TimeDuration::hours(1))
    } else {
        (now + TimeDuration::hours(2), now + TimeDuration::hours(3))
    }
}

fn wanted(entity_id: Uuid, status: &str) -> LibraryEvent {
    LibraryEvent::wanted_status_updated(entity_id, status.to_string())
}

fn download_progress(external_id: &str, pct: f64) -> LibraryEvent {
    LibraryEvent::kobload_download_updated(json!({
        "id": external_id,
        "status": "downloading",
        "progress": pct,
        "name": "someShow.S24E07.1080p.WEB-DL",
    }))
}

/// A wanted attempt is how a Kobload download id resolves back to an entity.
async fn attempt_for(pool: &PgPool, episode_id: Uuid, external_id: &str) {
    sqlx::query(
        "INSERT INTO wanted_attempts (id, episode_id, source, external_id, status, created_at)
         VALUES ($1, $2, 'test', $3, 'downloading', NOW())",
    )
    .bind(Uuid::now_v7())
    .bind(episode_id)
    .bind(external_id)
    .execute(pool)
    .await
    .expect("attempt inserted");
}

async fn episode_with_show(server: &TestServer) -> Uuid {
    let show_id = Uuid::now_v7();
    sqlx::query("INSERT INTO tv_shows (id, title) VALUES ($1, 'someShow')")
        .bind(show_id)
        .execute(&server.pool)
        .await
        .expect("show inserted");
    let season_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order)
         VALUES ($1, $2, '24', 24)",
    )
    .bind(season_id)
    .bind(show_id)
    .execute(&server.pool)
    .await
    .expect("season inserted");
    let episode_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title)
         VALUES ($1, $2, '7', 7, 'The Draft')",
    )
    .bind(episode_id)
    .bind(season_id)
    .execute(&server.pool)
    .await
    .expect("episode inserted");
    episode_id
}

async fn watch_exists(pool: &PgPool, id: Uuid) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM chat_watches WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("counted")
        == 1
}

async fn watch_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM chat_watches")
        .fetch_one(pool)
        .await
        .expect("counted")
}

// =============================================================================
// Coalescing
// =============================================================================

#[tokio::test]
async fn rapid_progress_events_produce_one_edit_per_window() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;
    let external_id = Uuid::now_v7().to_string();
    attempt_for(&server.pool, episode_id, &external_id).await;

    let h = harness(&server.pool, Duration::from_secs(15), true);
    register(
        &server.pool,
        &target("spaces/lounge"),
        episode_id,
        Duration::from_secs(3600),
    )
    .await;

    for pct in [4.0, 11.0, 19.0, 26.0, 34.0, 51.0, 88.0] {
        h.dispatcher
            .handle_event(&download_progress(&external_id, pct))
            .await;
    }

    let calls = h.recorder.calls();
    assert_eq!(
        calls.len(),
        1,
        "seven progress pings inside one window should be one edit: {calls:?}"
    );
    assert_eq!(
        calls[0],
        Call::Edit {
            message_id: "messages/original".to_string(),
            text: "Downloading someShow S24E07 — 4%".to_string(),
        }
    );
}

#[tokio::test]
async fn the_window_reopens() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;
    let external_id = Uuid::now_v7().to_string();
    attempt_for(&server.pool, episode_id, &external_id).await;

    let h = harness(&server.pool, Duration::ZERO, true);
    register(
        &server.pool,
        &target("spaces/lounge"),
        episode_id,
        Duration::from_secs(3600),
    )
    .await;

    for pct in [10.0, 20.0, 30.0] {
        h.dispatcher
            .handle_event(&download_progress(&external_id, pct))
            .await;
    }

    assert_eq!(
        h.recorder.texts(),
        vec![
            "Downloading someShow S24E07 — 10%",
            "Downloading someShow S24E07 — 20%",
            "Downloading someShow S24E07 — 30%",
        ]
    );
}

#[tokio::test]
async fn coalescing_is_per_request_not_global() {
    let server = TestServer::start_api_only().await;
    let first = episode_with_show(&server).await;
    let second = episode_with_show(&server).await;
    let ext_a = Uuid::now_v7().to_string();
    let ext_b = Uuid::now_v7().to_string();
    attempt_for(&server.pool, first, &ext_a).await;
    attempt_for(&server.pool, second, &ext_b).await;

    let h = harness(&server.pool, Duration::from_secs(15), true);
    let space = target("spaces/lounge");
    // Two things she asked for separately, so two labels and two messages.
    for (episode, label) in [(first, "someShow S24E07"), (second, "someOtherShow S01E01")] {
        register_labelled(
            &server.pool,
            &space,
            episode,
            Some("messages/original"),
            None,
            Duration::from_secs(3600),
            label,
        )
        .await;
    }

    h.dispatcher
        .handle_event(&download_progress(&ext_a, 10.0))
        .await;
    h.dispatcher
        .handle_event(&download_progress(&ext_b, 70.0))
        .await;
    // Both are now inside their own windows.
    h.dispatcher
        .handle_event(&download_progress(&ext_a, 12.0))
        .await;
    h.dispatcher
        .handle_event(&download_progress(&ext_b, 74.0))
        .await;

    assert_eq!(
        h.recorder.texts(),
        vec![
            "Downloading someShow S24E07 — 10%",
            "Downloading someOtherShow S01E01 — 70%",
        ],
        "one request's edit must not close another request's window"
    );
}

/// The wall of identical messages, at the other end of the download.
///
/// A season pack grabs thirteen episodes at one instant, and every episode is a
/// watch row of its own with no message id yet — so each one posted its own
/// "Searching for season 11 of Law & Order…". Coalescing per row only spaced
/// them out; the group is what makes them one message.
#[tokio::test]
async fn a_season_posts_its_progress_once_not_once_per_episode() {
    let server = TestServer::start_api_only().await;
    let mut episodes = Vec::new();
    for _ in 0..4 {
        episodes.push(episode_with_show(&server).await);
    }

    let h = harness(&server.pool, Duration::from_secs(15), true);
    let space = target("spaces/lounge");
    for episode in &episodes {
        register_labelled(
            &server.pool,
            &space,
            *episode,
            None,
            None,
            Duration::from_secs(3600),
            "season 11 of Law & Order",
        )
        .await;
    }

    for episode in &episodes {
        h.dispatcher
            .handle_event(&wanted(*episode, "searching"))
            .await;
    }

    assert_eq!(
        h.recorder.texts(),
        vec!["Searching for season 11 of Law & Order…"],
        "one pack is one message, whatever the row count says"
    );

    // And the message the siblings adopted is the one the verdict edits, rather
    // than a second message posted beside it.
    for episode in &episodes {
        h.dispatcher
            .handle_event(&wanted(*episode, "acquired"))
            .await;
    }

    assert_eq!(
        h.recorder.calls().len(),
        2,
        "the verdict is an edit of the progress message: {:?}",
        h.recorder.calls(),
    );
    assert!(
        matches!(h.recorder.calls()[1], Call::Edit { .. }),
        "{:?}",
        h.recorder.calls(),
    );
    assert_eq!(watch_count(&server.pool).await, 0);
}

// =============================================================================
// Terminal states
// =============================================================================

#[tokio::test]
async fn success_gets_a_terminal_message_and_closes_the_watch() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;

    let h = harness(&server.pool, Duration::from_secs(15), true);
    register(
        &server.pool,
        &target("spaces/lounge"),
        episode_id,
        Duration::from_secs(3600),
    )
    .await;

    h.dispatcher
        .handle_event(&wanted(episode_id, "queued"))
        .await;
    h.dispatcher
        .handle_event(&wanted(episode_id, "acquired"))
        .await;

    let texts = h.recorder.texts();
    assert_eq!(
        texts.last().unwrap(),
        "Done, someShow S24E07 is in the library."
    );
    assert_eq!(watch_count(&server.pool).await, 0, "watch should be closed");

    // A duplicate completion must not say it twice.
    let before = h.recorder.calls().len();
    h.dispatcher
        .handle_event(&wanted(episode_id, "completed"))
        .await;
    assert_eq!(h.recorder.calls().len(), before);
}

#[tokio::test]
async fn a_terminal_message_never_waits_for_the_coalescing_window() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;

    let h = harness(&server.pool, Duration::from_secs(3600), true);
    register(
        &server.pool,
        &target("spaces/lounge"),
        episode_id,
        Duration::from_secs(3600),
    )
    .await;

    h.dispatcher
        .handle_event(&wanted(episode_id, "downloading"))
        .await;
    h.dispatcher
        .handle_event(&wanted(episode_id, "acquired"))
        .await;

    assert_eq!(
        h.recorder.texts(),
        vec![
            "Downloading someShow S24E07…",
            "Done, someShow S24E07 is in the library.",
        ]
    );
}

#[tokio::test]
async fn failure_gets_a_terminal_message() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;

    let h = harness(&server.pool, Duration::from_secs(15), true);
    register(
        &server.pool,
        &target("spaces/lounge"),
        episode_id,
        Duration::from_secs(3600),
    )
    .await;

    h.dispatcher
        .handle_event(&wanted(episode_id, "search_failed"))
        .await;

    let texts = h.recorder.texts();
    assert_eq!(texts.len(), 1);
    assert!(
        texts[0].contains("couldn't get someShow S24E07"),
        "unexpected failure copy: {texts:?}"
    );
    assert_eq!(watch_count(&server.pool).await, 0);
}

// =============================================================================
// Expiry
// =============================================================================

#[tokio::test]
async fn an_expired_watch_says_goodbye_and_is_swept() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;
    let external_id = Uuid::now_v7().to_string();
    attempt_for(&server.pool, episode_id, &external_id).await;

    let h = harness(&server.pool, Duration::ZERO, true);
    let watch_id = register(
        &server.pool,
        &target("spaces/lounge"),
        episode_id,
        Duration::from_secs(1),
    )
    .await;
    sqlx::query("UPDATE chat_watches SET expires_at = NOW() - INTERVAL '1 hour' WHERE id = $1")
        .bind(watch_id)
        .execute(&server.pool)
        .await
        .expect("expired");

    // Expired rows are invisible to live events.
    h.dispatcher
        .handle_event(&download_progress(&external_id, 40.0))
        .await;
    h.dispatcher
        .handle_event(&wanted(episode_id, "downloading"))
        .await;
    assert!(
        h.recorder.calls().is_empty(),
        "an expired watch must not keep talking: {:?}",
        h.recorder.calls()
    );

    assert_eq!(h.dispatcher.sweep().await.expired, 1);
    let texts = h.recorder.texts();
    assert_eq!(texts.len(), 1);
    assert!(
        texts[0].contains("stopped watching someShow S24E07"),
        "unexpected expiry copy: {texts:?}"
    );
    assert_eq!(watch_count(&server.pool).await, 0);

    // Nothing left to sweep, and no repeat apology.
    assert_eq!(h.dispatcher.sweep().await.expired, 0);
    assert_eq!(h.recorder.calls().len(), 1);
}

#[tokio::test]
async fn a_live_watch_survives_the_sweep() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;

    let h = harness(&server.pool, Duration::ZERO, true);
    register(
        &server.pool,
        &target("spaces/lounge"),
        episode_id,
        Duration::from_secs(3600),
    )
    .await;

    assert_eq!(h.dispatcher.sweep().await.expired, 0);
    assert_eq!(watch_count(&server.pool).await, 1);
    assert!(h.recorder.calls().is_empty());
}

// =============================================================================
// Fan-out
// =============================================================================

#[tokio::test]
async fn one_event_reaches_every_space_watching_it() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;

    let h = harness(&server.pool, Duration::from_secs(15), true);
    register_with_message(
        &server.pool,
        &target("spaces/lounge"),
        episode_id,
        None,
        Duration::from_secs(3600),
    )
    .await;
    register_with_message(
        &server.pool,
        &target("spaces/kitchen"),
        episode_id,
        None,
        Duration::from_secs(3600),
    )
    .await;

    h.dispatcher
        .handle_event(&wanted(episode_id, "acquired"))
        .await;

    let spaces: Vec<String> = h
        .recorder
        .calls()
        .iter()
        .filter_map(|c| c.space().map(str::to_string))
        .collect();
    assert_eq!(spaces, vec!["spaces/lounge", "spaces/kitchen"]);
    assert_eq!(watch_count(&server.pool).await, 0);
}

#[tokio::test]
async fn an_event_nobody_is_watching_says_nothing() {
    let server = TestServer::start_api_only().await;
    let watched = episode_with_show(&server).await;
    let unwatched = episode_with_show(&server).await;

    let h = harness(&server.pool, Duration::ZERO, true);
    register(
        &server.pool,
        &target("spaces/lounge"),
        watched,
        Duration::from_secs(3600),
    )
    .await;

    h.dispatcher
        .handle_event(&wanted(unwatched, "acquired"))
        .await;
    h.dispatcher
        .handle_event(&LibraryEvent::tv_episode_updated(watched))
        .await;
    h.dispatcher
        .handle_event(&download_progress(&Uuid::now_v7().to_string(), 50.0))
        .await;

    assert!(h.recorder.calls().is_empty());
    assert_eq!(watch_count(&server.pool).await, 1);
}

#[tokio::test]
async fn re_asking_in_one_thread_refreshes_rather_than_duplicates() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;
    let space = target("spaces/lounge");

    let first = register(&server.pool, &space, episode_id, Duration::from_secs(3600)).await;
    let second = register(&server.pool, &space, episode_id, Duration::from_secs(3600)).await;

    assert_eq!(first, second);
    assert_eq!(watch_count(&server.pool).await, 1);
}

// =============================================================================
// Transports that cannot edit
// =============================================================================

#[tokio::test]
async fn without_edit_it_posts_on_transitions_only() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;
    let external_id = Uuid::now_v7().to_string();
    attempt_for(&server.pool, episode_id, &external_id).await;

    let h = harness(&server.pool, Duration::ZERO, false);
    register(
        &server.pool,
        &target("spaces/lounge"),
        episode_id,
        Duration::from_secs(3600),
    )
    .await;

    h.dispatcher
        .handle_event(&wanted(episode_id, "searching"))
        .await;
    for pct in [10.0, 20.0, 30.0, 40.0] {
        h.dispatcher
            .handle_event(&download_progress(&external_id, pct))
            .await;
    }
    h.dispatcher
        .handle_event(&wanted(episode_id, "acquired"))
        .await;

    let calls = h.recorder.calls();
    assert!(
        calls.iter().all(|c| matches!(c, Call::Post { .. })),
        "a transport without edit_in_place must never patch: {calls:?}"
    );
    assert_eq!(
        h.recorder.texts(),
        vec![
            "Searching for someShow S24E07…",
            "Downloading someShow S24E07 — 10%",
            "Done, someShow S24E07 is in the library.",
        ],
        "percentages after the first must not become messages"
    );
}

#[tokio::test]
async fn a_watch_without_a_message_id_posts_then_edits() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;

    let h = harness(&server.pool, Duration::ZERO, true);
    register_with_message(
        &server.pool,
        &target("spaces/lounge"),
        episode_id,
        None,
        Duration::from_secs(3600),
    )
    .await;

    h.dispatcher
        .handle_event(&wanted(episode_id, "searching"))
        .await;
    h.dispatcher
        .handle_event(&wanted(episode_id, "downloading"))
        .await;

    let calls = h.recorder.calls();
    assert!(matches!(calls[0], Call::Post { .. }), "{calls:?}");
    assert!(
        matches!(&calls[1], Call::Edit { message_id, .. } if message_id == "messages/1"),
        "the posted message id must be remembered and patched: {calls:?}"
    );
}

// =============================================================================
// The subscriber itself
// =============================================================================

#[tokio::test]
async fn the_subscriber_survives_a_lagged_receiver() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;

    let h = harness(&server.pool, Duration::ZERO, true);
    register(
        &server.pool,
        &target("spaces/lounge"),
        episode_id,
        Duration::from_secs(3600),
    )
    .await;

    let (tx, rx) = tokio::sync::broadcast::channel::<LibraryEvent>(2);
    // Overflow before anything consumes: the receiver's first recv is Lagged.
    for _ in 0..8 {
        let _ = tx.send(LibraryEvent::tv_episode_updated(episode_id));
    }

    let handle = tokio::spawn(run_subscriber(h.dispatcher, rx));

    let _ = tx.send(wanted(episode_id, "acquired"));
    for _ in 0..200 {
        if !h.recorder.calls().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(
        h.recorder.texts(),
        vec!["Done, someShow S24E07 is in the library."],
        "a lag must not kill the subscriber"
    );

    drop(tx);
    handle.await.expect("subscriber exits on a closed bus");
}

// =============================================================================
// A verdict that fails to land is owed, not lost
// =============================================================================

#[tokio::test]
async fn a_failed_terminal_is_retried_and_still_sent_exactly_once() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;

    let h = harness(&server.pool, Duration::ZERO, true);
    register(
        &server.pool,
        &target("spaces/lounge"),
        episode_id,
        Duration::from_secs(3600),
    )
    .await;

    h.recorder.fail_next(1);
    h.dispatcher
        .handle_event(&wanted(episode_id, "acquired"))
        .await;

    assert!(
        h.recorder.calls().is_empty(),
        "the send failed, so nothing was delivered"
    );
    assert_eq!(
        watch_count(&server.pool).await,
        1,
        "a verdict that did not land must survive to be retried"
    );
    assert_eq!(last_state(&server.pool).await.as_deref(), Some("done"));

    // A duplicate completion must not re-narrate a watch that already has a verdict.
    h.dispatcher
        .handle_event(&wanted(episode_id, "completed"))
        .await;
    assert!(h.recorder.calls().is_empty());
    assert_eq!(watch_count(&server.pool).await, 1);

    age_last_attempt(&server.pool).await;
    let outcome = h.dispatcher.sweep().await;

    assert_eq!(outcome.retried, 1);
    assert_eq!(outcome.expired, 0);
    assert_eq!(outcome.abandoned, 0);
    assert_eq!(
        h.recorder.texts(),
        vec!["Done, someShow S24E07 is in the library."],
        "exactly one message across both attempts"
    );
    assert_eq!(watch_count(&server.pool).await, 0);
}

#[tokio::test]
async fn a_verdict_that_never_lands_is_given_up_on_not_left_forever() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;

    let h = harness(&server.pool, Duration::ZERO, true);
    register(
        &server.pool,
        &target("spaces/lounge"),
        episode_id,
        Duration::from_secs(3600),
    )
    .await;

    h.recorder.fail_next(1);
    h.dispatcher
        .handle_event(&wanted(episode_id, "acquired"))
        .await;
    assert_eq!(watch_count(&server.pool).await, 1);

    // The retry window runs out.
    sqlx::query("UPDATE chat_watches SET expires_at = NOW() - INTERVAL '1 minute'")
        .execute(&server.pool)
        .await
        .expect("retry window elapsed");

    let outcome = h.dispatcher.sweep().await;
    assert_eq!(outcome.abandoned, 1);
    assert_eq!(outcome.expired, 0, "an abandoned verdict is not re-expired");
    assert!(h.recorder.calls().is_empty());
    assert_eq!(watch_count(&server.pool).await, 0);
}

// =============================================================================
// The requester's notification preferences
// =============================================================================

#[tokio::test]
async fn switching_notifications_off_closes_the_watch_silently() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;
    let user_id = create_user(&server, "quiet@example.com").await;
    set_prefs(
        &server.pool,
        &ChatNotificationPrefs {
            enabled: false,
            ..ChatNotificationPrefs::defaults(user_id)
        },
    )
    .await;

    let h = harness(&server.pool, Duration::ZERO, true);
    register_for_user(&server.pool, &target("spaces/lounge"), episode_id, user_id).await;

    h.dispatcher
        .handle_event(&wanted(episode_id, "acquired"))
        .await;

    assert!(
        h.recorder.calls().is_empty(),
        "a standing 'do not tell me' is a refusal, not a bug"
    );
    assert_eq!(
        watch_count(&server.pool).await,
        0,
        "a refused verdict still closes the watch — nothing is owed"
    );
}

#[tokio::test]
async fn quiet_hours_defer_a_verdict_rather_than_dropping_it() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;
    let user_id = create_user(&server, "asleep@example.com").await;

    let night = local_at(23, 30);
    let (from, to) = quiet_window(night, true);
    set_prefs(
        &server.pool,
        &ChatNotificationPrefs {
            quiet_from: Some(from),
            quiet_to: Some(to),
            ..ChatNotificationPrefs::defaults(user_id)
        },
    )
    .await;

    let h = harness_at(&server.pool, Duration::ZERO, true, Clock::pinned(night));
    register_for_user(&server.pool, &target("spaces/lounge"), episode_id, user_id).await;

    h.dispatcher
        .handle_event(&wanted(episode_id, "acquired"))
        .await;

    assert!(
        h.recorder.calls().is_empty(),
        "nothing fires at 2am because a scheduled search finished"
    );
    assert_eq!(
        watch_count(&server.pool).await,
        1,
        "the verdict is held, not lost"
    );
    assert_eq!(last_state(&server.pool).await.as_deref(), Some("done"));

    // Morning.
    let (from, to) = quiet_window(night, false);
    set_prefs(
        &server.pool,
        &ChatNotificationPrefs {
            quiet_from: Some(from),
            quiet_to: Some(to),
            ..ChatNotificationPrefs::defaults(user_id)
        },
    )
    .await;
    age_last_attempt(&server.pool).await;

    assert_eq!(h.dispatcher.sweep().await.retried, 1);
    assert_eq!(
        h.recorder.texts(),
        vec!["Done, someShow S24E07 is in the library."]
    );
    assert_eq!(watch_count(&server.pool).await, 0);
}

#[tokio::test]
async fn a_mute_defers_a_verdict_too() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;
    let user_id = create_user(&server, "muted@example.com").await;
    // Pinned to the middle of the afternoon so the mute is the only thing holding
    // the verdict back, and this user chooses no quiet window at all.
    let clock = daytime();
    set_prefs(
        &server.pool,
        &ChatNotificationPrefs {
            muted_until: Some(local_at(14, 0) + TimeDuration::hours(2)),
            ..ChatNotificationPrefs::defaults(user_id)
        },
    )
    .await;

    let h = harness_at(&server.pool, Duration::ZERO, true, clock);
    register_for_user(&server.pool, &target("spaces/lounge"), episode_id, user_id).await;

    h.dispatcher
        .handle_event(&wanted(episode_id, "acquired"))
        .await;

    assert!(h.recorder.calls().is_empty());
    assert_eq!(watch_count(&server.pool).await, 1);

    set_prefs(&server.pool, &ChatNotificationPrefs::defaults(user_id)).await;
    age_last_attempt(&server.pool).await;

    assert_eq!(h.dispatcher.sweep().await.retried, 1);
    assert_eq!(
        h.recorder.texts(),
        vec!["Done, someShow S24E07 is in the library."]
    );
}

#[tokio::test]
async fn default_prefs_deliver_the_whole_progression() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;
    let external_id = Uuid::now_v7().to_string();
    attempt_for(&server.pool, episode_id, &external_id).await;
    let user_id = create_user(&server, "default@example.com").await;

    // No prefs row at all. The defaults are on_import only — and that must not matter
    // here, because a watch is a reply to something she asked for, not a notification.
    let defaults = ChatNotificationPrefs::defaults(user_id);
    assert!(!defaults.triggers.contains(&NotificationTrigger::OnGrab));

    // Daytime, so the defaults under test are the trigger list — not the default
    // quiet window, which has its own tests below.
    let h = harness_at(&server.pool, Duration::ZERO, true, daytime());
    register_for_user(&server.pool, &target("spaces/lounge"), episode_id, user_id).await;

    h.dispatcher
        .handle_event(&wanted(episode_id, "searching"))
        .await;
    h.dispatcher
        .handle_event(&download_progress(&external_id, 40.0))
        .await;
    h.dispatcher
        .handle_event(&wanted(episode_id, "acquired"))
        .await;

    assert_eq!(
        h.recorder.texts(),
        vec![
            "Searching for someShow S24E07…",
            "Downloading someShow S24E07 — 40%",
            "Done, someShow S24E07 is in the library.",
        ],
        "the running commentary is the answer to her question, not opt-in noise"
    );
    assert_eq!(watch_count(&server.pool).await, 0);
}

#[tokio::test]
async fn turning_off_the_download_trigger_does_not_silence_a_reply() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;
    let user_id = create_user(&server, "nofirehose@example.com").await;

    // "Stop telling me about downloads" — an empty trigger list is as far as that
    // goes, and it still must not stop the bot answering when she asks for something.
    set_prefs(
        &server.pool,
        &ChatNotificationPrefs {
            triggers: vec![],
            ..ChatNotificationPrefs::defaults(user_id)
        },
    )
    .await;

    let h = harness_at(&server.pool, Duration::ZERO, true, daytime());
    register_for_user(&server.pool, &target("spaces/lounge"), episode_id, user_id).await;

    h.dispatcher
        .handle_event(&wanted(episode_id, "searching"))
        .await;
    h.dispatcher
        .handle_event(&wanted(episode_id, "search_failed"))
        .await;

    assert_eq!(
        h.recorder.texts(),
        vec![
            "Searching for someShow S24E07…",
            "That didn't work — I couldn't get someShow S24E07, and nothing's downloading for it now.",
        ],
        "a failure is still a terminal message, whatever the trigger list says"
    );
    assert_eq!(watch_count(&server.pool).await, 0);
}

// =============================================================================
// A chosen quiet window, on a clock the test owns
// =============================================================================

#[tokio::test]
async fn a_verdict_inside_a_chosen_quiet_window_is_deferred_then_delivered_in_the_morning() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;
    let user_id = create_user(&server, "defaults-asleep@example.com").await;
    choose_nightly_window(&server.pool, user_id).await;

    let h = harness_at(
        &server.pool,
        Duration::ZERO,
        true,
        Clock::pinned(local_at(23, 30)),
    );
    let watch_id =
        register_for_user(&server.pool, &target("spaces/lounge"), episode_id, user_id).await;

    h.dispatcher
        .handle_event(&wanted(episode_id, "acquired"))
        .await;

    assert!(
        h.recorder.calls().is_empty(),
        "23:30 is inside the chosen window, so nothing is said"
    );
    assert!(
        watch_exists(&server.pool, watch_id).await,
        "the verdict is owed, so the watch stays"
    );
    assert_eq!(last_state(&server.pool).await.as_deref(), Some("done"));

    h.clock.advance_to(local_at(8, 0));
    age_last_attempt(&server.pool).await;

    assert_eq!(
        h.dispatcher.sweep().await.retried,
        1,
        "the sweep pays the debt once the window has passed"
    );
    assert_eq!(
        h.recorder.texts(),
        vec!["Done, someShow S24E07 is in the library."]
    );
    assert!(!watch_exists(&server.pool, watch_id).await);
}

#[tokio::test]
async fn progress_inside_a_chosen_quiet_window_is_refused_while_the_verdict_is_deferred() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;
    let external_id = Uuid::now_v7().to_string();
    attempt_for(&server.pool, episode_id, &external_id).await;
    let user_id = create_user(&server, "defaults-progress@example.com").await;
    choose_nightly_window(&server.pool, user_id).await;

    let h = harness_at(
        &server.pool,
        Duration::ZERO,
        true,
        Clock::pinned(local_at(2, 0)),
    );
    let watch_id =
        register_for_user(&server.pool, &target("spaces/lounge"), episode_id, user_id).await;

    h.dispatcher
        .handle_event(&wanted(episode_id, "searching"))
        .await;
    h.dispatcher
        .handle_event(&download_progress(&external_id, 40.0))
        .await;

    assert!(h.recorder.calls().is_empty());
    assert_eq!(
        last_state(&server.pool).await,
        None,
        "a progress ping at 2am is dropped, not owed — by morning it is stale"
    );

    h.dispatcher
        .handle_event(&wanted(episode_id, "acquired"))
        .await;

    assert!(h.recorder.calls().is_empty());
    assert_eq!(
        last_state(&server.pool).await.as_deref(),
        Some("done"),
        "the verdict at the same hour is held instead"
    );

    h.clock.advance_to(local_at(8, 0));
    age_last_attempt(&server.pool).await;
    assert_eq!(h.dispatcher.sweep().await.retried, 1);

    assert_eq!(
        h.recorder.texts(),
        vec!["Done, someShow S24E07 is in the library."],
        "morning gets the verdict and none of the progress it superseded"
    );
    assert!(!watch_exists(&server.pool, watch_id).await);
}

/// 22:00-07:00 wraps midnight, so `from < to` is false and the naive `BETWEEN`
/// reading of it is inverted. These are the four instants that tell the two apart.
#[tokio::test]
async fn a_chosen_quiet_window_wraps_midnight_at_its_edges() {
    let server = TestServer::start_api_only().await;

    let cases = [
        (21, 59, true, "one minute before the window opens"),
        (22, 0, false, "the window is inclusive at its start"),
        (6, 59, false, "still inside, on the far side of midnight"),
        (7, 0, true, "the window is exclusive at its end"),
    ];

    for (i, (hour, minute, delivered, why)) in cases.into_iter().enumerate() {
        let episode_id = episode_with_show(&server).await;
        let user_id = create_user(&server, &format!("edge{i}@example.com")).await;
        choose_nightly_window(&server.pool, user_id).await;
        let h = harness_at(
            &server.pool,
            Duration::ZERO,
            true,
            Clock::pinned(local_at(hour, minute)),
        );
        let watch_id = register_for_user(
            &server.pool,
            &target(&format!("spaces/edge{i}")),
            episode_id,
            user_id,
        )
        .await;

        h.dispatcher
            .handle_event(&wanted(episode_id, "acquired"))
            .await;

        if delivered {
            assert_eq!(
                h.recorder.texts(),
                vec!["Done, someShow S24E07 is in the library."],
                "{hour:02}:{minute:02} — {why}"
            );
            assert!(!watch_exists(&server.pool, watch_id).await);
        } else {
            assert!(
                h.recorder.calls().is_empty(),
                "{hour:02}:{minute:02} — {why}"
            );
            assert!(
                watch_exists(&server.pool, watch_id).await,
                "{hour:02}:{minute:02} — a deferred verdict is still owed"
            );
        }
    }
}

// =============================================================================
// One request, one verdict
// =============================================================================

/// The wall of identical messages.
///
/// A season want registers a watch per episode under one season-level label.
/// Cancelling the season flips every episode at once, and each watch rendered
/// the same sentence about the same season — thirteen copies in the thread.
#[tokio::test]
async fn a_season_says_its_verdict_once_not_once_per_episode() {
    let server = TestServer::start_api_only().await;
    let mut episodes = Vec::new();
    for _ in 0..4 {
        episodes.push(episode_with_show(&server).await);
    }

    let h = harness(&server.pool, Duration::from_secs(15), true);
    let space = target("spaces/lounge");
    for episode in &episodes {
        register_labelled(
            &server.pool,
            &space,
            *episode,
            None,
            None,
            Duration::from_secs(3600),
            "season 11 of Law & Order",
        )
        .await;
    }

    for episode in &episodes {
        h.dispatcher.handle_event(&wanted(*episode, "failed")).await;
    }

    assert_eq!(
        h.recorder.texts().len(),
        1,
        "one request gets one verdict, not one per episode: {:?}",
        h.recorder.texts(),
    );
    assert_eq!(
        watch_count(&server.pool).await,
        0,
        "the silenced siblings must be closed, not left for the sweep to re-send",
    );
}

/// A stalled season expires as one request too. Thirteen rows, one apology.
#[tokio::test]
async fn an_expired_season_apologises_once_not_once_per_episode() {
    let server = TestServer::start_api_only().await;
    let mut episodes = Vec::new();
    for _ in 0..4 {
        episodes.push(episode_with_show(&server).await);
    }

    let h = harness(&server.pool, Duration::ZERO, true);
    let space = target("spaces/lounge");
    for episode in &episodes {
        register_labelled(
            &server.pool,
            &space,
            *episode,
            None,
            None,
            Duration::from_secs(1),
            "season 11 of Law & Order",
        )
        .await;
    }
    sqlx::query("UPDATE chat_watches SET expires_at = NOW() - INTERVAL '1 hour'")
        .execute(&server.pool)
        .await
        .expect("expired");

    assert_eq!(h.dispatcher.sweep().await.expired, 1);
    let texts = h.recorder.texts();
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(
        texts[0].contains("stopped watching season 11 of Law & Order"),
        "unexpected expiry copy: {texts:?}"
    );
    assert_eq!(
        watch_count(&server.pool).await,
        0,
        "the silenced siblings are closed, not left for the retry sweep to deliver",
    );
    assert_eq!(h.dispatcher.sweep().await.expired, 0);
    assert_eq!(h.recorder.calls().len(), 1);
}

/// Two seasons in one thread are two requests, and both are owed an answer.
#[tokio::test]
async fn a_different_request_in_the_same_thread_still_speaks() {
    let server = TestServer::start_api_only().await;
    let eleven = episode_with_show(&server).await;
    let twelve = episode_with_show(&server).await;

    let h = harness(&server.pool, Duration::from_secs(15), true);
    let space = target("spaces/lounge");
    for (episode, label) in [
        (eleven, "season 11 of Law & Order"),
        (twelve, "season 12 of Law & Order"),
    ] {
        register_labelled(
            &server.pool,
            &space,
            episode,
            None,
            None,
            Duration::from_secs(3600),
            label,
        )
        .await;
    }

    h.dispatcher.handle_event(&wanted(eleven, "failed")).await;
    h.dispatcher.handle_event(&wanted(twelve, "failed")).await;

    assert_eq!(h.recorder.texts().len(), 2, "{:?}", h.recorder.texts());
}

/// A cancellation is not a failure. She stopped it herself; being told it
/// "didn't work" reads as a bug in the thing she just did.
#[tokio::test]
async fn a_cancellation_is_reported_as_a_stop_not_a_failure() {
    let server = TestServer::start_api_only().await;
    let episode_id = episode_with_show(&server).await;

    let h = harness(&server.pool, Duration::from_secs(15), true);
    register(
        &server.pool,
        &target("spaces/lounge"),
        episode_id,
        Duration::from_secs(3600),
    )
    .await;

    h.dispatcher
        .handle_event(&wanted(episode_id, "cancelled"))
        .await;

    let texts = h.recorder.texts();
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(
        !texts[0].contains("didn't work"),
        "a cancellation she asked for is not a failure: {texts:?}",
    );
    assert!(
        texts[0].contains("someShow S24E07"),
        "it still has to say what it stopped: {texts:?}",
    );
}
