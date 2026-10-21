//! The relevance predicate is the thing that stops chat notifications being a
//! firehose, so every signal gets asserted in isolation and a user with no
//! signal must be absent from every result.

use crate::common::*;
use api::notifications::NotificationTrigger;
use api::notifications::relevance::{Entity, chat_recipients, classify, interested_users};
use api::repositories::{chat_identities, chat_notification_prefs, notification_agents};
use serde_json::json;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

// ============================================================================
// Fixtures
// ============================================================================

async fn user(pool: &PgPool, email: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO users (id, email, password_hash, role) VALUES ($1, $2, 'x', 'contributor')",
        id,
        email,
    )
    .execute(pool)
    .await
    .expect("insert user");
    id
}

async fn show(pool: &PgPool, title: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO tv_shows (id, title) VALUES ($1, $2)",
        id,
        title
    )
    .execute(pool)
    .await
    .expect("insert show");
    id
}

async fn season(pool: &PgPool, show_id: Uuid, number: i32) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order)
         VALUES ($1, $2, $3, $4)",
        id,
        show_id,
        number.to_string(),
        number,
    )
    .execute(pool)
    .await
    .expect("insert season");
    id
}

async fn episode(pool: &PgPool, season_id: Uuid, number: i32) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order)
         VALUES ($1, $2, $3, $4)",
        id,
        season_id,
        number.to_string(),
        number,
    )
    .execute(pool)
    .await
    .expect("insert episode");
    id
}

async fn movie(pool: &PgPool, title: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query!("INSERT INTO movies (id, title) VALUES ($1, $2)", id, title)
        .execute(pool)
        .await
        .expect("insert movie");
    id
}

async fn book(pool: &PgPool, title: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query!("INSERT INTO books (id, title) VALUES ($1, $2)", id, title)
        .execute(pool)
        .await
        .expect("insert book");
    id
}

async fn watched_episode(pool: &PgPool, user_id: Uuid, episode_id: Uuid) {
    sqlx::query!(
        "INSERT INTO tv_watch_states (episode_id, user_id, watched) VALUES ($1, $2, TRUE)",
        episode_id,
        user_id,
    )
    .execute(pool)
    .await
    .expect("insert tv watch state");
}

async fn watched_movie(pool: &PgPool, user_id: Uuid, movie_id: Uuid) {
    sqlx::query!(
        "INSERT INTO movie_watch_states (movie_id, user_id, watched) VALUES ($1, $2, TRUE)",
        movie_id,
        user_id,
    )
    .execute(pool)
    .await
    .expect("insert movie watch state");
}

async fn watchlisted(pool: &PgPool, user_id: Uuid, media_type: &str, media_id: Uuid) {
    sqlx::query!(
        "INSERT INTO watchlist_items (user_id, media_type, media_id) VALUES ($1, $2, $3)",
        user_id,
        media_type,
        media_id,
    )
    .execute(pool)
    .await
    .expect("insert watchlist item");
}

/// One `wanted_attempts` row. `user_id` is nullable — a `None` here is the
/// pre-attribution row shape, and it must pull nobody in.
async fn asked_for(
    pool: &PgPool,
    user_id: Option<Uuid>,
    book_id: Option<Uuid>,
    movie_id: Option<Uuid>,
    episode_id: Option<Uuid>,
) {
    sqlx::query!(
        "INSERT INTO wanted_attempts (id, source, status, user_id, book_id, movie_id, episode_id)
         VALUES ($1, 'test', 'grabbed', $2, $3, $4, $5)",
        Uuid::now_v7(),
        user_id,
        book_id,
        movie_id,
        episode_id,
    )
    .execute(pool)
    .await
    .expect("insert wanted attempt");
}

async fn reading(pool: &PgPool, user_id: Uuid, book_id: Uuid) {
    let device_id = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO devices (id, user_id, token) VALUES ($1, $2, $3)",
        device_id,
        user_id,
        format!("tok-{device_id}"),
    )
    .execute(pool)
    .await
    .expect("insert device");
    sqlx::query!(
        "INSERT INTO reading_states (id, book_id, user_id, device_id) VALUES ($1, $2, $3, $4)",
        Uuid::now_v7(),
        book_id,
        user_id,
        device_id,
    )
    .execute(pool)
    .await
    .expect("insert reading state");
}

fn sorted(mut ids: Vec<Uuid>) -> Vec<Uuid> {
    ids.sort();
    ids
}

