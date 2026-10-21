#![recursion_limit = "512"]

use api::{
    DbPool, RuntimeConfigManager, auth,
    blob_storage::BlobStorage,
    config::Config,
    create_app_state, create_router,
    db::{connect, migrate},
    kobo_handler::{DatabaseLogger, KobblerHandler},
    nats, repositories,
    search::{SearchBackend, SearchBackendChoice, SqlFallbackBackend},
    watcher::{WatcherConfig, WatcherService},
};
use kobo_proxy::{ProxyConfig, run_proxy};
use rand::distr::{Alphanumeric, SampleString};
use std::path::Path;
use std::sync::Arc;
use std::{env, net::SocketAddr};
use tower_http::trace::{DefaultOnResponse, OnResponse, TraceLayer};
use tracing::{Level, error, info, span};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    // Install rustls crypto provider before any TLS operations
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    dotenvy::dotenv().ok();

    let env_filter = tracing_subscriber::EnvFilter::from_default_env();
    let fmt_layer = tracing_subscriber::fmt::layer();

    #[cfg(feature = "tokio-console")]
    {
        let console_layer = std::env::var("TOKIO_CONSOLE")
            .ok()
            .filter(|v| v == "1")
            .map(|_| {
                eprintln!("[watchdog] tokio-console enabled — connect with: tokio-console http://<host>:6669");
                console_subscriber::ConsoleLayer::builder()
                    .server_addr(([0, 0, 0, 0], 6669u16))
                    .spawn()
            });
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .with(console_layer)
            .init();
    }

    #[cfg(not(feature = "tokio-console"))]
    {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .init();
    }

    let config = Config::from_env();
    let pool = match connect(&config.database_url, config.database_max_connections).await {
        Ok(pool) => pool,
        Err(err) => {
            error!("Failed to connect to database: {err}");
            return;
        }
    };

    if let Err(err) = migrate(&pool).await {
        error!("Database migration failed: {err}");
        return;
    }

    // Dev mode: auto-restore from snapshot if DEV_AUTO_RESTORE=1 and snapshot exists
    // Only restores if database is fresh (no users yet - bootstrap hasn't run)
    if env::var("DEV_AUTO_RESTORE")
        .ok()
        .filter(|s| s == "1")
        .is_some()
    {
        let snapshot_path = Path::new(&config.config_path)
            .parent()
            .unwrap_or(Path::new("/config"))
            .join("dev-snapshot.tar.gz");

        if snapshot_path.exists() {
            // Check if database is fresh (no users = hasn't been bootstrapped or restored)
            let user_count = repositories::users::count(&pool).await.unwrap_or(0);

            if user_count == 0 {
                info!(
                    "Dev mode: fresh database detected, restoring from {:?}",
                    snapshot_path
                );
                match api::backup::restore_backup(&pool, &config.storage_root, &snapshot_path).await
                {
                    Ok(stats) => {
                        info!(
                            "Dev restore completed: {} tables, {} books, {} files",
                            stats.tables_restored, stats.books_restored, stats.files_restored
                        );
                    }
                    Err(e) => {
                        tracing::warn!("Dev restore failed (continuing): {}", e);
                    }
                }
            } else {
                info!(
                    "Dev mode: database already initialized ({} users), skipping restore",
                    user_count
                );
            }
        }
    }

    if let Err(err) = bootstrap_admin(&pool, &config.admin_email).await {
        error!("Admin bootstrap failed: {err}");
        return;
    }

    if let Err(err) = bootstrap_dev_ingest_token(&pool).await {
        error!("Dev ingest token bootstrap failed: {err}");
        return;
    }

    // Initialize runtime config from file
    let runtime_config = match RuntimeConfigManager::new(config.config_path.clone()) {
        Ok(mgr) => mgr,
        Err(err) => {
            error!(
                "Failed to load runtime config from {}: {err}",
                config.config_path
            );
            return;
        }
    };

    // Initialize NATS (required for job processing)
    let nats_client = match nats::init_nats().await {
        Ok(client) => client,
        Err(err) => {
            error!("{err}");
            return;
        }
    };
    let nats_state = nats::NatsState::new(nats_client.clone());

    // Initialize MeiliSearch + reindex check and detect hardware acceleration concurrently
    let rt_config = runtime_config.get().await;
    // Read the resolved config (kobbler.toml, then MEILI_URL) rather than
    // Config::meili_url, whose localhost default would enable Meilisearch for
    // installs that never asked for it.
    let meili_url = rt_config.meili_url.clone().unwrap_or_default();
    let meili_master_key = rt_config
        .meili_master_key
        .clone()
        .unwrap_or_else(|| config.meili_master_key.clone());
    let search_choice = SearchBackendChoice::from_config(&meili_url, &meili_master_key);
    let hw_preference = rt_config
        .hw_encoder
        .as_deref()
        .unwrap_or("auto")
        .to_string();
    drop(rt_config);

    let pool_clone = pool.clone();
    let runtime_config_clone = runtime_config.clone();
    let ((search_backend, needs_reindex), (hw_accel, encoder_caps, decoder_caps)) = tokio::join!(
        async {
            let meili_state = match search_choice {
                SearchBackendChoice::SqlFallback => {
                    info!("Meilisearch not configured; search is running in SQL-fallback mode");
                    let backend: Arc<dyn SearchBackend> = Arc::new(SqlFallbackBackend::new());
                    return (backend, false);
                }
                SearchBackendChoice::Meili(state) => state,
            };

            if let Err(err) = meili_state.init_indexes().await {
                error!("Failed to initialize MeiliSearch indexes: {err}");
            } else {
                info!("MeiliSearch indexes initialized");
                // Configure chat workspace if chat is enabled
                let rt = runtime_config_clone.get().await;
                if rt.chat_enabled {
                    let (client, _) = meili_state.get_client().await;
                    if let Err(e) =
                        api::search::chat::setup_chat(&client, &meili_url, &meili_master_key, &rt)
                            .await
                    {
                        error!("Failed to configure chat workspace on startup: {e}");
                    } else {
                        info!("Chat workspace configured");
                    }
                }
            }
            let backend: Arc<dyn SearchBackend> = Arc::new(meili_state);
            let reindex_check =
                api::search::check_reindex_needed(&pool_clone, backend.as_ref()).await;
            if let Some(reason) = &reindex_check.reason {
                info!("Search reindex needed: {}", reason);
            }
            let needs = reindex_check.needs_reindex;
            (backend, needs)
        },
        async {
            let (accel, caps, decoder_caps) =
                api::ffmpeg::detect::resolve_hw_accel(&hw_preference).await;
            info!("Hardware acceleration: {:?}", accel);
            (accel, caps, decoder_caps)
        }
    );

    let config = Arc::new(config);
    let cancel_registry = nats_jobs::CancelRegistry::new();
    let state = create_app_state(
        pool.clone(),
        (*config).clone(),
        runtime_config.clone(),
        nats_state,
        search_backend.clone(),
        hw_accel,
        encoder_caps,
        decoder_caps,
        cancel_registry.clone(),
    );

    // Release the GPU device context (CUDA primary context ~100MB VRAM) once
    // transcoding has been idle past the grace window.
    {
        let hw_device = state.hw_device.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(10));
            loop {
                tick.tick().await;
                hw_device.reap_if_idle(std::time::Duration::from_secs(30));
            }
        });
    }

    // Start the proxy server if configured
    {
        let proxy_config = ProxyConfig {
            listen_addr: ([0, 0, 0, 0], config.proxy_port).into(),
        };
        let handler = Arc::new(KobblerHandler::new(
            pool.clone(),
            config.clone(),
            runtime_config.clone(),
            state.nats.publisher().clone(),
        ));
        let logger: Arc<dyn kobo_proxy::RequestLogger> = Arc::new(DatabaseLogger::new(
            pool.clone(),
            BlobStorage::new(&config.storage_root),
            runtime_config.clone(),
            state.library_events.clone(),
        ));

        tokio::spawn(async move {
            info!(
                "Starting proxy server on port {}",
                proxy_config.listen_addr.port()
            );
            if let Err(e) = run_proxy(proxy_config, handler, Some(logger)).await {
                error!("Proxy server failed: {e}");
            }
        });
    }

    // Start NATS job worker (handles proxy_log_cleanup, author_sync_check, and other jobs)
    {
        let nats = nats_client.clone();
        let pool_clone = pool.clone();
        let storage_root = config.storage_root.clone();
        let library_events = state.library_events.clone();
        let job_events = state.job_events.clone();
        let ingest_queue_events = state.ingest_queue_events.clone();
        let runtime_config_clone = runtime_config.clone();
        let search_clone = search_backend.clone();
        let app_state_clone = state.clone();

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        tokio::spawn(async move {
            nats::start_nats_worker(
                nats,
                pool_clone,
                storage_root,
                library_events,
                job_events,
                ingest_queue_events,
                runtime_config_clone,
                search_clone,
                app_state_clone,
                shutdown_rx,
                cancel_registry,
            )
            .await;
        });

        std::mem::forget(shutdown_tx);
        info!("NATS job worker started");
    }

    // Start NATS scheduler for periodic jobs
    {
        let nats = nats_client.clone();
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        tokio::spawn(async move {
            nats::start_scheduler(nats, shutdown_rx).await;
        });

        std::mem::forget(shutdown_tx);
        info!("NATS scheduler started");
    }

    // Live TV boot reconcile. Nothing in the lease registry survives a restart,
    // and the device keeps streaming to a dead client with nothing timing out on
    // its own, so orphaned leases and buffer directories are cleaned up here.
    {
        let state = state.clone();
        tokio::spawn(async move {
            let runtime = state.runtime_config.get().await;
            let root = api::livetv::live_session::buffer_root(&state.config, &runtime);
            if let Err(e) = tokio::fs::create_dir_all(&root).await {
                error!("Live TV buffer root {} is unusable: {e}", root.display());
                return;
            }
            if let Err(e) = api::livetv::tuner_lease::reconcile_on_boot(&state.pool, &root).await {
                error!("Live TV boot reconcile failed: {e}");
            }
            // Decision 13: a recording whose window is still open resumes by
            // appending a part; one whose window has closed is finished off.
            let publisher = state.nats.publisher();
            if let Err(e) = api::livetv::recorder::reconcile_on_boot(&state.pool, publisher).await {
                error!("Live TV recording reconcile failed: {e}");
            }
            api::livetv::live_session::spawn_sweeper(state);
        });
    }

    // One-shot, idempotent merge of duplicate sport-event rows (two TheSportsDB
    // event IDs for one race). Safe to run every boot — a no-op once canonicalized.
    {
        use api::jobs::JobContext;
        let publisher = state.nats.publisher();
        let context = JobContext::new("Sport-event dedup on startup");
        if let Err(e) = api::jobs::enqueue(
            &pool,
            publisher,
            "sport_event_dedupe",
            serde_json::json!({}),
            context,
        )
        .await
        {
            error!("Failed to enqueue sport_event_dedupe: {}", e);
        } else {
            info!("sport_event_dedupe job enqueued");
        }
    }

    // Trigger automatic reindex if needed (after workers are running)
    if needs_reindex {
        use api::jobs::JobContext;
        let publisher = state.nats.publisher();
        let context = JobContext::new("Automatic search reindex on startup");
        if let Err(e) = api::jobs::enqueue(
            &pool,
            publisher,
            "search_reindex",
            serde_json::json!({}),
            context,
        )
        .await
        {
            error!("Failed to enqueue automatic reindex: {}", e);
        } else {
            info!("Automatic search reindex job enqueued");
        }
    }

    // Start NATS download event subscription
    {
        let nats = nats_client.clone();
        let pool_clone = pool.clone();
        let library_events = state.library_events.clone();

        tokio::spawn(async move {
            nats::subscribe_download_events(nats, pool_clone, library_events).await;
        });

        info!("NATS download event subscription started");
    }

    // Start NATS want response event subscription
    {
        let nats = nats_client.clone();
        let pool_clone = pool.clone();
        let library_events = state.library_events.clone();

        tokio::spawn(async move {
            nats::subscribe_want_response_events(nats, pool_clone, library_events).await;
        });

        info!("NATS want response event subscription started");
    }

    // Start NATS watch pattern deleted event subscription
    {
        let nats = nats_client.clone();
        let pool_clone = pool.clone();
        let library_events = state.library_events.clone();

        tokio::spawn(async move {
            nats::subscribe_watch_pattern_deleted_events(nats, pool_clone, library_events).await;
        });

        info!("NATS watch pattern deleted event subscription started");
    }

    // Start NATS ingest-ready subscription (receives Kobload completion, enqueues ingest jobs)
    {
        let nats = nats_client.clone();
        let pool_clone = pool.clone();
        let nats_publisher = nats_jobs::Publisher::new(nats.clone(), "kobbler");

        tokio::spawn(async move {
            nats::subscribe_ingest_ready_events(nats, pool_clone, nats_publisher).await;
        });

        info!("NATS ingest-ready event subscription started");
    }

    // Start Kobload SSE bridge (subscribes to kobload and broadcasts to SSE clients)
    {
        let runtime_config = state.runtime_config.clone();
        let ws_broadcast = state.library_events.clone();

        tokio::spawn(async move {
            api::kobload_bridge::run_kobload_bridge(runtime_config, ws_broadcast).await;
        });

        info!("Kobload SSE bridge started");
    }

    // Start the chat watch subscriber (progress and completion messages into chat threads)
    {
        let rx = state.library_events.subscribe();
        let dispatcher = api::chatbot::watches::WatchDispatcher::new(pool.clone());

        tokio::spawn(async move {
            api::chatbot::watches::run_subscriber(dispatcher, rx).await;
        });

        info!("Chat watch subscriber started");
    }

    // Start the Discord gateway listener. It supervises itself: with no bot token
    // configured it waits and looks again, so setting Discord up in the settings page
    // does not need a restart.
    {
        let gateway_state = state.clone();

        tokio::spawn(async move {
            api::chatbot::transport::discord_gateway::run(gateway_state).await;
        });

        info!("Discord gateway listener started");
    }

    // Start search indexing subscription (listens to library events and enqueues index jobs)
    {
        let rx = state.library_events.subscribe();
        let nats_publisher = state.nats.publisher().clone();
        let pool_clone = pool.clone();

        tokio::spawn(async move {
            use api::LibraryEvent;

            api::events::run_subscriber("search indexing", rx, async move |event| {
                match event {
                    LibraryEvent::LibraryUpdated { book_id: Some(id) } => {
                        api::jobs::enqueue_book_index(&pool_clone, &nats_publisher, id).await;
                    }
                    LibraryEvent::AuthorUpdated { author_id } => {
                        api::jobs::enqueue_author_index(&pool_clone, &nats_publisher, author_id)
                            .await;
                    }
                    LibraryEvent::SeriesUpdated { series_id } => {
                        api::jobs::enqueue_series_index(&pool_clone, &nats_publisher, series_id)
                            .await;
                    }
                    LibraryEvent::MovieUpdated { movie_id } => {
                        api::jobs::enqueue_movie_index(&pool_clone, &nats_publisher, movie_id)
                            .await;
                    }
                    LibraryEvent::TvShowUpdated { show_id } => {
                        api::jobs::enqueue_tv_show_index(&pool_clone, &nats_publisher, show_id)
                            .await;

                        // Cascade: reindex all seasons and episodes for this show
                        // (show title may have changed, which affects their display_title).
                        // One batched job, not one job per child: a long-running show
                        // otherwise bursts hundreds of single-document writes at Meilisearch.
                        api::jobs::enqueue_tv_show_children_index(
                            &pool_clone,
                            &nats_publisher,
                            show_id,
                        )
                        .await;
                    }
                    LibraryEvent::TvSeasonUpdated { season_id } => {
                        api::jobs::enqueue_tv_season_index(&pool_clone, &nats_publisher, season_id)
                            .await;
                    }
                    LibraryEvent::TvEpisodeUpdated { episode_id } => {
                        api::jobs::enqueue_tv_episode_index(
                            &pool_clone,
                            &nats_publisher,
                            episode_id,
                        )
                        .await;
                    }
                    _ => {} // Ignore other events
                }
            })
            .await;
        });

        info!("Search indexing subscription started");
    }

    // Start notification dispatcher (listens to library events and enqueues notification_send jobs)
    {
        let rx = state.library_events.subscribe();
        let library_events_tx = state.library_events.clone();
        let nats_publisher = state.nats.publisher().clone();
        let pool_clone = pool.clone();
        let runtime_config_clone = state.runtime_config.clone();

        tokio::spawn(async move {
            use api::notifications::{resolve_entity, translate_event};

            api::events::run_subscriber("notification dispatcher", rx, async move |event| {
                // Skip our own NotificationCreated events to avoid infinite loop
                if matches!(event, api::events::LibraryEvent::NotificationCreated) {
                    return;
                }

                if let Some(translated) = translate_event(&event) {
                    // Look up entity details for rich notification content
                    let resolved = resolve_entity(&pool_clone, translated.entity_id).await;

                    let (entity_name, entity_type, entity_id, url) = match &resolved {
                        Some(entity) => (
                            entity.name.clone(),
                            Some(entity.entity_type.to_string()),
                            Some(translated.entity_id),
                            Some(entity.url.clone()),
                        ),
                        None => (
                            "Unknown item".to_string(),
                            None,
                            Some(translated.entity_id),
                            None,
                        ),
                    };

                    // Record user-facing activity for grab/fail events
                    // (OnImport is skipped — the ingest handler records "imported" directly)
                    let activity_kind = match translated.trigger {
                        api::notifications::NotificationTrigger::OnGrab => Some("grabbed"),
                        api::notifications::NotificationTrigger::OnFailed => Some("failed"),
                        _ => None,
                    };
                    if let Some(kind) = activity_kind {
                        let image_url = resolved.as_ref().and_then(|r| match r.entity_type {
                            "book" => Some(format!(
                                "/api/v1/library/books/{}/cover",
                                translated.entity_id
                            )),
                            "movie" => Some(format!(
                                "/api/v1/admin/movies/{}/poster",
                                translated.entity_id
                            )),
                            "show" => Some(format!(
                                "/api/v1/admin/tv/shows/{}/poster",
                                translated.entity_id
                            )),
                            _ => None,
                        });
                        api::routes::library::helpers::record_activity(
                            &pool_clone,
                            kind,
                            entity_type.as_deref(),
                            entity_id,
                            &entity_name,
                            None,
                            image_url.as_deref(),
                            url.as_deref(),
                            api::routes::library::helpers::requester_of(
                                &pool_clone,
                                translated.entity_id,
                            )
                            .await,
                        )
                        .await;
                        let _ =
                            library_events_tx.send(api::events::LibraryEvent::ActivityLogUpdated);
                    }

                    let message = translated.message_template.replace("{}", &entity_name);

                    let payload = api::notifications::NotificationPayload {
                        trigger: translated.trigger,
                        title: translated.title.clone(),
                        message: message.clone(),
                        entity_type: entity_type.clone(),
                        entity_id,
                        entity_name: Some(entity_name),
                        url: url.clone(),
                        public_url: runtime_config_clone.get().await.public_url.clone(),
                        show_id: resolved.as_ref().and_then(|r| r.show_id),
                        show_name: resolved.as_ref().and_then(|r| r.show_name.clone()),
                        season_number: resolved.as_ref().and_then(|r| r.season_number),
                        show_year: resolved.as_ref().and_then(|r| r.show_year),
                        upgraded: translated.upgraded,
                        details: resolved.as_ref().and_then(|r| r.details.clone()),
                    };

                    if let Err(e) = api::notifications::in_app::notify_household(
                        &pool_clone,
                        &api::notifications::in_app::InAppNotification {
                            title: translated.title.clone(),
                            message: message.clone(),
                            entity_type: entity_type.clone(),
                            entity_id,
                            entity_url: url.clone(),
                        },
                    )
                    .await
                    {
                        tracing::warn!(error = %e, "Failed to create in-app notifications");
                    }
                    let _ = library_events_tx.send(api::events::LibraryEvent::NotificationCreated);

                    // Query enabled agents that have this trigger
                    let agents =
                        match api::repositories::notification_agents::list_enabled(&pool_clone)
                            .await
                        {
                            Ok(agents) => agents,
                            Err(e) => {
                                tracing::warn!(error = %e, "Failed to query notification agents");
                                return;
                            }
                        };

                    let trigger_str = serde_json::to_value(translated.trigger).unwrap_or_default();

                    for agent in agents {
                        // Check if agent has this trigger in its triggers array
                        let triggers = agent.triggers.as_array().unwrap_or(&Vec::new()).clone();
                        if !triggers.contains(&trigger_str) {
                            continue;
                        }

                        let send_payload = serde_json::json!({
                            "agent_id": agent.id,
                            "payload": payload,
                        });
                        let context = api::jobs::JobContext::new(format!(
                            "Sending {} notification to {}",
                            trigger_str.as_str().unwrap_or("unknown"),
                            agent.name
                        ));
                        if let Err(e) = api::jobs::enqueue_routine(
                            &pool_clone,
                            &nats_publisher,
                            "notification_send",
                            send_payload,
                            context,
                        )
                        .await
                        {
                            tracing::warn!(error = %e, agent_id = %agent.id, "Failed to enqueue notification_send job");
                        }
                    }
                }
            })
            .await;
        });

        info!("Notification dispatcher started");
    }

    // Start watcher service for directory monitoring (in background, doesn't block server)
    {
        let mut watcher_service = WatcherService::new(
            pool.clone(),
            WatcherConfig::default(),
            config.storage_root.clone(),
            state.library_events.clone(),
            state.nats.publisher().clone(),
        );
        tokio::spawn(async move {
            if let Err(e) = watcher_service.start().await {
                error!("Failed to start watcher service: {}", e);
            } else {
                info!("Watcher service started");
                // Keep watcher service alive
                std::mem::forget(watcher_service);
            }
        });
    }

    // Note: Tracked author sync is now handled via NATS scheduler (author_sync_check job)

    // Playback session reaper — cleans up idle sessions every 30s
    {
        let sessions = state.playback_sessions.clone();
        let live_streams = state.live_streams.clone();
        let debrid_index = state.debrid_session_index.clone();
        let debrid_locks = state.debrid_resolve_locks.clone();
        let reaper_pool = pool.clone();
        let reaper_nats = state.nats.publisher().clone();
        let reaper_state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            let max_idle_vod = std::time::Duration::from_secs(14_400); // 4 hours for VOD
            let max_idle_live = std::time::Duration::from_secs(30); // 30s for IPTV live
            let max_download_idle = std::time::Duration::from_secs(300); // 5 min for debrid downloads
            loop {
                interval.tick().await;

                // Clean up stale provider connection counters
                api::iptv::provider_tracker::cleanup_stale();

                api::routes::admin::stream_prewarm::sweep(&reaper_state).await;

                // Stop downloads for idle debrid sessions (session stays alive for resume)
                for entry in sessions.iter() {
                    let session = entry.value();
                    if let Some(ref cancel) = session.download_cancel
                        && !cancel.is_cancelled()
                        && session.last_activity.elapsed() > max_download_idle
                    {
                        tracing::info!(
                            session_id = %entry.key(),
                            idle_secs = session.last_activity.elapsed().as_secs(),
                            "Stopping debrid download for idle session"
                        );
                        cancel.cancel();
                    }
                }

                let expired: Vec<uuid::Uuid> = sessions
                    .iter()
                    .filter(|entry| {
                        let session = entry.value();
                        let max_idle = if session.is_live {
                            max_idle_live
                        } else {
                            max_idle_vod
                        };
                        session.is_expired(max_idle)
                    })
                    .map(|entry| *entry.key())
                    .collect();
                for id in expired {
                    if let Some((_, mut session)) = sessions.remove(&id) {
                        // Cancel debrid download if still running
                        if let Some(ref cancel) = session.download_cancel {
                            cancel.cancel();
                        }
                        // Remove from debrid session dedup index
                        if let Some(ref key) = session.debrid_cache_key {
                            debrid_index.remove(key);
                        }
                        // For IPTV live sessions: release provider connection
                        if let Some(provider_id) = session.provider_id {
                            api::iptv::provider_tracker::release(provider_id);
                            live_streams.remove(&session.file_id);
                            tracing::info!(
                                session_id = %id,
                                provider_id = %provider_id,
                                "released IPTV provider connection (session expired)"
                            );
                        }
                        let dir = session.segment_dir().to_path_buf();
                        session.kill_all_transcodes().await;
                        // Enqueue background cleanup job
                        let payload = api::jobs::handlers::SessionCleanupPayload {
                            dir: dir.to_string_lossy().to_string(),
                        };
                        let context =
                            api::jobs::JobContext::new("Cleaning up expired playback session");
                        if let Err(e) = api::jobs::enqueue_routine(
                            &reaper_pool,
                            &reaper_nats,
                            "session_cleanup",
                            serde_json::to_value(&payload).unwrap(),
                            context,
                        )
                        .await
                        {
                            tracing::warn!(error = %e, "Failed to enqueue session cleanup, removing directly");
                            let _ = tokio::fs::remove_dir_all(&dir).await;
                        }
                        tracing::info!(session_id = %id, "Reaped expired playback session");
                    }
                }

                // Sweep orphaned debrid resolve locks
                debrid_locks.retain(|k, _| debrid_index.contains_key(k));
            }
        });
        info!("Playback session reaper started");
    }

    let trace_headers = env::var("KOBBLER_HTTP_TRACE")
        .ok()
        .map(|val| val == "1" || val.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let trace_scope = env::var("KOBBLER_HTTP_TRACE_SCOPE")
        .unwrap_or_else(|_| "all".to_string())
        .to_lowercase();
    let kobo_only = trace_scope == "kobo";

    let app = create_router(state).layer(
        TraceLayer::new_for_http()
            .make_span_with(move |request: &axum::http::Request<_>| {
                let path = request.uri().path();
                let is_kobo = path.starts_with("/kobo/");
                let include = trace_headers && (!kobo_only || is_kobo);
                if include {
                    span!(
                        Level::INFO,
                        "http.kobo",
                        method = %request.method(),
                        uri = %path,
                        headers = ?request.headers()
                    )
                } else {
                    span!(Level::TRACE, "http.other", method = %request.method(), uri = %path)
                }
            })
            .on_response(
                move |response: &axum::http::Response<_>,
                      latency: std::time::Duration,
                      span: &tracing::Span| {
                    let include = trace_headers
                        && span
                            .metadata()
                            .map(|m| m.name() == "http.kobo")
                            .unwrap_or(false);
                    if include {
                        DefaultOnResponse::new()
                            .include_headers(true)
                            .on_response(response, latency, span);
                    }
                },
            ),
    );

    let addr: SocketAddr = match config.bind_addr.parse() {
        Ok(addr) => addr,
        Err(err) => {
            error!("Invalid BIND_ADDR: {err}");
            return;
        }
    };

    info!("Listening on {}", addr);
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(err) => {
            error!("Failed to bind to {}: {err}", addr);
            return;
        }
    };

    // Watchdog: a non-tokio thread that detects async runtime hangs.
    // If the health endpoint stops responding, this thread dumps full
    // async task traces + runtime metrics, then exits so Docker restarts.
    {
        let watchdog_addr: std::net::SocketAddr = ([127, 0, 0, 1], addr.port()).into();
        let runtime_handle = tokio::runtime::Handle::current();
        let watchdog_pool = pool.clone();
        std::thread::Builder::new()
            .name("watchdog".into())
            .spawn(move || run_watchdog(watchdog_addr, runtime_handle, watchdog_pool))
            .expect("Failed to spawn watchdog thread");
        info!(
            "Watchdog thread started (checking localhost:{})",
            addr.port()
        );
    }

    if let Err(err) = axum::serve(listener, app).await {
        error!("Server failed: {err}");
    }
}

