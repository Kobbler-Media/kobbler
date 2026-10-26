//! Background watcher service.
//!
//! Monitors media root directories for new media files and enqueues
//! `media_root_scan` jobs. Real-time filesystem events are handled via notify.
//! Periodic polling is handled by the NATS scheduler (`media_root_scan_check` jobs).

use crate::DbPool;
use crate::repositories::media_roots;
use nats_jobs::Publisher;
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebouncedEventKind, Debouncer, new_debouncer};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

/// Configuration for the watcher service.
#[derive(Debug, Clone)]
pub struct WatcherConfig {
    /// Debounce delay for filesystem events.
    pub debounce_delay: Duration,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            debounce_delay: Duration::from_secs(2),
        }
    }
}

/// The background watcher service.
pub struct WatcherService {
    pool: DbPool,
    config: WatcherConfig,
    /// Active watchers keyed by media root ID.
    media_root_watchers: Arc<RwLock<HashMap<Uuid, ActiveWatcher>>>,
    /// Shutdown signal.
    shutdown_tx: Option<mpsc::Sender<()>>,
    /// NATS publisher for job enqueueing.
    nats_publisher: Publisher,
}

struct ActiveWatcher {
    #[allow(dead_code)]
    directory_id: Uuid,
    #[allow(dead_code)]
    path: PathBuf,
    /// The debouncer must be kept alive for real-time watching to work.
    /// When this is Some, real-time filesystem events are active.
    /// When None, the directory falls back to polling.
    #[allow(dead_code)]
    debouncer: Option<Debouncer<RecommendedWatcher>>,
}

impl WatcherService {
    /// Create a new watcher service.
    pub fn new(pool: DbPool, config: WatcherConfig, nats_publisher: Publisher) -> Self {
        Self {
            pool,
            config,
            media_root_watchers: Arc::new(RwLock::new(HashMap::new())),
            shutdown_tx: None,
            nats_publisher,
        }
    }

    /// Start the watcher service.
    ///
    /// This sets up real-time filesystem watchers for enabled directories.
    /// Periodic scanning is handled by the NATS scheduler via `directory_scan_check`.
    pub async fn start(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Starting watcher service");

        let (shutdown_tx, _shutdown_rx) = mpsc::channel::<()>(1);
        self.shutdown_tx = Some(shutdown_tx);

        // Load enabled watch directories and set up real-time watchers
        self.refresh_watchers().await?;

        Ok(())
    }

