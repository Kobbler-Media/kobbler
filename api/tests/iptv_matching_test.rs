use crate::common::*;
use std::collections::HashMap;
use std::time::Instant;

/// Full integration test: parse real provider data, run matching pipeline,
/// insert into DB, verify results, measure performance.
///
/// Requires manually-downloaded fixture files (gitignored, ~125MB total).
/// Run with: cargo test -p api --test iptv_matching_test -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn test_matching_pipeline_with_real_data() {
    use api::iptv::iptv_org::IptvOrgDatabase;
    use api::iptv::matching;
    use api::iptv::normalization::{detect_language_from_category, normalize_stream_name};
    use api::iptv::xtream::XtreamStream;

    let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let streams_path = fixture_dir.join("streams.json");
    let streams_json = match std::fs::read_to_string(&streams_path) {
        Ok(s) => s,
        Err(_) => {
            eprintln!(
                "Skipping: {} not found. Download test fixtures first.",
                streams_path.display()
            );
            return;
        }
    };
    let categories_json = match std::fs::read_to_string(fixture_dir.join("categories.json")) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("Skipping: categories.json not found.");
            return;
        }
    };
    let iptv_org_json = match std::fs::read_to_string(fixture_dir.join("iptv-org.json")) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("Skipping: iptv-org.json not found.");
            return;
        }
    };

    // Parse streams using our actual XtreamStream struct (tests flexi_string deserialization)
    let parse_start = Instant::now();
    let streams: Vec<XtreamStream> = serde_json::from_str(&streams_json).expect("parse streams");
    let parse_time = parse_start.elapsed();
    println!("\nParsed {} streams in {:?}", streams.len(), parse_time);
    assert!(
        parse_time.as_secs() < 5,
        "stream parsing too slow: {:?}",
        parse_time
    );

    // Parse categories
    let raw_cats: Vec<serde_json::Value> =
        serde_json::from_str(&categories_json).expect("parse categories");
    let categories: HashMap<String, String> = raw_cats
        .iter()
        .filter_map(|c| {
            Some((
                c["category_id"].as_str()?.to_string(),
                c["category_name"].as_str()?.to_string(),
            ))
        })
        .collect();
    println!("Categories: {}", categories.len());

    // Parse iptv-org
    let org_start = Instant::now();
    let iptv_org = IptvOrgDatabase::from_json(&iptv_org_json);
    let org_time = org_start.elapsed();
    println!(
        "iptv-org: {} channels indexed in {:?}",
        iptv_org.len(),
        org_time
    );
    assert!(
        org_time.as_secs() < 10,
        "iptv-org indexing too slow: {:?}",
        org_time
    );

    // =========================================================================
    // Normalization analysis (no DB)
    // =========================================================================
    let norm_start = Instant::now();
    let mut lang_counts: HashMap<String, u32> = HashMap::new();
    let mut quality_counts: HashMap<String, u32> = HashMap::new();
    let mut org_matches = 0u32;
    let mut empty_normalized = 0u32;

    for stream in &streams {
        let name = match &stream.name {
            Some(n) if !n.trim().is_empty() => n.as_str(),
            _ => continue,
        };
        let normalized = normalize_stream_name(name);
        *quality_counts
            .entry(normalized.quality.clone())
            .or_default() += 1;

        // Language from name prefix + category
        let cat_name = stream
            .category_id
            .as_deref()
            .and_then(|cid| categories.get(cid));
        let lang = normalized.language.clone().or_else(|| {
            cat_name
                .and_then(|n| detect_language_from_category(n))
                .map(String::from)
        });
        if let Some(l) = lang {
            *lang_counts.entry(l).or_default() += 1;
        }

        if normalized.normalized_name.len() <= 2 {
            empty_normalized += 1;
        }

        if iptv_org
            .lookup(&normalized.normalized_name, normalized.country.as_deref())
            .is_some()
        {
            org_matches += 1;
        }
    }
    let norm_time = norm_start.elapsed();
    println!(
        "Normalization: {:?} for {} streams",
        norm_time,
        streams.len()
    );
    assert!(
        norm_time.as_secs() < 10,
        "normalization too slow: {:?}",
        norm_time
    );

    let total_with_lang: u32 = lang_counts.values().sum();
    println!("\n--- Normalization Results ---");
    println!("Quality: {:?}", quality_counts);
    println!(
        "Language detected: {} ({:.1}%)",
        total_with_lang,
        100.0 * total_with_lang as f64 / streams.len() as f64
    );
    println!(
        "iptv-org matches: {} ({:.1}%)",
        org_matches,
        100.0 * org_matches as f64 / streams.len() as f64
    );
    println!(
        "Names normalized to <=2 chars: {} (problematic)",
        empty_normalized
    );

    // =========================================================================
    // Full DB pipeline: create provider, run matching, verify results
    // =========================================================================
    let server = TestServer::start_api_only().await;

    // Create a provider
    let provider_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO iptv_providers (id, name, url, username, password) VALUES ($1, 'Test Provider', 'http://test.example.com', 'user', 'pass')",
    )
    .bind(provider_id)
    .execute(&server.pool)
    .await
    .expect("create provider");

    // Run the full matching pipeline against the DB
    let match_start = Instant::now();
    let result = matching::match_streams(
        &server.pool,
        provider_id,
        streams,
        &categories,
        &iptv_org,
        true, // auto_create_channels
    )
    .await
    .expect("matching pipeline should succeed");
    let match_time = match_start.elapsed();

    println!("\n--- Matching Pipeline Results ({:?}) ---", match_time);
    println!("Matched: {}", result.matched);
    println!("New channels created: {}", result.new_channels);
    println!("Unmatched (staged): {}", result.unmatched);

    // Performance assertion: 50k streams should complete in under 60s
    assert!(
        match_time.as_secs() < 60,
        "matching pipeline too slow: {:?} for {} streams",
        match_time,
        result.matched + result.new_channels + result.unmatched
    );

    // Verify DB state
    let channel_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM iptv_channels")
        .fetch_one(&server.pool)
        .await
        .expect("count channels");
    let source_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM iptv_sources")
        .fetch_one(&server.pool)
        .await
        .expect("count sources");
    let unmatched_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM iptv_unmatched_sources")
        .fetch_one(&server.pool)
        .await
        .expect("count unmatched");

    println!("\n--- DB State ---");
    println!("Channels: {channel_count}");
    println!("Sources: {source_count}");
    println!("Unmatched: {unmatched_count}");

    // Sanity assertions
    assert!(channel_count > 0, "should create at least some channels");
    assert!(source_count > 0, "should insert at least some sources");
    assert_eq!(
        source_count as u32 + unmatched_count as u32,
        result.matched + result.new_channels + result.unmatched,
        "sources + unmatched should equal total processed"
    );

    // Check for duplicate channels (same normalized_name)
    let dupes: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (SELECT normalized_name, language, country FROM iptv_channels GROUP BY normalized_name, language, country HAVING COUNT(*) > 1) dupes",
    )
    .fetch_one(&server.pool)
    .await
    .expect("check dupes");
    println!("Duplicate channels: {dupes}");
    assert_eq!(dupes, 0, "should not have duplicate channels");

    // Check sources with channel reference
    let orphan_sources: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM iptv_sources s WHERE NOT EXISTS (SELECT 1 FROM iptv_channels c WHERE c.id = s.channel_id)",
    )
    .fetch_one(&server.pool)
    .await
    .expect("check orphans");
    assert_eq!(orphan_sources, 0, "should not have orphaned sources");

    // Sample some channels to verify quality
    type SampleRow = (String, String, Option<String>, Option<String>, i64);
    let sample_channels: Vec<SampleRow> = sqlx::query_as(
        r#"
        SELECT c.name, c.normalized_name, c.language, c.category,
               (SELECT COUNT(*) FROM iptv_sources WHERE channel_id = c.id) as source_count
        FROM iptv_channels c
        ORDER BY source_count DESC
        LIMIT 20
        "#,
    )
    .fetch_all(&server.pool)
    .await
    .expect("sample channels");

    println!("\n--- Top 20 Channels by Source Count ---");
    for (name, normalized, lang, cat, count) in &sample_channels {
        println!(
            "  [{count:>3} sources] {name} (normalized: '{normalized}', lang: {}, cat: {})",
            lang.as_deref().unwrap_or("-"),
            cat.as_deref().unwrap_or("-")
        );
    }

    // =========================================================================
    // EPG sync test (if fixture exists)
    // =========================================================================
    let epg_path = fixture_dir.join("epg.xml");
    if epg_path.exists() {
        let epg_start = Instant::now();
        let epg_synced = api::iptv::epg::sync_epg_from_file(&server.pool, &epg_path)
            .await
            .expect("EPG sync should succeed");
        let epg_time = epg_start.elapsed();

        let epg_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM iptv_epg")
            .fetch_one(&server.pool)
            .await
            .expect("count EPG");

        println!("\n--- EPG Sync ---");
        println!("  Synced: {epg_synced} entries in {:?}", epg_time);
        println!("  DB EPG rows: {epg_count}");

        // Sample EPG entries
        type EpgSample = (String, Option<String>, String, String);
        let epg_samples: Vec<EpgSample> = sqlx::query_as(
            r#"
            SELECT c.name, e.title, e.start_time::text, e.stop_time::text
            FROM iptv_epg e
            JOIN iptv_channels c ON c.id = e.channel_id
            ORDER BY e.start_time
            LIMIT 10
            "#,
        )
        .fetch_all(&server.pool)
        .await
        .expect("sample EPG");

        println!("  Sample EPG entries:");
        for (channel, title, start, stop) in &epg_samples {
            println!(
                "    {channel}: {} ({start} - {stop})",
                title.as_deref().unwrap_or("?")
            );
        }

        // Check channels with EPG data
        let channels_with_epg: i64 =
            sqlx::query_scalar("SELECT COUNT(DISTINCT channel_id) FROM iptv_epg")
                .fetch_one(&server.pool)
                .await
                .expect("channels with EPG");
        println!("  Channels with EPG: {channels_with_epg}");

        assert!(epg_synced > 0, "should sync at least some EPG entries");
        assert!(
            epg_time.as_secs() < 120,
            "EPG sync too slow: {:?}",
            epg_time
        );
    } else {
        println!("\n--- EPG Sync: SKIPPED (no epg.xml fixture) ---");
    }

    // =========================================================================
    // Deep data validation
    // =========================================================================
    println!("\n--- Deep Data Validation ---");

    // 1. BBC ONE quality variants should group — NL has HD/FHD/4K/HEVC variants
    type CountRow = (i64,);
    let (bbc_nl_sources,): CountRow = sqlx::query_as(
        "SELECT COUNT(*) FROM iptv_sources s JOIN iptv_channels c ON c.id = s.channel_id WHERE c.normalized_name = 'bbc one' AND c.country = 'NL'"
    ).fetch_one(&server.pool).await.expect("bbc one NL query");
    println!("  BBC ONE (NL) sources: {bbc_nl_sources}");
    assert!(
        bbc_nl_sources >= 3,
        "BBC ONE (NL) should have HD/FHD/4K/HEVC sources, got {bbc_nl_sources}"
    );

    // 2. TNT Sports 1 and 2 should be SEPARATE channels
    let (tnt1_id,): (i64,) = sqlx::query_as(
        "SELECT COUNT(DISTINCT id) FROM iptv_channels WHERE normalized_name = 'tnt sports 1'",
    )
    .fetch_one(&server.pool)
    .await
    .expect("tnt1");
    let (tnt2_id,): (i64,) = sqlx::query_as(
        "SELECT COUNT(DISTINCT id) FROM iptv_channels WHERE normalized_name = 'tnt sports 2'",
    )
    .fetch_one(&server.pool)
    .await
    .expect("tnt2");
    println!("  TNT Sports 1 channels: {tnt1_id}, TNT Sports 2 channels: {tnt2_id}");
    assert!(
        tnt1_id >= 1 && tnt2_id >= 1,
        "TNT Sports 1 and 2 should be separate channels"
    );

    // 3. Canal+ should be "canal plus" (+ converted)
    let (canal_count,): CountRow =
        sqlx::query_as("SELECT COUNT(*) FROM iptv_channels WHERE normalized_name = 'canal plus'")
            .fetch_one(&server.pool)
            .await
            .expect("canal plus");
    println!("  Canal Plus channels: {canal_count}");
    assert!(canal_count >= 1, "Canal+ should normalize to 'canal plus'");

    // 4. No channels with empty or very short normalized names
    let (short_channels,): CountRow =
        sqlx::query_as("SELECT COUNT(*) FROM iptv_channels WHERE LENGTH(normalized_name) <= 2")
            .fetch_one(&server.pool)
            .await
            .expect("short names");
    println!("  Channels with name <= 2 chars: {short_channels}");
    assert_eq!(
        short_channels, 0,
        "should not have ultra-short channel names"
    );

    // 5. Sources all reference valid channels
    let (orphan_sources,): CountRow = sqlx::query_as(
        "SELECT COUNT(*) FROM iptv_sources s WHERE NOT EXISTS (SELECT 1 FROM iptv_channels c WHERE c.id = s.channel_id)"
    ).fetch_one(&server.pool).await.expect("orphan check");
    assert_eq!(orphan_sources, 0, "no orphaned sources");

    // 6. EPG entries all reference valid channels
    if epg_path.exists() {
        let (orphan_epg,): CountRow = sqlx::query_as(
            "SELECT COUNT(*) FROM iptv_epg e WHERE NOT EXISTS (SELECT 1 FROM iptv_channels c WHERE c.id = e.channel_id)"
        ).fetch_one(&server.pool).await.expect("orphan epg check");
        println!("  Orphaned EPG entries: {orphan_epg}");
        assert_eq!(orphan_epg, 0, "no orphaned EPG entries");

        // Only validate times/joins if EPG rows survived pruning (test fixtures may contain expired data)
        let (epg_remaining,): CountRow = sqlx::query_as("SELECT COUNT(*) FROM iptv_epg")
            .fetch_one(&server.pool)
            .await
            .expect("count epg");

        if epg_remaining > 0 {
            // 7. EPG times are sane (not in the distant past or future)
            type TimeRow = (String, String);
            let (min_start, max_stop): TimeRow =
                sqlx::query_as("SELECT MIN(start_time)::text, MAX(stop_time)::text FROM iptv_epg")
                    .fetch_one(&server.pool)
                    .await
                    .expect("epg time range");
            println!("  EPG time range: {min_start} to {max_stop}");

            // 8. Sample EPG-channel join to verify mapping
            type EpgJoinRow = (String, Option<String>, String);
            let epg_joins: Vec<EpgJoinRow> = sqlx::query_as(
                r#"SELECT c.name, e.title, e.start_time::text
                   FROM iptv_epg e
                   JOIN iptv_channels c ON c.id = e.channel_id
                   WHERE c.normalized_name IN ('bbc one', 'tnt sports 1', 'sky sports main event', 'espn')
                   ORDER BY c.name, e.start_time
                   LIMIT 15"#
            ).fetch_all(&server.pool).await.expect("epg joins");
            println!("  EPG entries for key channels:");
            for (channel, title, start) in &epg_joins {
                println!(
                    "    {channel}: {} @ {start}",
                    title.as_deref().unwrap_or("?")
                );
            }
            assert!(!epg_joins.is_empty(), "key channels should have EPG data");
        } else {
            println!("  EPG rows all pruned (expired fixture data) — skipping time/join checks");
        }
    }

    // 9. Check language distribution makes sense
    type LangRow = (Option<String>, i64);
    let lang_dist: Vec<LangRow> = sqlx::query_as(
        "SELECT language, COUNT(*) as cnt FROM iptv_channels GROUP BY language ORDER BY cnt DESC LIMIT 10"
    ).fetch_all(&server.pool).await.expect("lang dist");
    println!("  Channel language distribution:");
    for (lang, cnt) in &lang_dist {
        println!("    {}: {cnt}", lang.as_deref().unwrap_or("NULL"));
    }

    // 10. Check source quality distribution
    type QualRow = (String, i64);
    let quality_dist: Vec<QualRow> = sqlx::query_as(
        "SELECT quality, COUNT(*) as cnt FROM iptv_sources GROUP BY quality ORDER BY cnt DESC",
    )
    .fetch_all(&server.pool)
    .await
    .expect("quality dist");
    println!("  Source quality distribution:");
    for (q, cnt) in &quality_dist {
        println!("    {q}: {cnt}");
    }

    // 11. Verify match methods
    type MethodRow = (Option<String>, i64);
    let method_dist: Vec<MethodRow> = sqlx::query_as(
        "SELECT match_method, COUNT(*) as cnt FROM iptv_sources GROUP BY match_method ORDER BY cnt DESC"
    ).fetch_all(&server.pool).await.expect("method dist");
    println!("  Source match method distribution:");
    for (method, cnt) in &method_dist {
        println!("    {}: {cnt}", method.as_deref().unwrap_or("NULL"));
    }

    // =========================================================================
    // Second provider matching test
    // =========================================================================
    println!("\n--- Second Provider Test ---");

    // Snapshot counts before second provider
    let channels_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM iptv_channels")
        .fetch_one(&server.pool)
        .await
        .expect("count channels before");
    let sources_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM iptv_sources")
        .fetch_one(&server.pool)
        .await
        .expect("count sources before");

    // Create a second provider
    let provider2_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO iptv_providers (id, name, url, username, password) VALUES ($1, 'Second Provider', 'http://test2.example.com', 'user2', 'pass2')",
    )
    .bind(provider2_id)
    .execute(&server.pool)
    .await
    .expect("create second provider");

    // Re-parse the first 1000 streams (simulating overlap from a second provider)
    let streams2: Vec<XtreamStream> = serde_json::from_str(&streams_json).expect("reparse streams");
    let subset: Vec<XtreamStream> = streams2.into_iter().take(1000).collect();
    let subset_len = subset.len();

    let match2_start = Instant::now();
    let result2 = matching::match_streams(
        &server.pool,
        provider2_id,
        subset,
        &categories,
        &iptv_org,
        true,
    )
    .await
    .expect("second provider matching should succeed");
    let match2_time = match2_start.elapsed();

    println!(
        "  Second provider: {} streams in {:?}",
        subset_len, match2_time
    );
    println!("  Matched existing: {}", result2.matched);
    println!("  New channels: {}", result2.new_channels);
    println!("  Unmatched: {}", result2.unmatched);

    // Verify: channel count should NOT increase significantly (streams overlap)
    let channels_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM iptv_channels")
        .fetch_one(&server.pool)
        .await
        .expect("count channels after");
    let sources_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM iptv_sources")
        .fetch_one(&server.pool)
        .await
        .expect("count sources after");

    println!(
        "  Channels: {} -> {} (delta: {})",
        channels_before,
        channels_after,
        channels_after - channels_before
    );
    println!(
        "  Sources: {} -> {} (delta: {})",
        sources_before,
        sources_after,
        sources_after - sources_before
    );

    // The matched + new_channels streams should have created new sources
    // (different provider_id) but reused existing channels
    let new_sources = sources_after - sources_before;
    assert!(
        new_sources > 0,
        "second provider should create new sources, got delta={new_sources}"
    );

    // Verify no duplicate channels were created (same normalized_name)
    let dupes_after: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (SELECT normalized_name, language, country FROM iptv_channels GROUP BY normalized_name, language, country HAVING COUNT(*) > 1) dupes",
    )
    .fetch_one(&server.pool)
    .await
    .expect("check dupes after second provider");
    assert_eq!(
        dupes_after, 0,
        "second provider should not create duplicate channels"
    );

    // Verify multi-provider sources: find channels with sources from both providers
    type MultiProvRow = (String, i64);
    let multi_provider: Vec<MultiProvRow> = sqlx::query_as(
        r#"
        SELECT c.name, COUNT(DISTINCT s.provider_id) as provider_count
        FROM iptv_channels c
        JOIN iptv_sources s ON s.channel_id = c.id
        GROUP BY c.id, c.name
        HAVING COUNT(DISTINCT s.provider_id) > 1
        ORDER BY provider_count DESC
        LIMIT 10
        "#,
    )
    .fetch_all(&server.pool)
    .await
    .expect("multi-provider channels");

    println!(
        "  Channels with sources from both providers: {}",
        multi_provider.len()
    );
    for (name, cnt) in &multi_provider {
        println!("    {name}: {cnt} providers");
    }
    assert!(
        !multi_provider.is_empty(),
        "at least some channels should have sources from both providers"
    );

    println!("\n--- Performance Summary ---");
    println!("  Parse: {:?}", parse_time);
    println!("  Normalize: {:?}", norm_time);
    println!("  iptv-org index: {:?}", org_time);
    println!("  Full matching + DB: {:?}", match_time);
    println!("  Second provider ({}): {:?}", subset_len, match2_time);
    println!(
        "  Total: {:?}",
        parse_time + norm_time + org_time + match_time + match2_time
    );
}