async fn bootstrap_admin(pool: &DbPool, email: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Check if any users exist (all users are admins)
    if repositories::users::any_exist(pool).await? {
        return Ok(());
    }

    // Use DEV_ADMIN_PASSWORD if set, otherwise generate a random password
    let password = env::var("DEV_ADMIN_PASSWORD")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(generate_password);

    let password_hash =
        auth::hash_password(&password).map_err(|e| format!("password hashing failed: {e}"))?;

    repositories::users::create(pool, email, &password_hash, "admin").await?;

    info!(
        "Admin account created: email={} password={}",
        email, password
    );
    Ok(())
}

async fn bootstrap_dev_ingest_token(pool: &DbPool) -> Result<(), Box<dyn std::error::Error>> {
    // Only create if DEV_INGEST_TOKEN is set
    let token = match env::var("DEV_INGEST_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        Some(t) => t,
        None => return Ok(()),
    };

    // Check if token already exists
    if repositories::ingest_tokens::token_exists(pool, &token).await? {
        return Ok(());
    }

    repositories::ingest_tokens::create(pool, &token, "dev-token").await?;

    info!("Dev ingest token created: {}", token);
    Ok(())
}

fn generate_password() -> String {
    Alphanumeric.sample_string(&mut rand::rng(), 24)
}

