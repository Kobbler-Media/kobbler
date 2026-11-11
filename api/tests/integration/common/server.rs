use api::{
    AppState, IngestQueueEvent, LibraryEvent, RuntimeConfigManager,
    blob_storage::BlobStorage,
    config::{Config, KoboMode},
    create_app_state, create_router,
    db::{DbPool, connect, migrate},
    file_config::FileConfig,
    jobs::JobEvent,
    jobs::handlers::KobloadIngestHandler,
    jobs::worker::JobHandler,
    kobo_handler::{DatabaseLogger, KobblerHandler},
    nats::NatsState,
    repositories::{auth_tokens, ingest_tokens},
    search::{SearchBackend, SqlFallbackBackend},
};
use kobo_proxy::serve_proxy;
use nats_jobs::{NatsClient, NatsConfig, Publisher};
use rand::distr::{Alphanumeric, SampleString};
use serde_json::{Value, json};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

use super::client::TestClient;

/// One-shot connection for CREATE/DROP DATABASE DDL.
/// These take Postgres-level locks on the template and must not block pool connections.
pub async fn admin_connect() -> sqlx::PgConnection {
    use sqlx::Connection;
    let base_url = std::env::var("DATABASE_URL_TEST")
        .unwrap_or_else(|_| "postgres://kobbler:kobbler@localhost:5433/kobbler_test".to_string());
    let postgres_url = base_url
        .rsplit_once('/')
        .map(|(base, _)| format!("{}/postgres", base))
        .unwrap_or(base_url);
    sqlx::PgConnection::connect(&postgres_url)
        .await
        .expect("Failed to connect to postgres admin database")
}

/// Generate a random token (same logic as the auth module).
fn generate_token(length: usize) -> String {
    Alphanumeric.sample_string(&mut rand::rng(), length)
}

/// Get a unique run ID for this test run.
/// Uses TEST_RUN_ID env var (set by Makefile) to ensure all test binaries
/// in the same `cargo test` invocation share the same template database.
/// Falls back to timestamp + PID if env var is not set (e.g., running single test).
/// Format is always TIMESTAMP_PID to match Makefile and enable consistent parsing.
fn get_run_id() -> &'static str {
    static RUN_ID: OnceLock<String> = OnceLock::new();
    RUN_ID.get_or_init(|| {
        // A run id that is not TIMESTAMP_PID makes a database name
        // `extract_timestamp_from_db_name` cannot read, and an uncollectable
        // database lives forever. Reject the value rather than trusting it.
        std::env::var("TEST_RUN_ID")
            .ok()
            .filter(|id| {
                let mut parts = id.split('_');
                matches!(
                    (parts.next(), parts.next(), parts.next()),
                    (Some(ts), Some(pid), None)
                        if !ts.is_empty()
                            && ts.bytes().all(|b| b.is_ascii_digit())
                            && !pid.is_empty()
                            && pid.bytes().all(|b| b.is_ascii_digit())
                )
            })
            .unwrap_or_else(|| {
                // Fallback for running tests outside of `make`
                // Format: TIMESTAMP_PID (matches Makefile format)
                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                let pid = std::process::id();
                format!("{}_{}", timestamp, pid)
            })
    })
}

/// Get the template database name for this run.
fn template_db_name() -> String {
    format!("kobbler_tpl_{}", get_run_id())
}

/// Get the prefix for test databases in this run.
fn test_db_prefix() -> String {
    format!("test_{}_", get_run_id())
}

/// Async-safe initialization of template database
static TEMPLATE_INIT: OnceCell<()> = OnceCell::const_new();

async fn init_test_env() {
    // Install rustls crypto provider (only needs to happen once, use std::sync::Once)
    static CRYPTO_INIT: std::sync::Once = std::sync::Once::new();
    CRYPTO_INIT.call_once(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("Failed to install ring crypto provider");

        // Initialize logging
        tracing_subscriber::fmt()
            .with_env_filter("api=debug,sqlx=warn,test=debug")
            .with_test_writer()
            .try_init()
            .ok();
    });

    // Initialize template database (async, runs once)
    TEMPLATE_INIT
        .get_or_init(|| async {
            init_template_database().await;
        })
        .await;
}

