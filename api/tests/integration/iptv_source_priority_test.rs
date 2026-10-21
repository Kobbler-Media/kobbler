//! Finding #10: IPTV source selection tie-breaks on `iptv_sources.priority`.
//!
//! Two defects, both covered here:
//!   1. Direction — `priority` is 0=best..100=worst (`iptv::quality::compute_priority`,
//!      `idx_iptv_sources_failover ON (channel_id, priority ASC, health_score DESC)`),
//!      so the best source is the *lowest* priority. Two backfills sorted DESC.
//!   2. Totality — `priority` is `NOT NULL DEFAULT 50` with no per-channel uniqueness,
//!      so ties are the normal case and `DISTINCT ON` then picks an arbitrary row.

use crate::common::*;
use api::repositories::iptv_sources;
use sqlx::PgPool;
use uuid::Uuid;

const LOW_ID: &str = "00000000-0000-0000-0000-0000000000aa";
const HIGH_ID: &str = "ffffffff-0000-0000-0000-0000000000ff";

fn uuid(s: &str) -> Uuid {
    Uuid::parse_str(s).expect("valid uuid")
}

async fn make_provider(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        r#"INSERT INTO iptv_providers (name, url, username, password)
           VALUES ($1, $2, 'u', 'p') RETURNING id"#,
    )
    .bind(name)
    .bind(format!("http://{name}.example.com"))
    .fetch_one(pool)
    .await
    .expect("insert provider")
}

async fn make_channel(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        r#"INSERT INTO iptv_channels (name, normalized_name) VALUES ($1, $2) RETURNING id"#,
    )
    .bind(name)
    .bind(name.to_ascii_lowercase())
    .fetch_one(pool)
    .await
    .expect("insert channel")
}

#[allow(clippy::too_many_arguments)]
async fn make_source(
    pool: &PgPool,
    id: Uuid,
    channel_id: Uuid,
    provider_id: Uuid,
    remote_stream_id: i64,
    priority: i32,
    category_name: Option<&str>,
    icon_url: Option<&str>,
    archive_days: Option<&str>,
) {
    let metadata = match archive_days {
        Some(d) => serde_json::json!({ "tv_archive": "1", "tv_archive_duration": d }),
        None => serde_json::json!({}),
    };
    sqlx::query(
        r#"INSERT INTO iptv_sources
             (id, channel_id, provider_id, remote_stream_id, remote_category_name,
              icon_url, priority, metadata)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
    )
    .bind(id)
    .bind(channel_id)
    .bind(provider_id)
    .bind(remote_stream_id)
    .bind(category_name)
    .bind(icon_url)
    .bind(priority)
    .bind(metadata)
    .execute(pool)
    .await
    .expect("insert source");
}

async fn channel_category(pool: &PgPool, id: Uuid) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT category FROM iptv_channels WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("read category")
}

async fn channel_icon(pool: &PgPool, id: Uuid) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT icon_url FROM iptv_channels WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("read icon")
}