// ============================================================================
// Per-signal isolation
// ============================================================================

#[tokio::test]
async fn each_video_signal_pulls_exactly_its_own_user() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let watcher = user(pool, "watcher@test.local").await;
    let lister = user(pool, "lister@test.local").await;
    let asker = user(pool, "asker@test.local").await;
    let bystander = user(pool, "bystander@test.local").await;

    let show_id = show(pool, "Signal Show").await;
    let season_id = season(pool, show_id, 1).await;
    let ep1 = episode(pool, season_id, 1).await;
    let ep2 = episode(pool, season_id, 2).await;

    watched_episode(pool, watcher, ep1).await;
    watchlisted(pool, lister, "show", show_id).await;
    asked_for(pool, Some(asker), None, None, Some(ep2)).await;
    // Attribution-less attempt: nobody is pulled in by it.
    asked_for(pool, None, None, None, Some(ep2)).await;

    let show_users = interested_users(pool, &Entity::Show(show_id))
        .await
        .expect("show relevance");
    assert_eq!(show_users, sorted(vec![watcher, lister, asker]));
    assert!(!show_users.contains(&bystander));

    // The episode the asker asked for: all three signals land.
    let ep2_users = interested_users(pool, &Entity::Episode(ep2))
        .await
        .expect("ep2 relevance");
    assert_eq!(ep2_users, sorted(vec![watcher, lister, asker]));

    // A different episode of the same show: watch state and the watchlist
    // resolve up to the show, the request does not — it named ep2.
    let ep1_users = interested_users(pool, &Entity::Episode(ep1))
        .await
        .expect("ep1 relevance");
    assert_eq!(ep1_users, sorted(vec![watcher, lister]));

    let season_users = interested_users(pool, &Entity::Season(season_id))
        .await
        .expect("season relevance");
    assert_eq!(season_users, sorted(vec![watcher, lister, asker]));
}

#[tokio::test]
async fn an_unrelated_show_is_silence_for_everyone() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let watcher = user(pool, "quiet-watcher@test.local").await;
    let watched_show = show(pool, "Watched Show").await;
    let watched_season = season(pool, watched_show, 1).await;
    let watched_ep = episode(pool, watched_season, 1).await;
    watched_episode(pool, watcher, watched_ep).await;

    let other_show = show(pool, "Never Heard Of It").await;
    let other_season = season(pool, other_show, 9).await;
    let other_ep = episode(pool, other_season, 4).await;

    assert!(
        interested_users(pool, &Entity::Show(other_show))
            .await
            .expect("show relevance")
            .is_empty()
    );
    assert!(
        interested_users(pool, &Entity::Season(other_season))
            .await
            .expect("season relevance")
            .is_empty()
    );
    assert!(
        interested_users(pool, &Entity::Episode(other_ep))
            .await
            .expect("episode relevance")
            .is_empty()
    );
}

#[tokio::test]
async fn movie_signals_are_independent() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let watcher = user(pool, "m-watcher@test.local").await;
    let lister = user(pool, "m-lister@test.local").await;
    let asker = user(pool, "m-asker@test.local").await;
    let bystander = user(pool, "m-bystander@test.local").await;

    let watched = movie(pool, "Seen It").await;
    let listed = movie(pool, "On The List").await;
    let requested = movie(pool, "Asked For It").await;
    let ignored = movie(pool, "Nobody Cares").await;

    watched_movie(pool, watcher, watched).await;
    watchlisted(pool, lister, "movie", listed).await;
    asked_for(pool, Some(asker), None, Some(requested), None).await;

    assert_eq!(
        interested_users(pool, &Entity::Movie(watched))
            .await
            .unwrap(),
        vec![watcher]
    );
    assert_eq!(
        interested_users(pool, &Entity::Movie(listed))
            .await
            .unwrap(),
        vec![lister]
    );
    assert_eq!(
        interested_users(pool, &Entity::Movie(requested))
            .await
            .unwrap(),
        vec![asker]
    );
    assert!(
        interested_users(pool, &Entity::Movie(ignored))
            .await
            .unwrap()
            .is_empty()
    );
    for id in [watched, listed, requested, ignored] {
        assert!(
            !interested_users(pool, &Entity::Movie(id))
                .await
                .unwrap()
                .contains(&bystander)
        );
    }
}