/// Create or update the template database with migrations
async fn init_template_database() {
    let base_url = std::env::var("DATABASE_URL_TEST")
        .unwrap_or_else(|_| "postgres://kobbler:kobbler@localhost:5433/kobbler_test".to_string());

    let mut ddl_conn = admin_connect().await;
    cleanup_stale_databases(&mut ddl_conn).await;

    let template_name = template_db_name();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS {} WITH (FORCE)",
        template_name
    )))
    .execute(&mut ddl_conn)
    .await
    .ok();

    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE DATABASE {}",
        template_name
    )))
    .execute(&mut ddl_conn)
    .await
    .expect("Failed to create template database");
    drop(ddl_conn);

    // Connect to template and run migrations
    let template_url = base_url
        .rsplit_once('/')
        .map(|(base, _)| format!("{}/{}", base, template_name))
        .unwrap_or_else(|| base_url.clone());

    let template_pool = connect(&template_url, 2)
        .await
        .expect("Failed to connect to template database");

    migrate(&template_pool)
        .await
        .expect("Failed to run migrations on template");

    template_pool.close().await;
}

/// Clean up stale test databases and templates older than 5 minutes.
/// Only cleans up databases from OTHER test runs (not the current one).
/// Also checks for active connections to avoid killing concurrent test runs.
pub async fn cleanup_stale_databases(conn: &mut sqlx::PgConnection) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let cutoff = now.saturating_sub(5 * 60); // 5 minutes ago

    // Find all test databases and templates
    let databases: Vec<String> = sqlx::query_scalar(
        "SELECT datname FROM pg_database WHERE datname LIKE 'test_%' OR datname LIKE 'kobbler_tpl_%'",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap_or_default();

    let current_run_id = get_run_id();

    for db in databases {
        // Skip databases from current run
        if db.contains(current_run_id) {
            continue;
        }

        // Extract timestamp from database name (format: test_TIMESTAMP_PID_UUID or kobbler_tpl_TIMESTAMP_PID)
        let timestamp = extract_timestamp_from_db_name(&db);
        let Some(ts) = timestamp else { continue };

        // A run's template sits idle while its tests use their clones, so age
        // and connections alone cannot tell a finished run from a long one.
        if ts >= cutoff || run_is_alive(&db) {
            continue;
        }

        // Safety check: don't drop if there are active connections (concurrent run might be using it)
        let active_connections: i64 =
            sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname = $1")
                .bind(&db)
                .fetch_one(&mut *conn)
                .await
                .unwrap_or(1); // Assume in-use if query fails

        if active_connections > 0 {
            continue;
        }

        // Safe to drop
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {} WITH (FORCE)",
            db
        )))
        .execute(&mut *conn)
        .await
        .ok();
    }
}

/// Linux only: elsewhere every run looks finished, as before.
fn run_is_alive(name: &str) -> bool {
    let pid = if name.starts_with("kobbler_tpl_") {
        name.split('_').nth(3)
    } else {
        name.split('_').nth(2)
    };
    pid.is_some_and(|pid| std::path::Path::new(&format!("/proc/{pid}")).exists())
}

/// Extract timestamp from database name.
/// Run ID format from Makefile is TIMESTAMP_PID (e.g., "1768448729_12345")
/// Handles formats:
///   - test_TIMESTAMP_PID_UUID -> parts[1] is timestamp
///   - kobbler_tpl_TIMESTAMP_PID -> parts[2] is timestamp
fn extract_timestamp_from_db_name(name: &str) -> Option<u64> {
    let parts: Vec<&str> = name.split('_').collect();
    if name.starts_with("test_") && parts.len() >= 4 {
        // test_TIMESTAMP_PID_UUID: ["test", "TIMESTAMP", "PID", "UUID"]
        parts[1].parse().ok()
    } else if name.starts_with("kobbler_tpl_") && parts.len() >= 4 {
        // kobbler_tpl_TIMESTAMP_PID: ["kobbler", "tpl", "TIMESTAMP", "PID"]
        parts[2].parse().ok()
    } else {
        None
    }
}