/// Non-tokio watchdog thread. Sends a raw HTTP request to the health endpoint
/// every 30 seconds. If 3 consecutive checks fail, dumps full async task traces
/// and runtime metrics, then kills the process so Docker can restart it.
///
/// Runs on a plain OS thread (not tokio) so it works even when the async
/// runtime is completely hung. Uses `Handle::dump()` via a throwaway
/// single-threaded runtime to capture async stack traces without needing
/// the main runtime to be responsive.
fn run_watchdog(
    addr: std::net::SocketAddr,
    runtime_handle: tokio::runtime::Handle,
    db_pool: api::DbPool,
) {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    const CHECK_INTERVAL: Duration = Duration::from_secs(30);
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
    const READ_TIMEOUT: Duration = Duration::from_secs(5);
    const MAX_FAILURES: u32 = 3;
    const STARTUP_GRACE: Duration = Duration::from_secs(60);
    const DUMP_TIMEOUT: Duration = Duration::from_secs(10);

    std::thread::sleep(STARTUP_GRACE);

    let mut consecutive_failures: u32 = 0;

    loop {
        std::thread::sleep(CHECK_INTERVAL);

        let ok = (|| -> bool {
            let mut stream = match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(s) => s,
                Err(_) => return false,
            };
            if stream.set_read_timeout(Some(READ_TIMEOUT)).is_err() {
                return false;
            }
            if stream
                .write_all(b"GET /health HTTP/1.0\r\nHost: localhost\r\n\r\n")
                .is_err()
            {
                return false;
            }
            let mut buf = [0u8; 256];
            match stream.read(&mut buf) {
                Ok(n) if n > 0 => String::from_utf8_lossy(&buf[..n]).contains("200"),
                _ => false,
            }
        })();

        if ok {
            if consecutive_failures > 0 {
                eprintln!(
                    "[watchdog] Health check recovered after {} consecutive failure(s)",
                    consecutive_failures
                );
            }
            consecutive_failures = 0;
        } else {
            consecutive_failures += 1;
            eprintln!(
                "[watchdog] Health check FAILED ({}/{})",
                consecutive_failures, MAX_FAILURES
            );

            // Dump runtime metrics on the FIRST failure, not just at kill time.
            // A worker stuck in a long synchronous call looks identical to an
            // idle one from outside the process: load is low, queues are empty,
            // the DB pool is healthy. These counters are the only cheap signal
            // that says otherwise, and a stall that recovers before the third
            // check never reached the kill-time dump -- so the failure mode
            // stayed invisible for as long as it kept recovering. Metrics only:
            // the async task dump is expensive and stays at kill time.
            if consecutive_failures == 1 {
                eprintln!(
                    "[watchdog] First failure — dumping runtime metrics. A single busy \
                     worker with everything else parked means blocking work on the runtime."
                );
                dump_runtime_metrics(&runtime_handle);
            }

            if consecutive_failures >= MAX_FAILURES {
                // ── Dump diagnostics before killing ──────────────────
                dump_runtime_metrics(&runtime_handle);
                dump_db_pool_stats(&db_pool);
                dump_async_task_traces(&runtime_handle, DUMP_TIMEOUT);

                let total_secs = MAX_FAILURES * CHECK_INTERVAL.as_secs() as u32
                    + CONNECT_TIMEOUT.as_secs() as u32 * MAX_FAILURES;

                eprintln!(
                    r#"
╔══════════════════════════════════════════════════════════════════════════╗
║                                                                          ║
║   ██╗    ██╗ █████╗ ████████╗ ██████╗██╗  ██╗██████╗  ██████╗  ██████╗  ║
║   ██║    ██║██╔══██╗╚══██╔══╝██╔════╝██║  ██║██╔══██╗██╔═══██╗██╔════╝  ║
║   ██║ █╗ ██║███████║   ██║   ██║     ███████║██║  ██║██║   ██║██║  ███╗  ║
║   ██║███╗██║██╔══██║   ██║   ██║     ██╔══██║██║  ██║██║   ██║██║   ██║  ║
║   ╚███╔███╔╝██║  ██║   ██║   ╚██████╗██║  ██║██████╔╝╚██████╔╝╚██████╔╝ ║
║    ╚══╝╚══╝ ╚═╝  ╚═╝   ╚═╝    ╚═════╝╚═╝  ╚═╝╚═════╝  ╚═════╝  ╚═════╝ ║
║                                                                          ║
║   KOBBLER API IS UNRESPONSIVE — KILLING PROCESS                          ║
║                                                                          ║
║   /health failed {} times in a row (~{}s with no response).               ║
║   Runtime metrics and async task traces have been dumped above.           ║
║                                                                          ║
║   Exiting with code 1 so Docker/systemd can restart the container.       ║
║                                                                          ║
╚══════════════════════════════════════════════════════════════════════════╝
"#,
                    MAX_FAILURES, total_secs,
                );
                std::process::exit(1);
            }
        }
    }
}