#[tokio::test]
async fn book_signals_are_reading_and_asking() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let reader = user(pool, "reader@test.local").await;
    let asker = user(pool, "b-asker@test.local").await;
    let bystander = user(pool, "b-bystander@test.local").await;

    let being_read = book(pool, "Half Finished").await;
    let requested = book(pool, "The New Sanderson").await;
    let ignored = book(pool, "Untouched").await;

    reading(pool, reader, being_read).await;
    asked_for(pool, Some(asker), Some(requested), None, None).await;

    assert_eq!(
        interested_users(pool, &Entity::Book(being_read))
            .await
            .unwrap(),
        vec![reader]
    );
    assert_eq!(
        interested_users(pool, &Entity::Book(requested))
            .await
            .unwrap(),
        vec![asker]
    );
    assert!(
        interested_users(pool, &Entity::Book(ignored))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !interested_users(pool, &Entity::Book(being_read))
            .await
            .unwrap()
            .contains(&bystander)
    );
}

#[tokio::test]
async fn matching_several_signals_yields_one_entry() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let keen = user(pool, "keen@test.local").await;

    let show_id = show(pool, "Triple Signal").await;
    let season_id = season(pool, show_id, 2).await;
    let ep = episode(pool, season_id, 3).await;
    let other_ep = episode(pool, season_id, 4).await;

    watched_episode(pool, keen, other_ep).await;
    watchlisted(pool, keen, "show", show_id).await;
    asked_for(pool, Some(keen), None, None, Some(ep)).await;
    asked_for(pool, Some(keen), None, None, Some(ep)).await;

    assert_eq!(
        interested_users(pool, &Entity::Episode(ep)).await.unwrap(),
        vec![keen]
    );
    assert_eq!(
        interested_users(pool, &Entity::Show(show_id))
            .await
            .unwrap(),
        vec![keen]
    );

    let movie_id = movie(pool, "Double Signal").await;
    watched_movie(pool, keen, movie_id).await;
    watchlisted(pool, keen, "movie", movie_id).await;
    asked_for(pool, Some(keen), None, Some(movie_id), None).await;
    assert_eq!(
        interested_users(pool, &Entity::Movie(movie_id))
            .await
            .unwrap(),
        vec![keen]
    );
}

// ============================================================================
// classify
// ============================================================================

#[tokio::test]
async fn classify_names_the_right_table() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = show(pool, "Classify Me").await;
    let season_id = season(pool, show_id, 1).await;
    let episode_id = episode(pool, season_id, 1).await;
    let movie_id = movie(pool, "Classify Movie").await;
    let book_id = book(pool, "Classify Book").await;

    let cases = [
        (show_id, Some(Entity::Show(show_id))),
        (season_id, Some(Entity::Season(season_id))),
        (episode_id, Some(Entity::Episode(episode_id))),
        (movie_id, Some(Entity::Movie(movie_id))),
        (book_id, Some(Entity::Book(book_id))),
        (Uuid::now_v7(), None),
    ];

    for (id, expected) in cases {
        assert_eq!(classify(pool, id).await.expect("classify"), expected);
    }
}

// ============================================================================
// Fan-out scoping
// ============================================================================

async fn identity_for(pool: &PgPool, user_id: Uuid, address: &str) -> Uuid {
    chat_identities::create(
        pool,
        &chat_identities::NewChatIdentity {
            transport: "google_chat",
            address: Some(address),
            external_id: None,
            user_id,
            max_role: "contributor",
            enabled: true,
        },
    )
    .await
    .expect("create identity")
    .id
}