/// A test server instance with pre-configured clients.
pub struct TestServer {
    /// Base URL for the API (admin, library, ingest routes)
    pub api_url: String,
    /// Base URL for the Kobo proxy (device sync routes)
    pub proxy_url: String,
    pub admin_token: String,
    pub ingest_token: String,
    pub pool: DbPool,
    /// NATS publisher for enqueuing jobs in tests
    pub nats_publisher: Publisher,
    /// Runtime config manager for testing job handlers
    pub runtime_config: RuntimeConfigManager,
    /// Search backend for testing indexing queries
    search_backend: Arc<dyn SearchBackend>,
    /// Broadcast channel for library events (for SSE tests)
    pub library_events: tokio::sync::broadcast::Sender<LibraryEvent>,
    /// Broadcast channel for job events (for SSE tests)
    pub job_events: tokio::sync::broadcast::Sender<JobEvent>,
    /// Broadcast channel for ingest queue events (for SSE tests)
    pub ingest_queue_events: tokio::sync::broadcast::Sender<IngestQueueEvent>,
    /// App state for creating job handlers in tests
    pub app_state: AppState,
    _storage_dir: TempDir,
    _config_dir: TempDir,
    _api_shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    db_name: String,
    // Note: proxy doesn't have graceful shutdown, it will stop when the test ends
}

impl TestServer {
    /// Start a test server with fresh database and Kobo proxy.
    /// Each call creates a new server on random ports.
    pub async fn start() -> Self {
        Self::start_internal(KoboMode::Local, None, true, |_| {}, None).await
    }

    /// Start a test server without the Kobo proxy (faster for API-only tests).
    /// Use this for tests that only need admin, library, or ingest APIs.
    pub async fn start_api_only() -> Self {
        Self::start_internal(KoboMode::Local, None, false, |_| {}, None).await
    }

    /// API-only server whose `state.search` is the given backend instead of the
    /// SQL fallback — e.g. a `MeiliState` pointed at a wiremock.
    pub async fn start_api_only_with_search(search: Arc<dyn SearchBackend>) -> Self {
        Self::start_internal(KoboMode::Local, None, false, |_| {}, Some(search)).await
    }

    /// API-only server with the static [`Config`] adjusted before startup.
    pub async fn start_api_only_with(configure: impl FnOnce(&mut Config)) -> Self {
        Self::start_internal(KoboMode::Local, None, false, configure, None).await
    }

    /// Start a test server with a specific Kobo mode and optional mock URL.
    pub async fn start_with_mode(mode: KoboMode, kobo_mock_url: Option<String>) -> Self {
        Self::start_internal(mode, kobo_mock_url, true, |_| {}, None).await
    }