fn dump_runtime_metrics(handle: &tokio::runtime::Handle) {
    let m = handle.metrics();
    eprintln!("\n[watchdog] ═══ TOKIO RUNTIME STATE ═══");
    eprintln!("[watchdog]   Workers:           {}", m.num_workers());
    eprintln!(
        "[watchdog]   Blocking threads:  {}",
        m.num_blocking_threads()
    );
    eprintln!("[watchdog]   Active tasks:      {}", m.num_alive_tasks());
    eprintln!("[watchdog]   Global queue:      {}", m.global_queue_depth());
    eprintln!(
        "[watchdog]   Blocking queue:    {}",
        m.blocking_queue_depth()
    );
    for i in 0..m.num_workers() {
        eprintln!(
            "[watchdog]   Worker {}: local_queue={}, steal_count={}, poll_count={}, park_count={}",
            i,
            m.worker_local_queue_depth(i),
            m.worker_steal_count(i),
            m.worker_poll_count(i),
            m.worker_park_count(i),
        );
    }
}

fn dump_db_pool_stats(pool: &api::DbPool) {
    eprintln!("\n[watchdog] ═══ DATABASE CONNECTION POOL ═══");
    eprintln!(
        "[watchdog]   Connections: {} total, {} idle (max: {})",
        pool.size(),
        pool.num_idle(),
        pool.options().get_max_connections(),
    );
    if pool.num_idle() == 0 && pool.size() >= pool.options().get_max_connections() {
        eprintln!(
            "[watchdog]   ⚠ POOL EXHAUSTED — all {} connections in use, new queries are blocking",
            pool.size()
        );
    }
}