#[tokio::test]
async fn fan_out_stops_at_prefs_and_identities() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let now = OffsetDateTime::now_utc();

    let interested = user(pool, "fan-interested@test.local").await;
    let muted = user(pool, "fan-muted@test.local").await;
    let no_identity = user(pool, "fan-no-identity@test.local").await;
    let bystander = user(pool, "fan-bystander@test.local").await;

    let movie_id = movie(pool, "Fan Out Movie").await;
    for id in [interested, muted, no_identity] {
        watchlisted(pool, id, "movie", movie_id).await;
    }

    identity_for(pool, interested, "interested@example.com").await;
    identity_for(pool, muted, "muted@example.com").await;

    // Quiet hours explicitly off for everyone here, so the run does not depend on
    // nobody having chosen a window.
    for id in [interested, muted, no_identity] {
        let mut prefs = chat_notification_prefs::get(pool, id).await.unwrap();
        prefs.quiet_from = Some(time::Time::MIDNIGHT);
        prefs.quiet_to = Some(time::Time::MIDNIGHT);
        chat_notification_prefs::upsert(pool, &prefs).await.unwrap();
    }
    let mut muted_prefs = chat_notification_prefs::get(pool, muted).await.unwrap();
    muted_prefs.muted_until = Some(now + time::Duration::hours(2));
    chat_notification_prefs::upsert(pool, &muted_prefs)
        .await
        .unwrap();

    let recipients = chat_recipients(
        pool,
        &Entity::Movie(movie_id),
        NotificationTrigger::OnImport,
        now,
    )
    .await
    .expect("chat recipients");

    let user_ids: Vec<Uuid> = recipients.iter().map(|r| r.user_id).collect();
    assert_eq!(user_ids, vec![interested]);
    assert_eq!(
        recipients[0].identity.address.as_deref(),
        Some("interested@example.com")
    );
    assert!(!user_ids.contains(&muted));
    assert!(!user_ids.contains(&no_identity));
    assert!(!user_ids.contains(&bystander));

    // A trigger nobody has switched on is silence even for the interested user.
    let grabs = chat_recipients(
        pool,
        &Entity::Movie(movie_id),
        NotificationTrigger::OnGrab,
        now,
    )
    .await
    .expect("chat recipients on_grab");
    assert!(grabs.is_empty());
}

// ============================================================================
// Regression: the global agents are untouched
// ============================================================================

/// Relevance scoping is a property of the chat agent. Pushover, Plex and web
/// push must keep fanning out to every enabled agent whose `triggers` array
/// carries the event, with no user in the query at all — so an entity with zero
/// interested users still selects the global agent.
#[tokio::test]
async fn global_agent_fan_out_ignores_relevance() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let now = OffsetDateTime::now_utc();

    let ignored_movie = movie(pool, "Nobody Asked For This").await;

    for (agent_type, name) in [
        ("pushover", "Global Pushover"),
        ("plex", "Global Plex"),
        ("webpush", "Global Web Push"),
    ] {
        notification_agents::create(
            pool,
            notification_agents::CreateNotificationAgent {
                agent_type: agent_type.to_string(),
                name: name.to_string(),
                enabled: true,
                config: json!({}),
                triggers: json!(["on_grab", "on_import", "on_failed"]),
            },
        )
        .await
        .expect("create agent");
    }

    assert!(
        interested_users(pool, &Entity::Movie(ignored_movie))
            .await
            .unwrap()
            .is_empty(),
        "fixture is meant to interest nobody"
    );
    assert!(
        chat_recipients(
            pool,
            &Entity::Movie(ignored_movie),
            NotificationTrigger::OnImport,
            now,
        )
        .await
        .unwrap()
        .is_empty(),
        "the chat agent stays quiet"
    );

    // The global dispatcher's own selection, unchanged: enabled agents whose
    // triggers array contains the event. No user_id, no relevance.
    let trigger = serde_json::to_value(NotificationTrigger::OnImport).unwrap();
    let selected: Vec<String> = notification_agents::list_enabled(pool)
        .await
        .expect("list enabled")
        .into_iter()
        .filter(|a| a.triggers.as_array().is_some_and(|t| t.contains(&trigger)))
        .map(|a| a.name)
        .collect();

    assert_eq!(
        selected,
        vec!["Global Pushover", "Global Plex", "Global Web Push"],
        "every enabled global agent still fires for an entity nobody cares about"
    );
}

// ============================================================================
// Production wiring: what the chat agent actually does with one event
// ============================================================================

use api::notifications::google_chat::{delivery_targets, normalise_user};
use api::notifications::relevance::{ChatDelivery, any_identity_bound, chat_delivery};

const CHAT: &str = "google_chat";

fn scoped(delivery: ChatDelivery) -> Vec<Uuid> {
    match delivery {
        ChatDelivery::Scoped(recipients) => recipients.into_iter().map(|r| r.user_id).collect(),
        ChatDelivery::Unbound => panic!("expected a scoped delivery, got the unbound fallback"),
    }
}