    /// Internal method to start a test server with configurable options.
    async fn start_internal(
        mode: KoboMode,
        kobo_mock_url: Option<String>,
        include_proxy: bool,
        configure: impl FnOnce(&mut Config),
        search: Option<Arc<dyn SearchBackend>>,
    ) -> Self {
        init_test_env().await;

        let base_url = std::env::var("DATABASE_URL_TEST").unwrap_or_else(|_| {
            "postgres://kobbler:kobbler@localhost:5433/kobbler_test".to_string()
        });

        // Generate unique database name for this test (includes run ID for isolation)
        let db_name = format!("{}{}", test_db_prefix(), uuid::Uuid::new_v4().simple());

        // One-shot connection for CREATE DATABASE (releases immediately after DDL)
        let mut admin_conn = admin_connect().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE {} TEMPLATE {}",
            db_name,
            template_db_name()
        )))
        .execute(&mut admin_conn)
        .await
        .expect("Failed to create test database from template");
        drop(admin_conn);

        // Connect to the new test database
        let database_url = base_url
            .rsplit_once('/')
            .map(|(base, _)| format!("{}/{}", base, db_name))
            .unwrap_or_else(|| base_url.clone());

        let pool = connect(&database_url, 2)
            .await
            .expect("Failed to connect to test database");

        // Create temp storage and config directories
        let storage_dir = TempDir::new().expect("Failed to create temp storage dir");
        let config_dir = TempDir::new().expect("Failed to create temp config dir");
        let config_path = config_dir.path().join("kobbler.toml");

        // Find random available ports for API and proxy
        let api_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("Failed to bind API to random port");
        let api_addr = api_listener.local_addr().unwrap();
        let api_url = format!("http://{}", api_addr);

        let proxy_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("Failed to bind proxy to random port");
        let proxy_addr = proxy_listener.local_addr().unwrap();
        let proxy_url = format!("http://{}", proxy_addr);

        // Set up runtime config file for tests
        let kobo_mode_str = match mode {
            KoboMode::Merge => "merge",
            KoboMode::Passthrough => "passthrough",
            KoboMode::Local => "local",
        };
        let kobo_upstream = kobo_mock_url.unwrap_or_else(|| "http://localhost:9999".to_string());

        let file_config = FileConfig {
            public_proxy_url: proxy_url.clone(),
            kobo_storeapi_base_url: kobo_upstream,
            kobo_mode: kobo_mode_str.to_string(),
            proxy_logging_enabled: false,
            proxy_log_retention_days: 1,
            blob_capture_enabled: false,
            kobo_sync_batch_size: 10,
            activity_limit: 50,
            default_page_size: 20,
            max_page_size: 100,
            ..Default::default()
        };
        file_config
            .save(config_path.to_str().unwrap())
            .expect("Failed to save test config file");

        // Create static config
        let mut config = Config {
            database_url: database_url.clone(),
            bind_addr: api_addr.to_string(),
            storage_root: storage_dir.path().to_string_lossy().to_string(),
            config_path: config_path.to_string_lossy().to_string(),
            admin_email: "admin@test.local".to_string(),
            web_origins: vec![api_url.clone()],
            proxy_port: proxy_addr.port(),
            database_max_connections: 2,
            http_max_body_bytes: 1024 * 1024 * 10, // 10MB for tests
            library_events_buffer: 100,
            token_length: 32,
            meili_url: std::env::var("MEILI_URL_TEST")
                .unwrap_or_else(|_| "http://localhost:7701".to_string()),
            meili_master_key: "test-master-key".to_string(),
            nas_health_monitoring: false,
            nas_health_stats_path: "/proc/fs/cifs/Stats".to_string(),
            pin_service_url: "http://127.0.0.1:9".to_string(),
            stremio_api_url: "http://127.0.0.1:9".to_string(),
        };
        configure(&mut config);

        // Initialize runtime config manager from file
        let runtime_config = RuntimeConfigManager::new(config.config_path.clone())
            .expect("Failed to create RuntimeConfigManager");

        let config = Arc::new(config);

        // Per-test NATS connection (can't share — async_nats internal mpsc channels
        // get closed when tokio tasks from other tests wind down, causing "channel closed")
        let nats_url =
            std::env::var("NATS_URL_TEST").unwrap_or_else(|_| "nats://localhost:4223".to_string());
        let nats_config = NatsConfig::new(&nats_url, "kobbler-test");
        let nats_client = NatsClient::connect(&nats_config)
            .await
            .expect("Failed to connect to test NATS");
        let nats_publisher = Publisher::new(nats_client.clone(), "kobbler-test");
        let nats_state = NatsState::new(nats_client);

        // Tests run the same backend a Meilisearch-less install runs: search_*
        // errors, so route handlers take the ILIKE path.
        let search_backend: Arc<dyn SearchBackend> =
            search.unwrap_or_else(|| Arc::new(SqlFallbackBackend::new()));

        let state = create_app_state(
            pool.clone(),
            (*config).clone(),
            runtime_config.clone(),
            nats_state,
            search_backend.clone(),
            api::ffmpeg::detect::HwAccel::None,
            api::ffmpeg::detect::EncoderCaps {
                h264: "libx264".to_string(),
                hevc: None,
                av1: None,
            },
            api::ffmpeg::detect::DecoderCaps {
                nvdec: false,
                fpcalc: false,
            },
            nats_jobs::CancelRegistry::new(),
        );
        let library_events = state.library_events.clone();
        let job_events = state.job_events.clone();
        let ingest_queue_events = state.ingest_queue_events.clone();
        let app_state = state.clone();
        let router = create_router(state);

        // Spawn API server
        let (api_shutdown_tx, api_shutdown_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            axum::serve(api_listener, router)
                .with_graceful_shutdown(async {
                    api_shutdown_rx.await.ok();
                })
                .await
                .ok();
        });

        // Spawn Kobo proxy (if needed)
        if include_proxy {
            let handler = Arc::new(KobblerHandler::new(
                pool.clone(),
                config.clone(),
                runtime_config.clone(),
                nats_publisher.clone(),
            ));
            let logger: Arc<dyn kobo_proxy::RequestLogger> = Arc::new(DatabaseLogger::new(
                pool.clone(),
                BlobStorage::new(&config.storage_root),
                runtime_config.clone(),
                library_events.clone(),
            ));

            tokio::spawn(async move {
                if let Err(e) = serve_proxy(proxy_listener, handler, Some(logger)).await {
                    eprintln!("Proxy server failed: {e}");
                }
            });
        }

        // Bootstrap: create admin user + tokens directly in DB (skip HTTP round-trips)
        let user_id = uuid::Uuid::new_v4();
        let password_hash = api::auth::hash_password("testadmin123").unwrap();
        sqlx::query("INSERT INTO users (id, email, password_hash) VALUES ($1, $2, $3)")
            .bind(user_id)
            .bind("admin@test.local")
            .bind(&password_hash)
            .execute(&pool)
            .await
            .expect("Failed to create admin user");

        // Generate auth token directly (skips HTTP login round-trip)
        let admin_token = generate_token(32);
        auth_tokens::create(&pool, user_id, &admin_token, "admin")
            .await
            .expect("Failed to create admin auth token");

        // Generate ingest token directly (skips HTTP round-trip)
        let ingest_token = generate_token(32);
        ingest_tokens::create(&pool, &ingest_token, "test")
            .await
            .expect("Failed to create ingest token");

        Self {
            api_url,
            proxy_url,
            admin_token,
            ingest_token,
            pool,
            nats_publisher,
            runtime_config,
            search_backend,
            library_events,
            job_events,
            ingest_queue_events,
            app_state,
            _storage_dir: storage_dir,
            _config_dir: config_dir,
            _api_shutdown: Some(api_shutdown_tx),
            db_name,
        }
    }

    /// Get a client for admin API requests.
    pub fn admin_client(&self) -> TestClient {
        TestClient::new(&self.api_url).with_auth(&self.admin_token)
    }

    /// Get a client for ingest API requests.
    pub fn ingest_client(&self) -> TestClient {
        TestClient::new(&self.api_url).with_auth(&self.ingest_token)
    }

    /// Get a client for general API requests (no auth).
    pub fn api_client(&self) -> TestClient {
        TestClient::new(&self.api_url)
    }

    /// Get a client for Kobo proxy requests.
    /// Device token should be included in the request path.
    pub fn kobo_client(&self) -> TestClient {
        TestClient::new(&self.proxy_url)
    }

    /// Get the search backend for testing indexing queries.
    pub fn search_backend(&self) -> &dyn SearchBackend {
        self.search_backend.as_ref()
    }

    /// Get the search backend as an Arc (needed for handler construction).
    pub fn search_backend_arc(&self) -> Arc<dyn SearchBackend> {
        self.search_backend.clone()
    }

    /// Get the storage root path.
    pub fn storage_root(&self) -> String {
        self._storage_dir.path().to_string_lossy().to_string()
    }

    /// Tests don't run a NATS worker, so this executes each pending job of the handler's
    /// type in place and records the result in `job_status`.
    pub async fn process_pending_jobs(&self, handler: &dyn JobHandler) {
        let rows: Vec<(uuid::Uuid, serde_json::Value)> = sqlx::query_as(
            "SELECT id, payload FROM job_status \
             WHERE job_type = $1 AND status = 'pending' \
             ORDER BY created_at",
        )
        .bind(handler.job_type())
        .fetch_all(&self.pool)
        .await
        .expect("query pending jobs");

        for (job_id, payload) in rows {
            sqlx::query(
                "UPDATE job_status SET status = 'running', started_at = NOW() WHERE id = $1",
            )
            .bind(job_id)
            .execute(&self.pool)
            .await
            .ok();

            match handler
                .execute(job_id, payload, CancellationToken::new())
                .await
            {
                Ok(result) => {
                    sqlx::query(
                        "UPDATE job_status SET status = 'completed', result = $1, completed_at = NOW() WHERE id = $2",
                    )
                    .bind(result)
                    .bind(job_id)
                    .execute(&self.pool)
                    .await
                    .ok();
                }
                Err(e) => {
                    sqlx::query(
                        "UPDATE job_status SET status = 'failed', error = $1, completed_at = NOW() WHERE id = $2",
                    )
                    .bind(e.to_string())
                    .bind(job_id)
                    .execute(&self.pool)
                    .await
                    .ok();
                }
            }
        }
    }

    pub async fn process_pending_ingest_jobs(&self) {
        self.process_pending_jobs(&KobloadIngestHandler::new(self.app_state.clone()))
            .await;
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        // Shutdown API server
        if let Some(tx) = self._api_shutdown.take() {
            tx.send(()).ok();
        }

        drop(self.pool.close());

        // A run clones one database per test from the template, ~17 MB each on the
        // tmpfs-backed Postgres; leaving them for the next run's sweep holds tens of
        // GB in RAM by the end of a full suite. Not waited for: a drop under load has
        // taken a minute, and blocking here stalls every other server on this test's
        // runtime, which then times out its requests.
        let db_name = std::mem::take(&mut self.db_name);
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("drop runtime")
                .block_on(async {
                    let mut conn = admin_connect().await;
                    sqlx::query(sqlx::AssertSqlSafe(format!(
                        "DROP DATABASE IF EXISTS {db_name} WITH (FORCE)"
                    )))
                    .execute(&mut conn)
                    .await
                    .ok();
                });
        });
    }
}