fn dump_async_task_traces(handle: &tokio::runtime::Handle, timeout: std::time::Duration) {
    eprintln!("\n[watchdog] ═══ ASYNC TASK DUMP ═══");
    eprintln!(
        "[watchdog]   Capturing task traces ({}s timeout)...",
        timeout.as_secs()
    );

    // Use a throwaway single-threaded runtime to drive Handle::dump().
    // This works even if the main runtime is completely deadlocked because
    // dump() signals worker threads via OS-level mechanisms.
    let dump_rt = match tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("[watchdog]   ✗ Failed to create dump runtime: {e}");
            return;
        }
    };

    let result = dump_rt.block_on(async { tokio::time::timeout(timeout, handle.dump()).await });

    match result {
        Ok(dump) => {
            let tasks: Vec<_> = dump.tasks().iter().collect();
            eprintln!("[watchdog]   Captured {} task(s):\n", tasks.len());
            for (i, task) in tasks.iter().enumerate() {
                let trace = format!("{}", task.trace());
                if trace.trim().is_empty() {
                    eprintln!("[watchdog]   ─── Task {}: (no trace available) ───", i);
                } else {
                    eprintln!("[watchdog]   ─── Task {} ───", i);
                    for line in trace.lines() {
                        eprintln!("[watchdog]     {line}");
                    }
                }
                eprintln!();
            }
        }
        Err(_) => {
            eprintln!("[watchdog]   ✗ Task dump TIMED OUT — runtime is too stuck to respond");
            eprintln!("[watchdog]     This means workers cannot even pause to report their state.");
            eprintln!(
                "[watchdog]     Likely cause: blocking syscall on a worker thread (not spawn_blocking)."
            );
        }
    }
}