async fn open_up_prefs(pool: &PgPool, user_id: Uuid) {
    // Quiet hours are on by default; a test that runs at 23:00 must not go red.
    let mut prefs = chat_notification_prefs::get(pool, user_id).await.unwrap();
    prefs.quiet_from = Some(time::Time::MIDNIGHT);
    prefs.quiet_to = Some(time::Time::MIDNIGHT);
    chat_notification_prefs::upsert(pool, &prefs).await.unwrap();
}

async fn identity_with_external(pool: &PgPool, user_id: Uuid, external_id: &str) {
    chat_identities::create(
        pool,
        &chat_identities::NewChatIdentity {
            transport: CHAT,
            address: None,
            external_id: Some(external_id),
            user_id,
            max_role: "contributor",
            enabled: true,
        },
    )
    .await
    .expect("create identity");
}

/// Phase 0 must keep working: with nothing paired, the agent still posts to the
/// single configured space exactly as it does today.
#[tokio::test]
async fn with_no_identities_bound_the_agent_falls_back_to_the_configured_space() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let now = OffsetDateTime::now_utc();

    let watcher = user(pool, "unbound-watcher@test.local").await;
    let movie_id = movie(pool, "Unbound Movie").await;
    watchlisted(pool, watcher, "movie", movie_id).await;

    assert!(!any_identity_bound(pool, CHAT).await.unwrap());
    assert!(matches!(
        chat_delivery(
            pool,
            CHAT,
            Some(movie_id),
            NotificationTrigger::OnImport,
            now
        )
        .await
        .unwrap(),
        ChatDelivery::Unbound
    ));

    // And it stays the fallback for something nobody cares about, because the
    // decision is about the install, not about this one event.
    let ignored = movie(pool, "Unbound And Ignored").await;
    assert!(matches!(
        chat_delivery(
            pool,
            CHAT,
            Some(ignored),
            NotificationTrigger::OnImport,
            now
        )
        .await
        .unwrap(),
        ChatDelivery::Unbound
    ));
}

/// The moment anyone pairs, the shared space stops being a destination — for
/// every event, including ones nobody is interested in.
#[tokio::test]
async fn one_bound_identity_switches_the_whole_agent_to_scoped() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let now = OffsetDateTime::now_utc();

    let paired = user(pool, "paired@test.local").await;
    identity_with_external(pool, paired, "users/1001").await;
    open_up_prefs(pool, paired).await;

    let uninteresting = movie(pool, "Interests Nobody").await;
    assert!(any_identity_bound(pool, CHAT).await.unwrap());
    assert!(
        scoped(
            chat_delivery(
                pool,
                CHAT,
                Some(uninteresting),
                NotificationTrigger::OnImport,
                now
            )
            .await
            .unwrap()
        )
        .is_empty(),
        "an event nobody cares about is silence, never a broadcast"
    );

    // An entity that resolves to nothing is silence too, not a broadcast.
    assert!(
        scoped(
            chat_delivery(
                pool,
                CHAT,
                Some(Uuid::now_v7()),
                NotificationTrigger::OnImport,
                now
            )
            .await
            .unwrap()
        )
        .is_empty()
    );
    assert!(
        scoped(
            chat_delivery(pool, CHAT, None, NotificationTrigger::OnImport, now)
                .await
                .unwrap()
        )
        .is_empty()
    );
}

#[tokio::test]
async fn two_interested_people_each_get_their_own_dm() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let now = OffsetDateTime::now_utc();

    let her = user(pool, "her@test.local").await;
    let him = user(pool, "him@test.local").await;
    let neither = user(pool, "neither@test.local").await;

    let show_id = show(pool, "Shared Favourite").await;
    let season_id = season(pool, show_id, 1).await;
    let ep = episode(pool, season_id, 1).await;
    let new_ep = episode(pool, season_id, 2).await;

    watched_episode(pool, her, ep).await;
    watchlisted(pool, him, "show", show_id).await;

    for id in [her, him, neither] {
        identity_with_external(pool, id, &format!("users/{}", id.simple())).await;
        open_up_prefs(pool, id).await;
    }

    let delivery = chat_delivery(pool, CHAT, Some(new_ep), NotificationTrigger::OnImport, now)
        .await
        .unwrap();

    let ChatDelivery::Scoped(recipients) = delivery else {
        panic!("expected scoped");
    };
    let mut got: Vec<Uuid> = recipients.iter().map(|r| r.user_id).collect();
    got.sort();
    assert_eq!(got, sorted(vec![her, him]));
    assert!(!got.contains(&neither));

    // Two people, two distinct DM targets — not one message to a shared space.
    let (targets, unreachable) = delivery_targets(&recipients);
    assert_eq!(unreachable, 0);
    assert_eq!(targets.len(), 2);
    assert_eq!(
        targets
            .iter()
            .map(|t| t.user_ref.clone())
            .collect::<Vec<_>>(),
        vec![
            format!("users/{}", her.simple()),
            format!("users/{}", him.simple())
        ]
    );
}