// =============================================================================
// Test Helper Functions - Create data via APIs
// =============================================================================

/// Create a user via the admin API.
pub async fn create_user(server: &TestServer, email: &str) -> uuid::Uuid {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/users",
            &json!({
                "email": email,
                "password": "testpass123"
            }),
        )
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    json["user_id"]
        .as_str()
        .expect("No user_id in response")
        .parse()
        .expect("Invalid UUID")
}

/// Create a user with a specific role via the admin API. Returns the user ID.
pub async fn create_user_with_role(server: &TestServer, email: &str, role: &str) -> uuid::Uuid {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/users",
            &json!({
                "email": email,
                "password": "testpass123",
                "role": role
            }),
        )
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    json["user_id"]
        .as_str()
        .expect("No user_id in response")
        .parse()
        .expect("Invalid UUID")
}

/// Create a user with a role and return an authenticated TestClient for them.
pub async fn create_user_client(server: &TestServer, email: &str, role: &str) -> TestClient {
    create_user_with_role(server, email, role).await;
    let client = TestClient::new(&server.api_url);
    let resp = client
        .post_json(
            "/api/v1/admin/login",
            &json!({
                "email": email,
                "password": "testpass123"
            }),
        )
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let token = json["token"].as_str().expect("No token in login response");
    TestClient::new(&server.api_url).with_auth(token)
}