    /// Stop the watcher service.
    pub async fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }
        self.media_root_watchers.write().await.clear();
        info!("Watcher service stopped");
    }

    /// Refresh watchers from database.
    pub async fn refresh_watchers(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.refresh_media_root_watchers().await?;
        Ok(())
    }

    /// Refresh media root watchers from database.
    async fn refresh_media_root_watchers(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let roots = media_roots::get_all_enabled(&self.pool).await?;

        let mut watchers = self.media_root_watchers.write().await;

        // Remove watchers for media roots that are no longer enabled
        let current_ids: Vec<Uuid> = roots.iter().map(|r| r.id).collect();
        watchers.retain(|id, _| current_ids.contains(id));

        // Add watchers for new media roots
        for root in roots {
            if let Entry::Vacant(entry) = watchers.entry(root.id) {
                let path = PathBuf::from(&root.path);

                if !path.exists() {
                    warn!("Media root directory does not exist: {}", root.path);
                    continue;
                }

                let debouncer = self.setup_media_root_notify_watcher(&root, &path).await;

                let has_realtime = debouncer.is_some();
                entry.insert(ActiveWatcher {
                    directory_id: root.id,
                    path,
                    debouncer,
                });

                if has_realtime {
                    info!(
                        "Watching media root with real-time events: {} ({})",
                        root.name, root.path
                    );
                } else {
                    info!(
                        "Watching media root with polling fallback: {} ({})",
                        root.name, root.path
                    );
                }
            }
        }

        Ok(())
    }

    /// Set up a notify-based real-time watcher for a media root directory.
    /// Returns the debouncer which must be kept alive for events to be received.
    /// Returns None for network filesystems (NFS/SMB) where inotify doesn't work.
    async fn setup_media_root_notify_watcher(
        &self,
        root: &media_roots::EnabledMediaRoot,
        path: &Path,
    ) -> Option<Debouncer<RecommendedWatcher>> {
        if is_network_mount(path) {
            info!(
                "Media root {} ({}) is on a network filesystem, skipping inotify (polling only)",
                root.name,
                path.display()
            );
            return None;
        }
        let pool = self.pool.clone();
        let nats = self.nats_publisher.clone();
        let media_root_id = root.id;
        let root_name = root.name.clone();
        let root_path = path.to_path_buf();
        let media_type = root.media_type.clone();
        let debounce_delay = self.config.debounce_delay;

        // Create event channel for file paths
        let (tx, mut rx) = mpsc::channel::<PathBuf>(100);

        // Spawn handler task that coalesces file events into batched scan jobs.
        // Instead of one scan job per file, we collect events over a short window:
        // - 1 file → single-file scan
        // - 2+ files → full directory scan (avoids thousands of individual jobs)
        tokio::spawn(async move {
            loop {
                // Wait for the first event
                let first = match rx.recv().await {
                    Some(p) => p,
                    None => break, // Channel closed
                };

                // Collect additional events over a short coalescing window
                let mut batch = vec![first];
                let coalesce_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
                loop {
                    match tokio::time::timeout_at(coalesce_deadline, rx.recv()).await {
                        Ok(Some(path)) => batch.push(path),
                        Ok(None) => return, // Channel closed
                        Err(_) => break,    // Timeout — process batch
                    }
                }

                // Filter to media files only
                let media_files: Vec<PathBuf> = batch
                    .into_iter()
                    .filter(|p| {
                        let is_media =
                            crate::jobs::handlers::media_root_scan::is_media_file(p, &media_type);
                        if !is_media {
                            trace!("Skipping non-media file event: {}", p.display());
                        }
                        is_media
                    })
                    .collect();

                if media_files.is_empty() {
                    continue;
                }

                // Skip if there's already an active scan for this media root
                let media_root_id_str = media_root_id.to_string();
                match crate::jobs::has_active_job(
                    &pool,
                    "media_root_scan",
                    "media_root_id",
                    &media_root_id_str,
                )
                .await
                {
                    Ok(true) => {
                        debug!(
                            "Skipping watcher enqueue for {} - scan already active for media root {}",
                            root_name, media_root_id
                        );
                        continue;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        warn!(
                            "Failed to check for active scan job for {}: {}",
                            root_name, e
                        );
                        // Continue to enqueue on check failure
                    }
                }

                // Single file → single-file scan; multiple → full directory scan
                let (payload, description) = if media_files.len() == 1 {
                    let file_path = &media_files[0];
                    let relative = file_path
                        .strip_prefix(&root_path)
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|_| file_path.to_string_lossy().to_string());
                    let file_name = file_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown file")
                        .to_string();
                    (
                        serde_json::json!({
                            "media_root_id": media_root_id,
                            "single_file": relative,
                        }),
                        format!("Processing '{}' in '{}'", file_name, root_name),
                    )
                } else {
                    // Full directory scan supersedes any pending single-file scans
                    if let Err(e) = crate::jobs::cancel_pending_jobs_with_field(
                        &pool,
                        "media_root_scan",
                        "media_root_id",
                        &media_root_id_str,
                        "single_file",
                    )
                    .await
                    {
                        warn!(
                            "Failed to cancel pending single-file scans for {}: {}",
                            root_name, e
                        );
                    }

                    let count = media_files.len();
                    (
                        serde_json::json!({
                            "media_root_id": media_root_id,
                        }),
                        format!("Scanning '{}' ({} file events coalesced)", root_name, count),
                    )
                };

                let display =
                    crate::jobs::fetch_job_display(&pool, "media_root", media_root_id).await;
                let context =
                    nats_jobs::JobContext::for_entity("media_root", media_root_id, description)
                        .with_display_opt(display);

                match crate::jobs::enqueue_routine(
                    &pool,
                    &nats,
                    "media_root_scan",
                    payload,
                    context,
                )
                .await
                {
                    Ok(job_id) => {
                        debug!(
                            "Enqueued media_root_scan job {} for {} file(s) in {}",
                            job_id,
                            media_files.len(),
                            root_name,
                        );
                    }
                    Err(e) => {
                        error!("Failed to enqueue media_root_scan for {}: {}", root_name, e);
                    }
                }
            }
        });

        // Create debounced watcher - media roots use recursive mode
        let tx_clone = tx.clone();
        match new_debouncer(
            debounce_delay,
            move |res: Result<Vec<notify_debouncer_mini::DebouncedEvent>, notify::Error>| {
                if let Ok(events) = res {
                    for event in events {
                        if event.kind == DebouncedEventKind::Any {
                            let tx = tx_clone.clone();
                            let _ = tx.try_send(event.path);
                        }
                    }
                }
            },
        ) {
            Ok(mut debouncer) => {
                if let Err(e) = debouncer.watcher().watch(path, RecursiveMode::Recursive) {
                    warn!(
                        "Failed to set up real-time watcher for media root {}: {}. Using polling fallback.",
                        path.display(),
                        e
                    );
                    return None;
                }

                info!("Real-time watcher active for media root {}", path.display());
                Some(debouncer)
            }
            Err(e) => {
                warn!(
                    "Failed to create debouncer for media root {}: {}. Using polling fallback.",
                    path.display(),
                    e
                );
                None
            }
        }
    }
}

/// Check if a path resides on a network filesystem (NFS or SMB/CIFS).
///
/// inotify only works on local Linux filesystems. Network mounts (NFS, SMB)
/// silently accept watch registrations but never deliver events. This function
/// uses `statfs(2)` to detect the filesystem type so we can skip inotify setup
/// and rely on the polling fallback instead.
fn is_network_mount(path: &Path) -> bool {
    use nix::sys::statfs::{FsType, NFS_SUPER_MAGIC, SMB_SUPER_MAGIC, statfs};
    // SMB2/3 uses a different magic number (0xFE534D42) not exported by nix
    const SMB2_SUPER_MAGIC: FsType = FsType(0xFE534D42);
    match statfs(path) {
        Ok(stat) => {
            let fstype = stat.filesystem_type();
            fstype == NFS_SUPER_MAGIC || fstype == SMB_SUPER_MAGIC || fstype == SMB2_SUPER_MAGIC
        }
        Err(_) => false, // conservative: attempt inotify if detection fails
    }
}