async fn clear_icon(pool: &PgPool, id: Uuid) {
    sqlx::query("UPDATE iptv_channels SET icon_url = NULL WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .expect("clear icon");
}

async fn clear_category(pool: &PgPool, id: Uuid) {
    sqlx::query("UPDATE iptv_channels SET category = NULL WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .expect("clear category");
}

// =============================================================================
// Direction: lowest `priority` is the preferred source
// =============================================================================

#[tokio::test]
async fn category_backfill_prefers_lowest_priority_source() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let provider = make_provider(pool, "cat-dir").await;
    let channel = make_channel(pool, "Category Direction").await;

    // Insert worst-first so insertion order cannot be mistaken for correctness.
    make_source(
        pool,
        uuid(HIGH_ID),
        channel,
        provider,
        1,
        50, // sd — least preferred
        Some("News Channels"),
        None,
        None,
    )
    .await;
    make_source(
        pool,
        uuid(LOW_ID),
        channel,
        provider,
        2,
        10, // 4k — most preferred
        Some("Sports Channels"),
        None,
        None,
    )
    .await;

    api::iptv::sync::backfill_channel_categories(pool)
        .await
        .expect("backfill categories");

    assert_eq!(
        channel_category(pool, channel).await.as_deref(),
        Some("sports"),
        "priority is 0=best..100=worst, so the priority-10 source must win, not the priority-50 one"
    );
}

#[tokio::test]
async fn icon_backfill_prefers_lowest_priority_source() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let provider = make_provider(pool, "icon-dir").await;
    let channel = make_channel(pool, "Icon Direction").await;

    make_source(
        pool,
        uuid(HIGH_ID),
        channel,
        provider,
        1,
        50,
        None,
        Some("http://icons.example.com/worst.png"),
        None,
    )
    .await;
    make_source(
        pool,
        uuid(LOW_ID),
        channel,
        provider,
        2,
        10,
        None,
        Some("http://icons.example.com/best.png"),
        None,
    )
    .await;

    api::iptv::matching::backfill_channel_icons(pool)
        .await
        .expect("backfill icons");

    assert_eq!(
        channel_icon(pool, channel).await.as_deref(),
        Some("http://icons.example.com/best.png"),
        "the priority-10 (4k) source's icon must win over the priority-50 (sd) one"
    );
}

#[tokio::test]
async fn catchup_prefers_lowest_priority_source() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let provider = make_provider(pool, "catchup-dir").await;
    let channel = make_channel(pool, "Catchup Direction").await;

    make_source(
        pool,
        uuid(HIGH_ID),
        channel,
        provider,
        1,
        50,
        None,
        None,
        Some("3"),
    )
    .await;
    make_source(
        pool,
        uuid(LOW_ID),
        channel,
        provider,
        2,
        10,
        None,
        None,
        Some("7"),
    )
    .await;

    let map = iptv_sources::get_catchup_channels(pool, &[channel])
        .await
        .expect("catchup channels");

    assert_eq!(
        map.get(&channel),
        Some(&7),
        "catchup info must come from the preferred (lowest-priority) source"
    );
}

#[tokio::test]
async fn list_by_channel_returns_failover_order() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let provider = make_provider(pool, "failover-order").await;
    let channel = make_channel(pool, "Failover Order").await;

    make_source(
        pool,
        uuid(HIGH_ID),
        channel,
        provider,
        1,
        50,
        None,
        None,
        None,
    )
    .await;
    make_source(
        pool,
        uuid(LOW_ID),
        channel,
        provider,
        2,
        10,
        None,
        None,
        None,
    )
    .await;

    let sources = iptv_sources::list_by_channel(pool, channel)
        .await
        .expect("list sources");

    let priorities: Vec<i32> = sources.iter().map(|s| s.priority).collect();
    assert_eq!(
        priorities,
        vec![10, 50],
        "failover tries sources in list order, so the best (lowest priority) must be first"
    );
}

// =============================================================================
// Totality: equal priorities must resolve to the same row every time
// =============================================================================

#[tokio::test]
async fn category_backfill_breaks_priority_ties_deterministically() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let provider = make_provider(pool, "cat-tie").await;
    let channel = make_channel(pool, "Category Tie").await;

    // Both at the schema default (50): the ordinary case, not an edge case.
    make_source(
        pool,
        uuid(HIGH_ID),
        channel,
        provider,
        1,
        50,
        Some("News Channels"),
        None,
        None,
    )
    .await;
    make_source(
        pool,
        uuid(LOW_ID),
        channel,
        provider,
        2,
        50,
        Some("Sports Channels"),
        None,
        None,
    )
    .await;

    let mut seen = Vec::new();
    for _ in 0..5 {
        clear_category(pool, channel).await;
        sqlx::query("ANALYZE iptv_sources")
            .execute(pool)
            .await
            .expect("analyze");
        api::iptv::sync::backfill_channel_categories(pool)
            .await
            .expect("backfill categories");
        seen.push(channel_category(pool, channel).await);
    }

    assert!(
        seen.windows(2).all(|w| w[0] == w[1]),
        "tied priorities must resolve to one stable source, got {seen:?}"
    );
    assert_eq!(
        seen[0].as_deref(),
        Some("sports"),
        "the tie-break is source id ASC, so {LOW_ID} must win"
    );
}