/// Create a device via the admin API. Returns (device_id, device_token).
pub async fn create_device(server: &TestServer, user_id: uuid::Uuid) -> (uuid::Uuid, String) {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/devices",
            &json!({
                "user_id": user_id,
                "name": "Test Device"
            }),
        )
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let id = json["device_id"]
        .as_str()
        .expect("No device_id")
        .parse()
        .expect("Invalid UUID");
    let token = json["device_token"]
        .as_str()
        .expect("No device_token")
        .to_string();

    // Enable auto-sync so books ingested via auto_sync appear on this device
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/devices/{}", id),
            &json!({ "auto_sync_new_books": true }),
        )
        .await
        .assert_status(reqwest::StatusCode::NO_CONTENT);

    (id, token)
}

/// Create a book via the ingest API.
pub async fn create_book(server: &TestServer, title: &str) -> uuid::Uuid {
    create_book_with_author(server, title, "Test Author").await
}

/// The seeded admin user, whose session `admin_client` uses.
pub async fn get_admin_user_id(server: &TestServer) -> uuid::Uuid {
    sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM users WHERE email = 'admin@test.local'")
        .fetch_one(&server.pool)
        .await
        .expect("admin user must exist")
}

/// Create a book with a specific author via the ingest API.
pub async fn create_book_with_author(server: &TestServer, title: &str, author: &str) -> uuid::Uuid {
    let book_id = uuid::Uuid::new_v4();
    let resp = server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book_id),
            &json!({
                "title": title,
                "authors": [author]
            }),
        )
        .await;
    resp.assert_success();
    book_id
}