#[tokio::test]
async fn muted_and_quiet_hours_users_are_dropped_before_delivery() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let now = OffsetDateTime::now_utc();

    let awake = user(pool, "awake@test.local").await;
    let muted = user(pool, "muted-chat@test.local").await;
    let asleep = user(pool, "asleep@test.local").await;

    let movie_id = movie(pool, "Late Night Arrival").await;
    for id in [awake, muted, asleep] {
        watchlisted(pool, id, "movie", movie_id).await;
        identity_with_external(pool, id, &format!("users/{}", id.simple())).await;
        open_up_prefs(pool, id).await;
    }

    let mut muted_prefs = chat_notification_prefs::get(pool, muted).await.unwrap();
    muted_prefs.muted_until = Some(now + time::Duration::hours(3));
    chat_notification_prefs::upsert(pool, &muted_prefs)
        .await
        .unwrap();

    // A quiet window covering every hour of the day, so the assertion holds
    // whenever the suite runs.
    let mut asleep_prefs = chat_notification_prefs::get(pool, asleep).await.unwrap();
    asleep_prefs.quiet_from = Some(time::Time::MIDNIGHT);
    asleep_prefs.quiet_to = Some(time::Time::from_hms(23, 59, 59).unwrap());
    chat_notification_prefs::upsert(pool, &asleep_prefs)
        .await
        .unwrap();

    let got = scoped(
        chat_delivery(
            pool,
            CHAT,
            Some(movie_id),
            NotificationTrigger::OnImport,
            now,
        )
        .await
        .unwrap(),
    );
    assert_eq!(got, vec![awake]);
}

#[tokio::test]
async fn an_email_only_identity_is_counted_unreachable_not_broadcast() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let now = OffsetDateTime::now_utc();

    let addressable = user(pool, "addressable@test.local").await;
    let email_only = user(pool, "email-only@test.local").await;

    let movie_id = movie(pool, "Half Paired").await;
    for id in [addressable, email_only] {
        watchlisted(pool, id, "movie", movie_id).await;
        open_up_prefs(pool, id).await;
    }
    identity_with_external(pool, addressable, "users/2002").await;
    identity_for(pool, email_only, "email-only@example.com").await;

    let ChatDelivery::Scoped(recipients) = chat_delivery(
        pool,
        CHAT,
        Some(movie_id),
        NotificationTrigger::OnImport,
        now,
    )
    .await
    .unwrap() else {
        panic!("expected scoped");
    };
    assert_eq!(recipients.len(), 2);

    let (targets, unreachable) = delivery_targets(&recipients);
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].user_ref, "users/2002");
    assert_eq!(
        unreachable, 1,
        "no chat user id means no DM space, not a broadcast"
    );
}

/// A disabled identity does not keep the install in scoped mode on its own, and
/// an identity on another transport is not a Google Chat binding.
#[tokio::test]
async fn only_enabled_identities_on_this_transport_count() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let someone = user(pool, "disabled-identity@test.local").await;
    let identity_id = chat_identities::create(
        pool,
        &chat_identities::NewChatIdentity {
            transport: CHAT,
            address: None,
            external_id: Some("users/3003"),
            user_id: someone,
            max_role: "contributor",
            enabled: false,
        },
    )
    .await
    .expect("create identity")
    .id;
    assert!(!any_identity_bound(pool, CHAT).await.unwrap());

    chat_identities::update(
        pool,
        identity_id,
        &chat_identities::ChatIdentityPatch {
            enabled: Some(true),
            ..Default::default()
        },
    )
    .await
    .expect("enable identity");
    assert!(any_identity_bound(pool, CHAT).await.unwrap());
    assert!(!any_identity_bound(pool, "web").await.unwrap());
}

#[test]
fn user_resource_names_are_normalised() {
    for raw in ["users/1234", "1234", " users/1234 ", "/users/1234/"] {
        assert_eq!(normalise_user(raw), "users/1234", "normalising {raw:?}");
    }
}