#[tokio::test]
async fn icon_backfill_breaks_priority_ties_deterministically() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let provider = make_provider(pool, "icon-tie").await;
    let channel = make_channel(pool, "Icon Tie").await;

    make_source(
        pool,
        uuid(HIGH_ID),
        channel,
        provider,
        1,
        50,
        None,
        Some("http://icons.example.com/high-id.png"),
        None,
    )
    .await;
    make_source(
        pool,
        uuid(LOW_ID),
        channel,
        provider,
        2,
        50,
        None,
        Some("http://icons.example.com/low-id.png"),
        None,
    )
    .await;

    let mut seen = Vec::new();
    for _ in 0..5 {
        clear_icon(pool, channel).await;
        sqlx::query("ANALYZE iptv_sources")
            .execute(pool)
            .await
            .expect("analyze");
        api::iptv::matching::backfill_channel_icons(pool)
            .await
            .expect("backfill icons");
        seen.push(channel_icon(pool, channel).await);
    }

    assert!(
        seen.windows(2).all(|w| w[0] == w[1]),
        "tied priorities must resolve to one stable source, got {seen:?}"
    );
    assert_eq!(
        seen[0].as_deref(),
        Some("http://icons.example.com/low-id.png"),
        "the tie-break is source id ASC, so {LOW_ID} must win"
    );
}

#[tokio::test]
async fn catchup_breaks_priority_ties_deterministically() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let provider = make_provider(pool, "catchup-tie").await;
    let channel = make_channel(pool, "Catchup Tie").await;

    make_source(
        pool,
        uuid(HIGH_ID),
        channel,
        provider,
        1,
        50,
        None,
        None,
        Some("3"),
    )
    .await;
    make_source(
        pool,
        uuid(LOW_ID),
        channel,
        provider,
        2,
        50,
        None,
        None,
        Some("14"),
    )
    .await;

    let mut seen = Vec::new();
    for _ in 0..5 {
        sqlx::query("ANALYZE iptv_sources")
            .execute(pool)
            .await
            .expect("analyze");
        let map = iptv_sources::get_catchup_channels(pool, &[channel])
            .await
            .expect("catchup channels");
        seen.push(map.get(&channel).copied());
    }

    assert!(
        seen.windows(2).all(|w| w[0] == w[1]),
        "tied priorities must resolve to one stable source, got {seen:?}"
    );
    assert_eq!(
        seen[0],
        Some(14),
        "the tie-break is source id ASC, so {LOW_ID} must win"
    );
}

#[tokio::test]
async fn list_by_channel_breaks_full_ties_deterministically() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let provider = make_provider(pool, "list-tie").await;
    let channel = make_channel(pool, "List Tie").await;

    // Equal priority *and* equal health_score (both schema defaults).
    make_source(
        pool,
        uuid(HIGH_ID),
        channel,
        provider,
        1,
        50,
        None,
        None,
        None,
    )
    .await;
    make_source(
        pool,
        uuid(LOW_ID),
        channel,
        provider,
        2,
        50,
        None,
        None,
        None,
    )
    .await;

    let mut seen = Vec::new();
    for _ in 0..5 {
        sqlx::query("ANALYZE iptv_sources")
            .execute(pool)
            .await
            .expect("analyze");
        let sources = iptv_sources::list_by_channel(pool, channel)
            .await
            .expect("list sources");
        seen.push(sources.iter().map(|s| s.id).collect::<Vec<_>>());
    }

    assert!(
        seen.windows(2).all(|w| w[0] == w[1]),
        "failover order must be stable across calls, got {seen:?}"
    );
    assert_eq!(
        seen[0],
        vec![uuid(LOW_ID), uuid(HIGH_ID)],
        "fully tied sources fall back to id ASC"
    );
}