/// Create a book with series info via the ingest API.
pub async fn create_book_in_series(
    server: &TestServer,
    title: &str,
    series_name: &str,
    series_index: i32,
) -> uuid::Uuid {
    let book_id = uuid::Uuid::new_v4();
    let resp = server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book_id),
            &json!({
                "title": title,
                "authors": ["Test Author"],
                "series_name": series_name,
                "series_index": series_index
            }),
        )
        .await;
    resp.assert_success();
    book_id
}

/// Create an ALL_USERS entitlement via the ingest API.
pub async fn entitle_book(server: &TestServer, book_id: uuid::Uuid) {
    let resp = server
        .ingest_client()
        .post_json(
            "/api/v1/entitlements",
            &json!({
                "book_id": book_id,
                "scope_type": "ALL_USERS"
            }),
        )
        .await;
    resp.assert_success();
}

/// Create a USER-scoped entitlement via the ingest API.
pub async fn entitle_book_to_user(server: &TestServer, book_id: uuid::Uuid, user_id: uuid::Uuid) {
    let resp = server
        .ingest_client()
        .post_json(
            "/api/v1/entitlements",
            &json!({
                "book_id": book_id,
                "scope_type": "USER",
                "scope_id": user_id
            }),
        )
        .await;
    resp.assert_success();
}

/// Create a DEVICE-scoped entitlement via the ingest API.
pub async fn entitle_book_to_device(
    server: &TestServer,
    book_id: uuid::Uuid,
    device_id: uuid::Uuid,
) {
    let resp = server
        .ingest_client()
        .post_json(
            "/api/v1/entitlements",
            &json!({
                "book_id": book_id,
                "scope_type": "DEVICE",
                "scope_id": device_id
            }),
        )
        .await;
    resp.assert_success();
}

/// Set the upstream Kobo API URL for a device.
/// Used in merge mode tests to point devices at a mock server.
pub async fn set_device_upstream(server: &TestServer, device_id: uuid::Uuid, upstream_url: &str) {
    sqlx::query("UPDATE devices SET kobo_api_url = $1 WHERE id = $2")
        .bind(upstream_url)
        .bind(device_id)
        .execute(&server.pool)
        .await
        .expect("Failed to update device upstream URL");
}

/// Upload a cover image for a book.
pub async fn upload_cover(server: &TestServer, book_id: uuid::Uuid, image_bytes: Vec<u8>) {
    let resp = server
        .ingest_client()
        .put_bytes(
            &format!("/api/v1/books/{}/cover", book_id),
            image_bytes,
            "image/jpeg",
        )
        .await;
    resp.assert_success();
}

/// Upload an EPUB file for a book.
pub async fn upload_epub(server: &TestServer, book_id: uuid::Uuid, epub_bytes: Vec<u8>) {
    let resp = server
        .ingest_client()
        .put_bytes(
            &format!("/api/v1/books/{}/files/KEPUB", book_id),
            epub_bytes,
            "application/epub+zip",
        )
        .await;
    resp.assert_success();
}

/// Create a minimal valid JPEG image for testing.
pub fn test_jpeg_image() -> Vec<u8> {
    // Minimal valid JPEG (1x1 red pixel)
    vec![
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00,
        0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x08, 0x06, 0x06, 0x07, 0x06,
        0x05, 0x08, 0x07, 0x07, 0x07, 0x09, 0x09, 0x08, 0x0A, 0x0C, 0x14, 0x0D, 0x0C, 0x0B, 0x0B,
        0x0C, 0x19, 0x12, 0x13, 0x0F, 0x14, 0x1D, 0x1A, 0x1F, 0x1E, 0x1D, 0x1A, 0x1C, 0x1C, 0x20,
        0x24, 0x2E, 0x27, 0x20, 0x22, 0x2C, 0x23, 0x1C, 0x1C, 0x28, 0x37, 0x29, 0x2C, 0x30, 0x31,
        0x34, 0x34, 0x34, 0x1F, 0x27, 0x39, 0x3D, 0x38, 0x32, 0x3C, 0x2E, 0x33, 0x34, 0x32, 0xFF,
        0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, 0x11, 0x00, 0xFF, 0xC4, 0x00,
        0x1F, 0x00, 0x00, 0x01, 0x05, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B,
        0xFF, 0xC4, 0x00, 0xB5, 0x10, 0x00, 0x02, 0x01, 0x03, 0x03, 0x02, 0x04, 0x03, 0x05, 0x05,
        0x04, 0x04, 0x00, 0x00, 0x01, 0x7D, 0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21,
        0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08,
        0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A,
        0x16, 0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x34, 0x35, 0x36, 0x37,
        0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56,
        0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73, 0x74, 0x75,
        0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8A, 0x92, 0x93,
        0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9,
        0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6,
        0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2,
        0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7,
        0xF8, 0xF9, 0xFA, 0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00, 0xFB, 0xD5,
        0xDB, 0x20, 0xA8, 0xF7, 0xFF, 0xD9,
    ]
}

