use std::env;
use std::str::FromStr;

/// Kobo proxy operating mode
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum KoboMode {
    /// Smart proxy: merge our books with Kobo's responses
    #[default]
    Merge,
    /// Pure proxy: Kobbler doesn't exist, just log and forward everything
    Passthrough,
    /// Local only: never call Kobo, stub unknown endpoints
    Local,
}

impl FromStr for KoboMode {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "passthrough" => KoboMode::Passthrough,
            "local" => KoboMode::Local,
            _ => KoboMode::Merge,
        })
    }
}

impl KoboMode {
    /// Whether we should proxy requests to Kobo upstream
    pub fn should_proxy_upstream(&self) -> bool {
        matches!(self, KoboMode::Merge | KoboMode::Passthrough)
    }

    /// Whether we should intercept and handle our own IDs locally
    pub fn should_intercept_local(&self) -> bool {
        matches!(self, KoboMode::Merge | KoboMode::Local)
    }

    /// Whether we should merge our data with Kobo's
    pub fn should_merge(&self) -> bool {
        matches!(self, KoboMode::Merge)
    }
}

/// Static configuration that cannot change at runtime.
/// Dynamic settings are in RuntimeConfig (loaded from config file).
#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: String,
    pub storage_root: String,
    pub config_path: String,
    pub proxy_port: u16,
    pub admin_email: String,
    pub web_origins: Vec<String>,
    // Configurable limits (previously magic numbers)
    pub database_max_connections: u32,
    /// Maximum body size for incoming HTTP requests (uploads, API payloads).
    /// File downloads from Kobload are streamed and bypass this limit.
    /// Default: 200MB. Override with `HTTP_MAX_BODY_BYTES` env var.
    /// If using a reverse proxy (e.g., Nginx Proxy Manager), also set
    /// `client_max_body_size 0;` and `proxy_request_buffering off;` there.
    pub http_max_body_bytes: usize,
    pub library_events_buffer: usize,
    pub token_length: usize,
    // MeiliSearch
    pub meili_url: String,
    pub meili_master_key: String,
    /// Enable CIFS NAS contention monitoring via the share-reconnect counter.
    /// Off by default.
    pub nas_health_monitoring: bool,
    /// Path the monitor reads for cumulative SMB share-reconnect counters.
    /// On bare metal this is `/proc/fs/cifs/Stats`. In containerised deploys
    /// runc forbids bind-mounting anything inside /proc, so you must point
    /// this at a host-side file (e.g. `/var/lib/kobbler/nas-stats`) that a
    /// host cron refreshes from `/proc/fs/cifs/Stats` every minute, and
    /// bind-mount that file into the api container at the same path.
    pub nas_health_stats_path: String,
    pub pin_service_url: String,
    pub stremio_api_url: String,
}

impl Config {
    pub fn from_env() -> Self {
        let database_url = env::var("DATABASE_URL").expect("DATABASE_URL is required");
        let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
        let storage_root = env::var("STORAGE_ROOT").unwrap_or_else(|_| "/data".to_string());
        let config_path =
            env::var("CONFIG_PATH").unwrap_or_else(|_| "/config/kobbler.toml".to_string());
        let proxy_port = env::var("PROXY_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(6767);
        let admin_email = env::var("ADMIN_EMAIL").unwrap_or_else(|_| "admin@localhost".to_string());
        // CORS origins - empty means allow all (safe since API has auth)
        let web_origins = env::var("KOBBLER_WEB_ORIGINS")
            .ok()
            .map(|val| {
                val.split(',')
                    .map(|origin| origin.trim().to_string())
                    .filter(|origin| !origin.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let database_max_connections = env::var("DATABASE_MAX_CONNECTIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);
        let http_max_body_bytes = env::var("HTTP_MAX_BODY_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1024 * 1024 * 200); // 200MB
        // Kobload download progress shares this bus with library changes, so the
        // slow consumers (search indexing enqueues a job per event) can fall behind
        // a burst. A lag drops index jobs that nothing re-enqueues, and the ring is
        // ~120 bytes per slot, so headroom is far cheaper than the failure.
        let library_events_buffer = env::var("LIBRARY_EVENTS_BUFFER")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5000);
        let token_length = env::var("TOKEN_LENGTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(32);

        let meili_url =
            env::var("MEILI_URL").unwrap_or_else(|_| "http://localhost:7700".to_string());
        let meili_master_key =
            env::var("MEILI_MASTER_KEY").unwrap_or_else(|_| "dev-master-key".to_string());
        let nas_health_monitoring = env::var("KOBBLER_NAS_HEALTH_MONITORING")
            .ok()
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
            .unwrap_or(false);
        let nas_health_stats_path = env::var("KOBBLER_NAS_HEALTH_STATS_PATH")
            .unwrap_or_else(|_| "/proc/fs/cifs/Stats".to_string());
        let pin_service_url = env::var("KOBBLER_PIN_SERVICE_URL")
            .ok()
            .map(|v| v.trim().trim_end_matches('/').to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "https://pin.biboop.com".to_string());
        let stremio_api_url = env::var("KOBBLER_STREMIO_API_URL")
            .ok()
            .map(|v| v.trim().trim_end_matches('/').to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "https://api.strem.io".to_string());

        Self {
            database_url,
            bind_addr,
            storage_root,
            config_path,
            proxy_port,
            admin_email,
            web_origins,
            database_max_connections,
            http_max_body_bytes,
            library_events_buffer,
            token_length,
            meili_url,
            meili_master_key,
            nas_health_monitoring,
            nas_health_stats_path,
            pin_service_url,
            stremio_api_url,
        }
    }

    /// Get the proxy base URL for Kobo device sync.
    /// This should be the full public URL where the Kobo proxy is accessible,
    /// including any mount point (e.g., "https://example.com/kobo").
    pub fn public_proxy_url(&self, public_proxy_url: &str) -> String {
        public_proxy_url.trim_end_matches('/').to_string()
    }
}