/// Create minimal EPUB bytes for testing.
pub fn test_epub_bytes() -> Vec<u8> {
    // Just return some bytes - the server doesn't validate EPUB content
    b"PK\x03\x04fake epub content for testing".to_vec()
}

/// Touch a book to force it to appear in next sync (updates its updated_at).
pub async fn touch_book(server: &TestServer, book_id: uuid::Uuid) {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/library/books/touch",
            &json!({
                "book_ids": [book_id]
            }),
        )
        .await;
    resp.assert_success();
}

/// Resync a device (reset sync state).
pub async fn resync_device(server: &TestServer, device_id: uuid::Uuid) {
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/devices/{}/resync", device_id),
            &json!({}),
        )
        .await
        .assert_success();
}

/// Add an item directly to the ingest queue (bypasses scanning).
/// Returns the queue item ID.
pub async fn add_queue_item(
    server: &TestServer,
    file_path: &str,
    file_name: &str,
    source_format: &str,
) -> uuid::Uuid {
    let item_id = uuid::Uuid::new_v4();
    // Create a book with ingest_status set (unified model)
    sqlx::query(
        r#"
        INSERT INTO books (id, title, want_status, ingest_status)
        VALUES ($1, $2, 'none', 'pending')
        "#,
    )
    .bind(item_id)
    .bind(file_name)
    .execute(&server.pool)
    .await
    .expect("Failed to insert book for queue item");

    // Create the corresponding ingest source record
    sqlx::query(
        r#"
        INSERT INTO book_ingest_source (book_id, file_path, file_name, file_size, source_format)
        VALUES ($1, $2, $3, 1024, $4)
        "#,
    )
    .bind(item_id)
    .bind(file_path)
    .bind(file_name)
    .bind(source_format)
    .execute(&server.pool)
    .await
    .expect("Failed to insert ingest source");

    item_id
}

/// Enable URL rewriting via the admin API config endpoint.
pub async fn enable_url_rewriting(server: &TestServer) {
    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "rewrite_kobo_urls": true
            }),
        )
        .await;
    resp.assert_success();
}

/// Wait for a resource to be deleted (returns 404).
/// Polls the resource until it returns 404 or times out.
pub async fn wait_for_deletion(client: &TestClient, path: &str, timeout_ms: u64) {
    use std::time::Instant;
    use tokio::time::Duration;

    let start = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);

    while start.elapsed() < timeout {
        let resp = client.get(path).await;
        if resp.status == reqwest::StatusCode::NOT_FOUND {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    panic!(
        "Resource at {} was not deleted within {}ms",
        path, timeout_ms
    );
}

/// Add an ebook file to a book.
pub async fn add_ebook_file(server: &TestServer, book_id: uuid::Uuid) {
    sqlx::query(
        r#"
        INSERT INTO book_files (id, book_id, format, size_bytes, file_path, file_hash, source_path, source_type, source_metadata, added_at)
        VALUES ($1, $2, 'EPUB', 1000, '/test/book.epub', 'hash', '/src/book.epub', 'watch_directory', '{}', NOW())
        "#,
    )
    .bind(uuid::Uuid::new_v4())
    .bind(book_id)
    .execute(&server.pool)
    .await
    .expect("Failed to create book file");
}

/// Add an audiobook file to a book.
pub async fn add_audiobook_file(server: &TestServer, book_id: uuid::Uuid) {
    sqlx::query(
        r#"
        INSERT INTO audiobook_files (id, book_id, file_index, format, file_path, file_size, added_at)
        VALUES ($1, $2, 0, 'M4B', '/test/book.m4b', 500000, NOW())
        "#,
    )
    .bind(uuid::Uuid::now_v7())
    .bind(book_id)
    .execute(&server.pool)
    .await
    .expect("Failed to create audiobook file");
}
