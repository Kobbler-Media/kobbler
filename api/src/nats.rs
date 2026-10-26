//! NATS JetStream integration for cross-service communication and job processing.

use nats_jobs::{
    JobHandler as NatsJobHandler, JobStatusDb, NatsClient, NatsConfig, Publisher, ScheduledJob,
    Scheduler, WorkerConfig,
};
use sqlx::PgPool;
use tokio::sync::watch;
use tracing::{error, info};

use crate::repositories::{
    books as books_repo, movies as movies_repo, tv_episodes as episodes_repo,
    tv_seasons as seasons_repo, wanted_attempts as wanted_attempts_repo,
};

use crate::RuntimeConfigManager;
use crate::jobs::handlers::{
    ArtWarmHandler,
    AudiobookChapterRepairHandler,
    AudiobookConvertHandler,
    AudiobookProbeHandler,
    AudiobookTranscribeHandler,
    AuthorEnrichHandler,
    AuthorSyncCheckHandler,
    BookEnrichHandler,
    BookWantCheckHandler,
    BookWantHandler,
    ChapterDetectHandler,
    ChapterRedetectHandler,
    ChatCleanupHandler,
    ChatTurnLogCleanupHandler,
    ChatWatchCleanupHandler,
    CrossFormatSyncHandler,
    DatabaseCleanupHandler,
    DebridCacheCleanupHandler,
    DebridKeepHandler,
    DebridPrewarmHandler,
    DebridProcessHandler,
    DiscoverSyncHandler,
    EbookExtractHandler,
    EbookRepairHandler,
    EnrichAudiobookHandler,
    EntityWriteImagesHandler,
    EpisodeCacheGcHandler,
    EpisodeCacheMigrateHandler,
    FileAvailabilityHandler,
    ForcedSubtitleCheckHandler,
    FormatConversionHandler,
    HardcoverReconcileHandler,
    IptvEpgCleanupHandler,
    IptvHealthCheckHandler,
    IptvOrgRefreshHandler,
    IptvRecordHandler,
    IptvRecordingSchedulerHandler,
    IptvSyncCheckHandler,
    // IPTV handlers
    IptvSyncHandler,
    JobCleanupHandler,
    KepubCheckHandler,
    KeyframeBackfillHandler,
    KindleDeliveryHandler,
    KobloadIngestHandler,
    LivetvAdDetectHandler,
    LivetvAffinityHandler,
    LivetvGuideSyncHandler,
    LivetvRecordHandler,
    LivetvRecordingSchedulerHandler,
    LivetvRetentionHandler,
    LivetvScanHandler,
    MediaAnalyzeBackfillHandler,
    MediaAnalyzeHandler,
    MediaExtractHandler,
    MediaIntegrityCheckHandler,
    MediaProbeHandler,
    MediaRootBackfillHandler,
    MediaRootBookConvertHandler,
    MediaRootBulkResolveHandler,
    MediaRootDeleteHandler,
    MediaRootEnrichHandler,
    MediaRootInterpretHandler,
    MediaRootMatchHandler,
    MediaRootReorganizeHandler,
    MediaRootScanCheckHandler,
    MediaRootScanHandler,
    MediaRootWriteImagesHandler,
    MediaRootWriteSidecarsHandler,
    MetadataExtractionHandler,
    MetadataWriteHandler,
    MigrationEpisodeReconcileHandler,
    MovieEnrichHandler,
    MovieWantCheckHandler,
    MovieWantHandler,
    NotificationSendHandler,
    OrphanCleanupHandler,
    PersonEnrichHandler,
    PersonSyncCheckHandler,
    PlaybackSessionExpiryHandler,
    ProxyLogCleanupHandler,
    SearchIndexHandler,
    SearchReindexHandler,
    SeriesEnrichHandler,
    SeriesSyncCheckHandler,
    SessionCleanupHandler,
    SidecarRewriteHandler,
    SkipDetectHandler,
    SportEventDedupeHandler,
    SportsLeagueEnrichHandler,
    SportsMonitorHandler,
    SportsWantHandler,
    SubtitleSearchHandler,
    SyncProviderHandler,
    SyncPullHandler,
    TempFileCleanupHandler,
    TraktBulkSyncWatchStateHandler,
    TraktImportHistoryHandler,
    TraktSyncWatchStateHandler,
    TranscribeBackfillHandler,
    TvMonitorHandler,
    TvShowEnrichHandler,
    TvWantHandler,
};
use crate::jobs::worker::JobHandler;

/// NATS state for the application (required).
#[derive(Clone)]
pub struct NatsState {
    /// The NATS client.
    pub client: NatsClient,
    /// Publisher for enqueuing jobs and publishing events.
    publisher: Publisher,
}

impl NatsState {
    /// Create a new NATS state.
    pub fn new(client: NatsClient) -> Self {
        let publisher = Publisher::new(client.clone(), "kobbler");
        Self { client, publisher }
    }

    /// Get the publisher for enqueuing jobs.
    pub fn publisher(&self) -> &Publisher {
        &self.publisher
    }
}

/// Initialize NATS connection (required for startup).
pub async fn init_nats() -> Result<NatsClient, String> {
    let config = NatsConfig::from_env("kobbler");

    match NatsClient::try_connect(&config).await {
        Some(client) => {
            info!("Connected to NATS at {}", config.url);
            Ok(client)
        }
        None => Err(format!(
            "Failed to connect to NATS at {}. NATS is required for job processing.",
            config.url
        )),
    }
}

/// Adapter to use Kobbler's JobHandler trait with nats-jobs.
struct HandlerAdapter<H: JobHandler> {
    inner: H,
}

impl<H: JobHandler> HandlerAdapter<H> {
    fn new(handler: H) -> Self {
        Self { inner: handler }
    }
}

#[async_trait::async_trait]
impl<H: JobHandler + 'static> NatsJobHandler for HandlerAdapter<H> {
    fn job_type(&self) -> &'static str {
        self.inner.job_type()
    }

    fn max_concurrent(&self) -> usize {
        self.inner.max_concurrent()
    }

    async fn execute(
        &self,
        job_id: uuid::Uuid,
        payload: serde_json::Value,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<serde_json::Value, nats_jobs::JobError> {
        self.inner
            .execute(job_id, payload, cancel)
            .await
            .map_err(|e| match e {
                crate::jobs::JobError::Cancelled => nats_jobs::JobError::Cancelled,
                e if e.is_retryable() => nats_jobs::JobError::retryable(e.to_string()),
                e => nats_jobs::JobError::permanent(e.to_string()),
            })
    }

    fn post_job_delay(&self, payload: &serde_json::Value) -> Option<std::time::Duration> {
        self.inner.post_job_delay(payload)
    }

    fn max_deliver(&self) -> Option<i64> {
        self.inner.max_deliver()
    }
}

/// Start the NATS-based job worker.
#[allow(clippy::too_many_arguments)]
pub async fn start_nats_worker(
    nats: NatsClient,
    pool: PgPool,
    storage_root: String,
    library_events: tokio::sync::broadcast::Sender<crate::LibraryEvent>,
    job_events: tokio::sync::broadcast::Sender<crate::jobs::JobEvent>,
    ingest_queue_events: tokio::sync::broadcast::Sender<crate::IngestQueueEvent>,
    runtime_config: RuntimeConfigManager,
    search: std::sync::Arc<dyn crate::search::SearchBackend>,
    app_state: crate::AppState,
    shutdown_rx: watch::Receiver<bool>,
    cancel_registry: nats_jobs::CancelRegistry,
) {
    let job_db = JobStatusDb::new(pool.clone());
    let config = WorkerConfig::for_service("kobbler");
    let nats_publisher = Publisher::new(nats.clone(), "kobbler");

    // Status callback: broadcast job status changes to SSE listeners
    let job_events_cb = job_events.clone();
    let status_callback: nats_jobs::StatusCallback =
        std::sync::Arc::new(move |change: nats_jobs::JobStatusChange| {
            let _ = job_events_cb.send(crate::jobs::JobEvent {
                job_id: change.job_id,
                job_type: change.job_type,
                status: change.status.to_string(),
                progress: change.progress,
                error: change.error,
                result: change.result,
                description: change.description,
                entity_type: change.entity_type,
                entity_id: change.entity_id,
                display: change.display,
                created_at: change.created_at,
                scheduled: change.scheduled,
            });
        });

    // Global limit on concurrent filesystem-bound operations.
    // Prevents NAS saturation when many job types are active simultaneously.
    let fs_concurrency: usize = std::env::var("KOBBLER_FS_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let fs_semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(fs_concurrency));
    info!("FS concurrency limit: {fs_concurrency}");

    // Enrichment semaphore: limits concurrent thumbnail/LUFS/peaks processing.
    let enrichment_concurrency: usize = std::env::var("KOBBLER_ENRICHMENT_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let enrichment_semaphore =
        std::sync::Arc::new(tokio::sync::Semaphore::new(enrichment_concurrency));
    info!("Enrichment concurrency limit: {enrichment_concurrency}");

    let mut worker = nats_jobs::JobWorker::new(nats.clone(), config)
        .with_job_db(job_db)
        .with_status_callback(status_callback)
        .with_cancel_registry(cancel_registry);

    // Register handlers using the adapter
    worker.register_handler(HandlerAdapter::new(MetadataWriteHandler::new(pool.clone())));
    worker.register_handler(HandlerAdapter::new(FileAvailabilityHandler::new(
        pool.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(MetadataExtractionHandler::new(
        pool.clone(),
        job_events.clone(),
        ingest_queue_events,
        runtime_config.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(SyncProviderHandler::new(pool.clone())));

    // Kobload ingest handler (async file processing)
    let gpu_lock = app_state.gpu_lock.clone();
    let initial_scan_complete = app_state.initial_scan_complete.clone();
    let media_root_io_gate = app_state.media_root_io_gate.clone();
    worker.register_handler(HandlerAdapter::new(KobloadIngestHandler::new(
        app_state.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(DebridPrewarmHandler::new(
        app_state.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(DebridKeepHandler::new(
        app_state.clone(),
    )));

    // Format conversion handler
    worker.register_handler(HandlerAdapter::new(FormatConversionHandler::new(
        pool.clone(),
        storage_root.clone(),
    )));

    // Audiobook conversion handler (M4B → MP3Z for Kobo)
    worker.register_handler(HandlerAdapter::new(AudiobookConvertHandler::new(
        pool.clone(),
        storage_root.clone(),
    )));

    // Audiobook probe handler (ffprobe + chapter extraction)
    worker.register_handler(HandlerAdapter::new(AudiobookProbeHandler::new(
        pool.clone(),
        storage_root.clone(),
        fs_semaphore.clone(),
        runtime_config.clone(),
        nats_publisher.clone(),
    )));

    // Audiobook transcription handler (faster-whisper speech-to-text)
    worker.register_handler(HandlerAdapter::new(AudiobookTranscribeHandler::new(
        pool.clone(),
        storage_root.clone(),
        runtime_config.clone(),
        nats_publisher.clone(),
        search.clone(),
        gpu_lock.clone(),
    )));

    // Transcript-based chapter detection handler
    worker.register_handler(HandlerAdapter::new(ChapterDetectHandler::new(
        pool.clone(),
        search.clone(),
    )));

    // Chapter re-detection handler (silence + transcript combined)
    worker.register_handler(HandlerAdapter::new(ChapterRedetectHandler::new(
        pool.clone(),
        storage_root.clone(),
    )));

    // Ebook text extraction handler (EPUB → text segments)
    worker.register_handler(HandlerAdapter::new(EbookExtractHandler::new(
        pool.clone(),
        storage_root.clone(),
        nats_publisher.clone(),
        search.clone(),
        fs_semaphore.clone(),
    )));

    worker.register_handler(HandlerAdapter::new(AudiobookChapterRepairHandler::new(
        pool.clone(),
        storage_root.clone(),
        nats_publisher.clone(),
        search.clone(),
    )));

    // Ebook repair handler (re-convert books an older pipeline damaged)
    worker.register_handler(HandlerAdapter::new(EbookRepairHandler::new(
        pool.clone(),
        storage_root.clone(),
        nats_publisher.clone(),
        fs_semaphore.clone(),
    )));

    // KEPUB catch-up (books a Kobo can see but that have no KEPUB yet)
    worker.register_handler(HandlerAdapter::new(KepubCheckHandler::new(
        pool.clone(),
        nats_publisher.clone(),
    )));

    // Cross-format sync handler (align audio timestamps ↔ ebook CFIs)
    worker.register_handler(HandlerAdapter::new(CrossFormatSyncHandler::new(
        pool.clone(),
    )));

    // Scheduled job handlers
    worker.register_handler(HandlerAdapter::new(ProxyLogCleanupHandler::new(
        pool.clone(),
        &storage_root,
        runtime_config.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(ChatTurnLogCleanupHandler::new(
        pool.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(ChatCleanupHandler::new(
        pool.clone(),
        runtime_config.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(ChatWatchCleanupHandler::new(
        pool.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(AuthorSyncCheckHandler::new(
        pool.clone(),
        nats_publisher.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(AuthorEnrichHandler::new(
        pool.clone(),
        nats_publisher.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(SeriesEnrichHandler::new(
        pool.clone(),
        storage_root.clone(),
        nats_publisher.clone(),
        library_events.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(SeriesSyncCheckHandler::new(
        pool.clone(),
        nats_publisher.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(HardcoverReconcileHandler::new(
        pool.clone(),
        nats_publisher.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(BookWantHandler::new(
        pool.clone(),
        runtime_config.clone(),
        nats_publisher.clone(),
        library_events.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(MovieWantHandler::new(
        pool.clone(),
        runtime_config.clone(),
        nats_publisher.clone(),
        library_events.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(MovieWantCheckHandler::new(
        pool.clone(),
        nats_publisher.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(BookWantCheckHandler::new(
        pool.clone(),
        runtime_config.clone(),
        nats_publisher.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(DiscoverSyncHandler::new(
        pool.clone(),
        runtime_config.clone(),
        storage_root.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(
        crate::jobs::handlers::TmdbTitlesSyncHandler::new(
            pool.clone(),
            crate::tmdb_titles::export::EXPORTS_URL.to_string(),
            crate::tmdb_titles::export::ExportLimits::default(),
        ),
    ));
    worker.register_handler(HandlerAdapter::new(
        crate::jobs::handlers::HardcoverTitlesSyncHandler::new(
            pool.clone(),
            runtime_config.clone(),
            crate::hardcover_titles::SyncPlan::default(),
        ),
    ));
    worker.register_handler(HandlerAdapter::new(
        crate::jobs::handlers::StremioSyncHandler::new(
            pool.clone(),
            app_state.config.stremio_api_url.clone(),
        ),
    ));
    worker.register_handler(HandlerAdapter::new(TvWantHandler::new(
        pool.clone(),
        runtime_config.clone(),
        nats_publisher.clone(),
        library_events.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(TvMonitorHandler::new(
        pool.clone(),
        runtime_config.clone(),
        nats_publisher.clone(),
        library_events.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(SportsWantHandler::new(
        pool.clone(),
        runtime_config.clone(),
        nats_publisher.clone(),
        library_events.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(SportsMonitorHandler::new(
        pool.clone(),
        nats_publisher.clone(),
        library_events.clone(),
    )));

    // Search index handlers
    worker.register_handler(HandlerAdapter::new(SearchIndexHandler::new(
        pool.clone(),
        search.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(SearchReindexHandler::new(
        pool.clone(),
        search.clone(),
    )));

    // Maintenance handlers
    worker.register_handler(HandlerAdapter::new(JobCleanupHandler::new(
        pool.clone(),
        nats.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(DatabaseCleanupHandler::new(
        pool.clone(),
        search.clone(),
        nats_publisher.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(DebridCacheCleanupHandler::new(
        pool.clone(),
        std::sync::Arc::new(runtime_config.clone()),
    )));
    worker.register_handler(HandlerAdapter::new(DebridProcessHandler::new(
        pool.clone(),
        nats_publisher.clone(),
        runtime_config.clone(),
        storage_root.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(EpisodeCacheGcHandler::new(
        pool.clone(),
        storage_root.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(MediaIntegrityCheckHandler::new(
        pool.clone(),
        nats_publisher.clone(),
        storage_root.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(EpisodeCacheMigrateHandler::new(
        pool.clone(),
        storage_root.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(ArtWarmHandler::new(
        pool.clone(),
        storage_root.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(TempFileCleanupHandler::new(
        pool.clone(),
        fs_semaphore.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(OrphanCleanupHandler::new(
        pool.clone(),
        storage_root.clone(),
        runtime_config.clone(),
    )));

    // Kindle delivery handler
    worker.register_handler(HandlerAdapter::new(KindleDeliveryHandler::new(
        pool.clone(),
        storage_root.clone(),
    )));

    // Notification send handler
    worker.register_handler(HandlerAdapter::new(NotificationSendHandler::new(
        pool.clone(),
    )));

    // Trakt sync handlers
    worker.register_handler(HandlerAdapter::new(TraktImportHistoryHandler::new(
        pool.clone(),
        crate::video_sync_providers::VideoSyncProviderRegistry::new(),
    )));
    worker.register_handler(HandlerAdapter::new(TraktSyncWatchStateHandler::new(
        pool.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(TraktBulkSyncWatchStateHandler::new(
        pool.clone(),
    )));

    // Sync pull handler (pulls reading data from external providers)
    worker.register_handler(HandlerAdapter::new(SyncPullHandler::new(pool.clone())));

    // Migration handlers
    worker.register_handler(HandlerAdapter::new(MigrationEpisodeReconcileHandler::new(
        pool.clone(),
        nats_publisher.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(SportEventDedupeHandler::new(
        pool.clone(),
    )));

    // Per-entity enrichment handlers
    worker.register_handler(HandlerAdapter::new(MovieEnrichHandler::new(
        pool.clone(),
        nats_publisher.clone(),
        storage_root.clone(),
        library_events.clone(),
        runtime_config.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(TvShowEnrichHandler::new(
        pool.clone(),
        nats_publisher.clone(),
        storage_root.clone(),
        library_events.clone(),
        runtime_config.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(SportsLeagueEnrichHandler::new(
        pool.clone(),
        library_events.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(BookEnrichHandler::new(
        pool.clone(),
        nats_publisher.clone(),
        storage_root.clone(),
        library_events.clone(),
        runtime_config.clone(),
    )));

    // Media root handlers
    worker.register_handler(HandlerAdapter::new(MediaRootScanHandler::new(
        pool.clone(),
        nats_publisher.clone(),
        search.clone(),
        fs_semaphore.clone(),
        runtime_config.clone(),
        initial_scan_complete.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(MediaRootInterpretHandler::new(
        pool.clone(),
        runtime_config.clone(),
        nats_publisher.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(MediaRootMatchHandler::new(
        pool.clone(),
        nats_publisher.clone(),
        search.clone(),
        library_events.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(MediaRootEnrichHandler::new(
        pool.clone(),
        nats_publisher.clone(),
        storage_root.to_string(),
    )));
    worker.register_handler(HandlerAdapter::new(MediaRootScanCheckHandler::new(
        pool.clone(),
        nats_publisher.clone(),
        runtime_config.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(MediaRootReorganizeHandler::new(
        pool.clone(),
        fs_semaphore.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(MediaRootBackfillHandler::new(
        pool.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(MediaRootBookConvertHandler::new(
        pool.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(MediaRootDeleteHandler::new(
        pool.clone(),
        search.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(MediaRootWriteSidecarsHandler::new(
        pool.clone(),
        fs_semaphore.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(SidecarRewriteHandler::new(
        pool.clone(),
        nats_publisher.clone(),
        job_events.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(MediaRootWriteImagesHandler::new(
        pool.clone(),
        fs_semaphore.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(EntityWriteImagesHandler::new(
        pool.clone(),
        storage_root.clone(),
        fs_semaphore.clone(),
        runtime_config.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(MediaRootBulkResolveHandler::new(
        pool.clone(),
        nats_publisher.clone(),
        search.clone(),
        job_events.clone(),
    )));

    // Subtitle search handler
    worker.register_handler(HandlerAdapter::new(SubtitleSearchHandler::new(
        pool.clone(),
    )));

    // Person enrichment and sync handlers
    worker.register_handler(HandlerAdapter::new(PersonEnrichHandler::new(pool.clone())));
    worker.register_handler(HandlerAdapter::new(PersonSyncCheckHandler::new(
        pool.clone(),
        nats_publisher.clone(),
    )));

    // Media probe handler (fast ffprobe metadata)
    worker.register_handler(HandlerAdapter::new(MediaProbeHandler::new(
        pool.clone(),
        nats_publisher.clone(),
        runtime_config.clone(),
        fs_semaphore.clone(),
    )));

    // Media extract handler (heavy I/O: keyframes + subtitle pre-extraction)
    worker.register_handler(HandlerAdapter::new(MediaExtractHandler::new(
        pool.clone(),
        storage_root.clone(),
        runtime_config.clone(),
        fs_semaphore.clone(),
        media_root_io_gate.clone(),
    )));

    // Single-pass media analysis (cache pipeline, the only media-analysis path)
    worker.register_handler(HandlerAdapter::new(MediaAnalyzeHandler::new(
        pool.clone(),
        nats_publisher.clone(),
        runtime_config.clone(),
        enrichment_semaphore.clone(),
        gpu_lock.clone(),
        storage_root.clone(),
        media_root_io_gate.clone(),
    )));

    // V2 skip detection — operates on cached embeddings, writes skip_detect_candidates
    worker.register_handler(HandlerAdapter::new(SkipDetectHandler::new(
        pool.clone(),
        runtime_config.clone(),
        storage_root.clone(),
        library_events.clone(),
    )));

    // Audiobook enrichment handler (peaks + LUFS for audiobook files)
    worker.register_handler(HandlerAdapter::new(EnrichAudiobookHandler::new(
        pool.clone(),
        storage_root.clone(),
        runtime_config.clone(),
        enrichment_semaphore.clone(),
    )));

    // media-cache backfill: walks files whose media_cache_version < PIPELINE_VERSION
    // and enqueues media_analyze. Gated at tick-time on enrichment_enabled.
    worker.register_handler(HandlerAdapter::new(MediaAnalyzeBackfillHandler::new(
        pool.clone(),
        nats_publisher.clone(),
        runtime_config.clone(),
        initial_scan_complete.clone(),
    )));

    // Forced subtitle check handler
    worker.register_handler(HandlerAdapter::new(ForcedSubtitleCheckHandler::new(
        pool.clone(),
        nats_publisher.clone(),
    )));

    // Media backfill handler (enqueues media_probe for files needing probe/extraction)
    worker.register_handler(HandlerAdapter::new(KeyframeBackfillHandler::new(
        pool.clone(),
        nats_publisher.clone(),
        initial_scan_complete.clone(),
    )));

    // Transcribe backfill handler (enqueues audiobook_transcribe for untranscribed books)
    worker.register_handler(HandlerAdapter::new(TranscribeBackfillHandler::new(
        pool.clone(),
        nats_publisher.clone(),
        runtime_config.clone(),
        initial_scan_complete.clone(),
    )));

    // Session cleanup handler (background directory deletion with retries)
    worker.register_handler(HandlerAdapter::new(SessionCleanupHandler));
    worker.register_handler(HandlerAdapter::new(PlaybackSessionExpiryHandler::new(
        pool.clone(),
    )));

    // IPTV handlers
    let iptv_http = crate::http::new_client();
    let iptv_storage_root = storage_root.clone();
    worker.register_handler(HandlerAdapter::new(IptvSyncHandler::new(
        pool.clone(),
        iptv_http.clone(),
        runtime_config.clone(),
        iptv_storage_root.clone(),
        search.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(IptvSyncCheckHandler::new(
        pool.clone(),
        nats_publisher.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(IptvHealthCheckHandler::new(
        pool.clone(),
        runtime_config.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(IptvEpgCleanupHandler::new(
        pool.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(IptvRecordingSchedulerHandler::new(
        pool.clone(),
        nats_publisher.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(IptvRecordHandler::new(
        pool.clone(),
        iptv_storage_root,
    )));
    worker.register_handler(HandlerAdapter::new(IptvOrgRefreshHandler::new(
        pool.clone(),
        iptv_http,
    )));

    // Live TV (HDHomeRun) handlers
    worker.register_handler(HandlerAdapter::new(LivetvScanHandler::new(
        pool.clone(),
        nats_publisher.clone(),
        search.clone(),
        library_events.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(LivetvGuideSyncHandler::new(
        pool.clone(),
        search.clone(),
        library_events.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(LivetvRecordingSchedulerHandler::new(
        pool.clone(),
        nats_publisher.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(LivetvRecordHandler::new(
        app_state.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(LivetvAffinityHandler::new(
        pool.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(LivetvRetentionHandler::new(
        app_state.clone(),
    )));
    worker.register_handler(HandlerAdapter::new(LivetvAdDetectHandler::new(
        app_state.clone(),
    )));

    // Mark orphaned "running" jobs as failed on startup.
    // If we just started, no jobs can legitimately be running — they're orphans
    // from a prior instance. NATS will redeliver their messages anyway, creating
    // fresh pending rows.
    match sqlx::query_scalar!(
        r#"
        WITH updated AS (
            UPDATE job_status
            SET status = 'failed',
                error = 'Orphaned on startup — worker restarted',
                completed_at = NOW()
            WHERE status = 'running'
            RETURNING 1
        )
        SELECT COUNT(*) as "count!" FROM updated
        "#
    )
    .fetch_one(&pool)
    .await
    {
        Ok(count) if count > 0 => {
            info!(
                count = count,
                "Marked orphaned running jobs as failed on startup"
            );
        }
        Err(e) => {
            error!(error = %e, "Failed to reset orphaned running jobs");
        }
        _ => {}
    }

    info!("Starting NATS job worker");
    if let Err(e) = worker.run(shutdown_rx).await {
        error!("NATS job worker error: {}", e);
    }
}

/// Start the scheduler for periodic jobs.
pub async fn start_scheduler(nats: NatsClient, shutdown_rx: watch::Receiver<bool>) {
    let publisher = Publisher::new(nats, "kobbler");
    let mut scheduler = Scheduler::new(publisher);

    // Register periodic jobs
    scheduler.register(ScheduledJob::new(
        "proxy-log-cleanup",
        "kobbler",
        "proxy_log_cleanup",
        std::time::Duration::from_secs(3600), // hourly
    ));

    // Chat turn log cleanup — 30-day retention on the chat debugging log
    scheduler.register(ScheduledJob::new(
        "chat-turn-log-cleanup",
        "kobbler",
        "chat_turn_log_cleanup",
        std::time::Duration::from_secs(24 * 3600), // daily
    ));

    // Tier 2: close conversations that have gone quiet and summarise the ones
    // that did something. Frequent, because the summary is what the next
    // morning's "did that ever finish?" reads.
    scheduler.register(
        ScheduledJob::new(
            "chat-session-close",
            "kobbler",
            "chat_cleanup",
            std::time::Duration::from_secs(600), // every 10 minutes
        )
        .with_payload(|| serde_json::json!({ "mode": "sessions" })),
    );

    // Chat retention — raw turns 30 days, session summaries a year, profile
    // facts until their owner removes them.
    scheduler.register(
        ScheduledJob::new(
            "chat-retention",
            "kobbler",
            "chat_cleanup",
            std::time::Duration::from_secs(24 * 3600), // daily
        )
        .with_payload(|| serde_json::json!({ "mode": "retention" })),
    );

    scheduler.register(
        ScheduledJob::new(
            "playback-session-expiry",
            "kobbler",
            "playback_session_expiry",
            std::time::Duration::from_secs(600),
        )
        .run_immediately(),
    );

    // Chat watch expiry — every expired watch gets a final message before its row goes
    scheduler.register(
        ScheduledJob::new(
            "chat-watch-cleanup",
            "kobbler",
            "chat_watch_cleanup",
            std::time::Duration::from_secs(600), // every 10 minutes
        )
        .run_immediately(),
    );

    // Catch-up for the KEPUB the Kobo actually wants. Short interval because
    // it is one query when there is nothing to do, and because a book sent to
    // the device should not sit as a plain EPUB for long.
    scheduler.register(
        ScheduledJob::new(
            "kepub-check",
            "kobbler",
            "kepub_check",
            std::time::Duration::from_secs(300), // every 5 minutes
        )
        .run_immediately(),
    );

    scheduler.register(
        ScheduledJob::new(
            "author-sync-check",
            "kobbler",
            "author_sync_check",
            std::time::Duration::from_secs(300), // every 5 minutes
        )
        .run_immediately(),
    );

    scheduler.register(
        ScheduledJob::new(
            "series-sync-check",
            "kobbler",
            "series_sync_check",
            std::time::Duration::from_secs(300), // every 5 minutes
        )
        .run_immediately(),
    );

    // Follow Hardcover's merges and deletions across everything linked to it. Not run
    // on boot: it walks the whole catalogue (~600 rate-limited Hardcover requests).
    scheduler.register(ScheduledJob::new(
        "hardcover-reconcile",
        "kobbler",
        "hardcover_reconcile",
        std::time::Duration::from_secs(24 * 3600), // daily
    ));

    // Job cleanup - cleans up stale pending jobs and old completed jobs
    scheduler.register(
        ScheduledJob::new(
            "job-cleanup",
            "kobbler",
            "job_cleanup",
            std::time::Duration::from_secs(1800), // every 30 minutes
        )
        .run_immediately(),
    );

    // Sync pull - fetches user book data from sync providers
    // Run immediately on startup to populate provider read IDs from existing reads
    scheduler.register(
        ScheduledJob::new(
            "sync-pull-hardcover",
            "kobbler",
            "sync_pull",
            std::time::Duration::from_secs(3600), // hourly
        )
        .with_payload(|| serde_json::json!({ "provider_type": "hardcover" }))
        .run_immediately(),
    );

    // Orphan cleanup — remove orphaned images and book file directories
    scheduler.register(ScheduledJob::new(
        "orphan-cleanup",
        "kobbler",
        "orphan_cleanup",
        std::time::Duration::from_secs(24 * 3600), // daily
    ));

    // Database cleanup — removes orphaned rows (e.g. episode files with no episode links)
    scheduler.register(ScheduledJob::new(
        "database-cleanup",
        "kobbler",
        "database_cleanup",
        std::time::Duration::from_secs(24 * 3600), // daily
    ));

    // Debrid cache cleanup — evicts expired/oversized cached stream files
    scheduler.register(ScheduledJob::new(
        "debrid-cache-cleanup",
        "kobbler",
        "debrid_cache_cleanup",
        std::time::Duration::from_secs(900), // every 15 minutes
    ));

    // Media integrity check — re-examines files already in the library for the
    // damage nothing else looks for: a cache that no longer matches its source,
    // or the zero blocks an unfinished download leaves behind.
    //
    // Stamp-only on the schedule. That tier is one `stat` per file and no
    // reads, so it is cheap enough to run daily; the sampling tier reads
    // ~768 KB per file across the whole library and is a manual, off-peak
    // decision rather than something to inflict on the NAS unattended.
    //
    // `remediate_missing` closes the deleting-a-file-by-hand hole: nothing else
    // reconciles a file that disappears out of band (the scan is additive), so
    // its row survives, `is_in_library` stays true, and the episode sits at
    // `imported` where no want pass will ever look at it again. Only the
    // Missing verdict is acted on, and only while every media root reads back
    // — an unmounted share makes the whole library look missing at once. The
    // content-sample verdict still needs a person and `remediate`.
    scheduler.register(
        ScheduledJob::new(
            "media-integrity-check",
            "kobbler",
            "media_integrity_check",
            std::time::Duration::from_secs(24 * 3600), // daily
        )
        .with_payload(|| serde_json::json!({ "stamp_only": true, "remediate_missing": true })),
    );

    // Episode cache GC — drops episode_cache/{kind}/{id}/ dirs whose canonical
    // id is no longer in the DB and prunes pre-MIN_SUPPORTED manifests.
    scheduler.register(ScheduledJob::new(
        "episode-cache-gc",
        "kobbler",
        "episode_cache_gc",
        std::time::Duration::from_secs(7 * 24 * 3600), // weekly
    ));

    // One-shot migration that moves content sidecars (vhash, chromaprint, …)
    // out of legacy `media_cache/{file_id}/` into the new
    // `episode_cache/{namespace}/{id}/` layout. Idempotent — already-migrated
    // dirs are walked but skipped (just a `read_dir` per file_id), so the
    // post-migration steady state is effectively free. `run_immediately`
    // fires on every server startup; the long interval keeps a re-run from
    // firing during normal uptime, but a restart re-validates that no stray
    // legacy sidecars have been re-introduced (e.g. by a brief deploy
    // rollback).
    scheduler.register(
        ScheduledJob::new(
            "episode-cache-migrate",
            "kobbler",
            "episode_cache_migrate",
            std::time::Duration::from_secs(30 * 24 * 3600), // every 30 days
        )
        .run_immediately(),
    );

    // Library art onto disk for titles enriched before enrichment warmed it. On startup
    // too, so the first run after an upgrade fills in what is already there.
    scheduler.register(
        ScheduledJob::new(
            "art-warm",
            "kobbler",
            "art_warm",
            std::time::Duration::from_secs(24 * 3600), // daily
        )
        .run_immediately(),
    );

    // Temp file cleanup — deletes stale .kobbler-dl-* files from media roots
    scheduler.register(ScheduledJob::new(
        "temp-file-cleanup",
        "kobbler",
        "temp_file_cleanup",
        std::time::Duration::from_secs(3600), // hourly
    ));

    // Nightly search reindex — defensive rebuild to catch any index drift
    scheduler.register(ScheduledJob::new(
        "search-reindex-nightly",
        "kobbler",
        "search_reindex",
        std::time::Duration::from_secs(24 * 3600), // daily
    ));

    // Person sync check - auto-wants credits for monitored people
    scheduler.register(
        ScheduledJob::new(
            "person-sync-check",
            "kobbler",
            "person_sync_check",
            std::time::Duration::from_secs(300), // every 5 minutes
        )
        .run_immediately(),
    );

    // Movie want check - re-checks wanted movies whose release date may now be close
    scheduler.register(ScheduledJob::new(
        "movie-want-check",
        "kobbler",
        "movie_want_check",
        std::time::Duration::from_secs(24 * 3600), // daily
    ));

    // Book want check - re-sends wanted books so their Kobload patterns stay current
    scheduler.register(ScheduledJob::new(
        "book-want-check",
        "kobbler",
        "book_want_check",
        std::time::Duration::from_secs(24 * 3600), // daily
    ));

    // Discover sync - fetches trending/popular content from TMDB and Hardcover
    scheduler.register(
        ScheduledJob::new(
            "discover-sync",
            "kobbler",
            "discover_sync",
            std::time::Duration::from_secs(6 * 3600), // every 6 hours
        )
        .run_immediately(),
    );

    // TMDB titles for search suggestions. The handler skips an export it already has and
    // localizes weekly, so this interval only bounds how late a new day's file is picked up.
    scheduler.register(
        ScheduledJob::new(
            "tmdb-titles-sync",
            "kobbler",
            "tmdb_titles_sync",
            std::time::Duration::from_secs(6 * 3600),
        )
        .run_immediately(),
    );

    scheduler.register(
        ScheduledJob::new(
            "hardcover-titles-sync",
            "kobbler",
            "hardcover_titles_sync",
            std::time::Duration::from_secs(3600),
        )
        .run_immediately(),
    );

    // Stremio Continue Watching - mirrors linked Stremio accounts onto the home page
    scheduler.register(
        ScheduledJob::new(
            "stremio-sync",
            "kobbler",
            "stremio_sync",
            std::time::Duration::from_secs(600), // every 10 minutes
        )
        .run_immediately(),
    );

    // TV monitor - checks monitored shows for new episodes
    scheduler.register(ScheduledJob::new(
        "tv-monitor",
        "kobbler",
        "tv_monitor",
        std::time::Duration::from_secs(3600), // hourly
    ));

    // Sports monitor - checks monitored leagues for new events
    scheduler.register(ScheduledJob::new(
        "sports-monitor",
        "kobbler",
        "sports_monitor",
        std::time::Duration::from_secs(3600), // hourly
    ));

    // Media backfill - finds video files needing probe (missing keyframes, never probed)
    // and enqueues media_probe jobs for keyframes + subtitle pre-extraction
    scheduler.register(
        ScheduledJob::new(
            "keyframe-backfill",
            "kobbler",
            "keyframe_backfill",
            std::time::Duration::from_secs(1800), // every 30 minutes
        )
        .run_immediately(),
    );

    // media-cache backfill: finds files with media_cache_version < current
    // and enqueues media_analyze. Gated at tick-time on runtime_config
    // `enrichment_enabled` — flag defaults off in prod so first deploy does
    // not queue the library; operators flip the flag when ready for the load.
    scheduler.register(
        ScheduledJob::new(
            "media-analyze-backfill",
            "kobbler",
            "media_analyze_backfill",
            std::time::Duration::from_secs(1800), // every 30 minutes
        )
        .run_immediately(),
    );

    // Transcribe backfill - finds audiobooks with untranscribed files
    scheduler.register(
        ScheduledJob::new(
            "transcribe-backfill",
            "kobbler",
            "transcribe_backfill",
            std::time::Duration::from_secs(1800), // every 30 minutes
        )
        .run_immediately(),
    );

    // File availability check — catches book/audiobook files that have moved or
    // disappeared on disk. Cheap (~15s for 5k rows) and idempotent.
    scheduler.register(ScheduledJob::new(
        "file-availability-check",
        "kobbler",
        "file_availability_check",
        std::time::Duration::from_secs(1800), // every 30 minutes
    ));

    // Forced subtitle check - finds files missing forced subs and enqueues searches
    scheduler.register(ScheduledJob::new(
        "forced-subtitle-check",
        "kobbler",
        "forced_subtitle_check",
        std::time::Duration::from_secs(6 * 3600), // every 6 hours
    ));

    // Media root scan check - triggers scans for all enabled media roots
    scheduler.register(
        ScheduledJob::new(
            "media-root-scan-check",
            "kobbler",
            "media_root_scan_check",
            std::time::Duration::from_secs(300), // every 5 minutes
        )
        .run_immediately(),
    );

    // IPTV scheduled jobs
    scheduler.register(ScheduledJob::new(
        "iptv-sync-check",
        "kobbler",
        "iptv_sync_check",
        std::time::Duration::from_secs(300), // every 5 minutes
    ));
    scheduler.register(ScheduledJob::new(
        "iptv-health-check",
        "kobbler",
        "iptv_health_check",
        std::time::Duration::from_secs(900), // every 15 minutes
    ));
    scheduler.register(ScheduledJob::new(
        "iptv-epg-cleanup",
        "kobbler",
        "iptv_epg_cleanup",
        std::time::Duration::from_secs(21600), // every 6 hours
    ));
    scheduler.register(ScheduledJob::new(
        "iptv-recording-scheduler",
        "kobbler",
        "iptv_recording_scheduler",
        std::time::Duration::from_secs(60), // every 1 minute
    ));
    scheduler.register(ScheduledJob::new(
        "iptv-org-refresh",
        "kobbler",
        "iptv_org_refresh",
        std::time::Duration::from_secs(86400), // daily
    ));

    // Live TV guide refresh. The feed is HDHomeRun's cloud service, not the tuner on the
    // LAN, and it only ever reaches 48 h ahead — so the whole of it is re-pulled and held
    // here rather than scheduled ahead, and the 24 h prune rides along. Every pass also
    // re-reads what is already stored: a broadcaster moving tonight's film by twenty
    // minutes is a change to an airing we have, not a new one, and two hours is the most
    // of that a viewer should ever be looking at. One pass is a dozen requests.
    scheduler.register(
        ScheduledJob::new(
            "livetv-guide-sync",
            "kobbler",
            "livetv_guide_sync",
            std::time::Duration::from_secs(2 * 3600),
        )
        .run_immediately(),
    );
    // Lineup refresh. Broadcasters retune a few times a year and an LCN renumber
    // silently breaks the guide join, so the scan cannot only ever be manual. The
    // handler skips a tuner scanned in the last 20 h, which is what actually
    // paces this — the scheduler forgets everything across a restart.
    scheduler.register(ScheduledJob::new(
        "livetv-scan",
        "kobbler",
        "livetv_scan",
        std::time::Duration::from_secs(6 * 3600),
    ));
    scheduler.register(ScheduledJob::new(
        "livetv-recording-scheduler",
        "kobbler",
        "livetv_recording_scheduler",
        std::time::Duration::from_secs(60),
    ));
    // Watch history rolls up on a schedule (Decision 14): the surfaces read the
    // affinity table, never the raw events.
    scheduler.register(ScheduledJob::new(
        "livetv-affinity",
        "kobbler",
        "livetv_affinity",
        std::time::Duration::from_secs(3600),
    ));
    scheduler.register(ScheduledJob::new(
        "livetv-retention",
        "kobbler",
        "livetv_retention",
        std::time::Duration::from_secs(6 * 3600),
    ));

    info!("Starting NATS scheduler");
    scheduler.run(shutdown_rx).await;
}

/// Subscribe to download events from Kobload.
///
/// This function handles NATS reconnections automatically - when NATS reconnects,
/// the consumer is recreated to handle the case where consumer state was lost.
pub async fn subscribe_download_events(
    nats: NatsClient,
    pool: PgPool,
    library_events: tokio::sync::broadcast::Sender<crate::LibraryEvent>,
) {
    use futures::StreamExt;

    let mut reconnect_rx = nats.subscribe_reconnect();
    let mut backoff = std::time::Duration::from_secs(1);

    let consumer_config = async_nats::jetstream::consumer::pull::Config {
        durable_name: Some("kobbler-download-events".to_string()),
        filter_subject: "events.download.>".to_string(),
        ..Default::default()
    };

    'reconnect: loop {
        // Get fresh stream reference
        let stream = match nats.jetstream().get_stream("EVENTS").await {
            Ok(s) => s,
            Err(e) => {
                error!(
                    "Failed to get EVENTS stream: {}, retrying in {:?}",
                    e, backoff
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                continue 'reconnect;
            }
        };

        let consumer = match stream
            .get_or_create_consumer("kobbler-download-events", consumer_config.clone())
            .await
        {
            Ok(c) => c,
            Err(e) => {
                error!(
                    "Failed to create download events consumer: {}, retrying in {:?}",
                    e, backoff
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                continue 'reconnect;
            }
        };

        let mut messages = match consumer.messages().await {
            Ok(m) => m,
            Err(e) => {
                error!(
                    "Failed to get messages stream: {}, retrying in {:?}",
                    e, backoff
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                continue 'reconnect;
            }
        };

        info!("Subscribed to download events from Kobload");
        backoff = std::time::Duration::from_secs(1); // Reset backoff on successful setup

        loop {
            tokio::select! {
                // Proactive: Reconnect signal from NatsClient
                _ = reconnect_rx.recv() => {
                    info!("Download events consumer received reconnect signal, recreating consumer");
                    continue 'reconnect;
                }

                msg_result = messages.next() => {
                    match msg_result {
                        Some(Ok(msg)) => {
                            let event: nats_jobs::Event = match serde_json::from_slice(&msg.payload) {
                                Ok(e) => e,
                                Err(e) => {
                                    error!("Failed to deserialize download event: {}", e);
                                    let _ = msg.ack().await;
                                    continue;
                                }
                            };

                            info!(
                                event_type = %event.event_type,
                                event_id = %event.id,
                                "Received download event"
                            );

                            // Handle different event types
                            match event.event_type.as_str() {
                                "download.completed" => {
                                    if let Err(e) =
                                        handle_download_completed(&pool, &library_events, event.payload).await
                                    {
                                        error!("Failed to handle download completed event: {}", e);
                                    }
                                }
                                "download.watch_matched" => {
                                    if let Err(e) =
                                        handle_watch_matched(&pool, &library_events, event.payload).await
                                    {
                                        error!("Failed to handle watch matched event: {}", e);
                                    }
                                }
                                _ => {
                                    tracing::warn!("Unknown download event type: {}", event.event_type);
                                }
                            }

                            let _ = msg.ack().await;
                        }
                        Some(Err(e)) => {
                            // Fallback: Handle stream errors by reconnecting
                            error!(
                                "Error receiving download event: {}, reconnecting",
                                e
                            );
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                            continue 'reconnect;
                        }
                        None => {
                            // Stream ended unexpectedly
                            error!("Download events stream ended, reconnecting");
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                            continue 'reconnect;
                        }
                    }
                }
            }
        }
    }
}

/// Handle download completion events from NATS.
///
/// This is intentionally status-only: it updates the wanted_attempt status for
/// real-time UI visibility, but does NOT trigger file ingest. Full ingest
/// (file download, metadata extraction, library import) happens via the HTTP
/// callback path at POST /api/v1/ingest/{token} — see routes/ingest/kobload.rs.
/// Kobload always sends both NATS events and HTTP callbacks; clients deduplicate.
async fn handle_download_completed(
    pool: &PgPool,
    library_events: &tokio::sync::broadcast::Sender<crate::LibraryEvent>,
    payload: serde_json::Value,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let event: nats_jobs::DownloadCompletedEvent = serde_json::from_value(payload)?;

    info!(
        download_id = %event.download_id,
        status = %event.status,
        files_count = event.files.len(),
        "Processing download completed event"
    );

    let download_id = event.download_id.to_string();

    // Handle recoverable errors from Kobload (e.g., can't access download files)
    let (effective_status, message) = if let Some(ref recoverable_err) = event.recoverable_error {
        info!(
            download_id = %event.download_id,
            error = %recoverable_err,
            "Kobload reported recoverable error - waiting for resolution"
        );
        ("kobload_error".to_string(), Some(recoverable_err.clone()))
    } else {
        (event.status.to_lowercase(), event.error.clone())
    };

    // Store files_json and update attempt status
    let files_json = serde_json::to_value(&event.files).ok();
    let attempt_result = wanted_attempts_repo::update_status_returning(
        pool,
        &download_id,
        &effective_status,
        message.as_deref(),
        files_json,
    )
    .await?;

    let book_id = attempt_result.and_then(|(book_id, _, _)| book_id);

    // Broadcast status update event for real-time UI updates
    if let Some(book_id) = book_id {
        let _ = library_events.send(crate::LibraryEvent::wanted_status_updated(
            book_id,
            effective_status.clone(),
        ));
    }

    // If recoverable error, acknowledged - Kobload will retry
    if event.recoverable_error.is_some() {
        return Ok(());
    }

    // If not completed, just acknowledge
    if effective_status != "completed"
        && effective_status != "complete"
        && effective_status != "downloaded"
    {
        return Ok(());
    }

    // For completed downloads with files, we need to ingest
    // This creates an ingest queue item for manual review or auto-ingest
    // NOTE: Full file processing is complex and currently handled via HTTP callback
    // at POST /api/v1/ingest/{token} - see routes/ingest.rs kobload_ingest()
    // For now we just update status; full NATS-based ingest is a future enhancement

    info!(
        download_id = %event.download_id,
        files_count = event.files.len(),
        "Download completed - files available for ingest"
    );

    Ok(())
}

async fn handle_watch_matched(
    pool: &PgPool,
    library_events: &tokio::sync::broadcast::Sender<crate::LibraryEvent>,
    payload: serde_json::Value,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let event: nats_jobs::WatchMatchedEvent = serde_json::from_value(payload)?;

    info!(
        pattern_id = %event.watch_pattern_id,
        release_title = %event.release_title,
        action = %event.action,
        "Processing watch matched event"
    );

    let parsed_id = crate::wanted::book_source::entity_id(&event.source_id);

    // Only the book path currently produces wanted_attempt rows via watch match.
    // Movie/tv_episode/tv_season watch matches get their attempt rows from the
    // download.completed → kobload_ingest job path instead; for those, just drop
    // the event here without warning.
    let book_id = match parsed_id {
        Some(id) if books_repo::is_wanted_book(pool, id).await.unwrap_or(false) => Some(id),
        _ => None,
    };

    if let Some(book_id) = book_id {
        // Create or update a download attempt based on the action
        let new_status = match event.action.as_str() {
            "downloading" | "grabbed" => "downloading",
            "matched" => "queued",
            _ => "queued",
        };

        // Create an attempt record for this watch match
        let _ = wanted_attempts_repo::upsert_by_external_id(
            pool,
            wanted_attempts_repo::BookAttemptUpsert {
                book_id,
                source: "watch",
                external_id: &event.watch_pattern_id.to_string(),
                status: new_status,
                ..Default::default()
            },
        )
        .await;

        info!(
            book_id = %book_id,
            status = new_status,
            release_title = %event.release_title,
            "Updated wanted book status from watch match"
        );

        // Broadcast status update for real-time UI
        let _ = library_events.send(crate::LibraryEvent::wanted_status_updated(
            book_id,
            new_status.to_string(),
        ));
    } else if let Some(id) = parsed_id {
        // Not a wanted book — check whether it's a movie/episode/season we
        // simply don't process via the watch path. If so, drop silently.
        let is_known_non_book = movies_repo::get(pool, id).await.ok().flatten().is_some()
            || episodes_repo::get(pool, id).await.ok().flatten().is_some()
            || seasons_repo::get(pool, id).await.ok().flatten().is_some();
        if !is_known_non_book {
            tracing::warn!(
                source_system = %event.source_system,
                source_id = %event.source_id,
                "Watch match source_id not found"
            );
        }
    } else {
        tracing::warn!(
            source_system = %event.source_system,
            source_id = %event.source_id,
            "Watch match source_id not found"
        );
    }

    Ok(())
}

/// Subscribe to want response events from Kobload.
///
/// This function handles NATS reconnections automatically - when NATS reconnects,
/// the consumer is recreated to handle the case where consumer state was lost.
pub async fn subscribe_want_response_events(
    nats: NatsClient,
    pool: PgPool,
    library_events: tokio::sync::broadcast::Sender<crate::LibraryEvent>,
) {
    use futures::StreamExt;

    let mut reconnect_rx = nats.subscribe_reconnect();
    let mut backoff = std::time::Duration::from_secs(1);

    let consumer_config = async_nats::jetstream::consumer::pull::Config {
        durable_name: Some("kobbler-want-response".to_string()),
        filter_subject: "events.want.response.kobbler".to_string(),
        ..Default::default()
    };

    'reconnect: loop {
        // Get fresh stream reference
        let stream = match nats.jetstream().get_stream("EVENTS").await {
            Ok(s) => s,
            Err(e) => {
                error!(
                    "Failed to get EVENTS stream for want responses: {}, retrying in {:?}",
                    e, backoff
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                continue 'reconnect;
            }
        };

        let consumer = match stream
            .get_or_create_consumer("kobbler-want-response", consumer_config.clone())
            .await
        {
            Ok(c) => c,
            Err(e) => {
                error!(
                    "Failed to create want response consumer: {}, retrying in {:?}",
                    e, backoff
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                continue 'reconnect;
            }
        };

        let mut messages = match consumer.messages().await {
            Ok(m) => m,
            Err(e) => {
                error!(
                    "Failed to get want response messages stream: {}, retrying in {:?}",
                    e, backoff
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                continue 'reconnect;
            }
        };

        info!("Subscribed to want response events from Kobload");
        backoff = std::time::Duration::from_secs(1); // Reset backoff on successful setup

        loop {
            tokio::select! {
                // Proactive: Reconnect signal from NatsClient
                _ = reconnect_rx.recv() => {
                    info!("Want response consumer received reconnect signal, recreating consumer");
                    continue 'reconnect;
                }

                msg_result = messages.next() => {
                    match msg_result {
                        Some(Ok(msg)) => {
                            let event: nats_jobs::Event = match serde_json::from_slice(&msg.payload) {
                                Ok(e) => e,
                                Err(e) => {
                                    error!("Failed to deserialize want response event: {}", e);
                                    let _ = msg.ack().await;
                                    continue;
                                }
                            };

                            info!(
                                event_type = %event.event_type,
                                event_id = %event.id,
                                "Received want response event"
                            );

                            if let Err(e) =
                                handle_want_response(&pool, &library_events, event.payload).await
                            {
                                error!("Failed to handle want response event: {}", e);
                            }

                            let _ = msg.ack().await;
                        }
                        Some(Err(e)) => {
                            // Fallback: Handle stream errors by reconnecting
                            error!(
                                "Error receiving want response event: {}, reconnecting",
                                e
                            );
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                            continue 'reconnect;
                        }
                        None => {
                            // Stream ended unexpectedly
                            error!("Want response events stream ended, reconnecting");
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                            continue 'reconnect;
                        }
                    }
                }
            }
        }
    }
}

async fn handle_want_response(
    pool: &PgPool,
    library_events: &tokio::sync::broadcast::Sender<crate::LibraryEvent>,
    payload: serde_json::Value,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let response: nats_jobs::WantResponse = serde_json::from_value(payload)?;

    let source_system = response.source_system.as_deref().unwrap_or("");
    let source_id = response.source_id.as_deref().unwrap_or("");

    info!(
        source_system = %source_system,
        source_id = %source_id,
        watch_pattern_id = %response.watch_pattern_id,
        status = %response.status,
        download_id = ?response.download_id,
        results_found = response.results_found,
        best_score = response.best_score,
        "Processing want response event"
    );

    let Some(source_uuid) = crate::wanted::book_source::entity_id(source_id) else {
        tracing::warn!(source_id = %source_id, "Invalid UUID in want response source_id");
        return Ok(());
    };

    // Determine source_type from metadata (echoed from WantRequest)
    let source_type = response
        .metadata
        .as_ref()
        .and_then(|m| m.get("source_type"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Echoed back from WantRequest.metadata, so a download that a person asked for can be
    // attributed to them minutes later when kobload finally answers.
    let requester = response
        .metadata
        .as_ref()
        .and_then(|m| m.get("user_id"))
        .and_then(|v| v.as_str())
        .and_then(|s| uuid::Uuid::parse_str(s).ok());

    match source_type {
        "movie" => {
            handle_want_response_movie(pool, library_events, &response, source_uuid, requester)
                .await
        }
        "tv_episode" | "sport_event" => {
            handle_want_response_episode(pool, library_events, &response, source_uuid, requester)
                .await
        }
        "tv_season" => {
            handle_want_response_season(pool, library_events, &response, source_uuid, requester)
                .await
        }
        _ => {
            // Default: treat as book (backwards compatibility)
            handle_want_response_book(pool, library_events, &response, source_uuid, requester).await
        }
    }
}

async fn handle_want_response_book(
    pool: &PgPool,
    library_events: &tokio::sync::broadcast::Sender<crate::LibraryEvent>,
    response: &nats_jobs::WantResponse,
    book_id: uuid::Uuid,
    requester: Option<uuid::Uuid>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !books_repo::is_wanted_book(pool, book_id)
        .await
        .unwrap_or(false)
    {
        tracing::warn!(book_id = %book_id, "Want response received for non-wanted book");
        return Ok(());
    }

    let (status, external_id) = match response.status {
        nats_jobs::WantStatus::Downloading => {
            if let Err(e) = books_repo::set_want_status(pool, book_id, "downloading").await {
                tracing::warn!(book_id = %book_id, error = %e, "Failed to set want_status to downloading");
            }
            let _ = library_events.send(crate::LibraryEvent::library_updated(Some(book_id)));
            if let Some(download_id) = response.download_id {
                ("downloading", download_id.to_string())
            } else {
                ("downloading", response.watch_pattern_id.to_string())
            }
        }
        nats_jobs::WantStatus::Watching => {
            info!(book_id = %book_id, watch_pattern_id = %response.watch_pattern_id, "Book want created watch pattern");
            if let Err(e) = books_repo::set_want_status(pool, book_id, "watching").await {
                tracing::warn!(book_id = %book_id, error = %e, "Failed to set want_status to watching");
            }
            let _ = library_events.send(crate::LibraryEvent::library_updated(Some(book_id)));
            let _ = library_events.send(crate::LibraryEvent::wanted_status_updated(
                book_id,
                "watching".to_string(),
            ));
            return Ok(());
        }
        nats_jobs::WantStatus::Failed => {
            tracing::warn!(book_id = %book_id, "Book want request failed");
            if let Err(e) = books_repo::set_want_status(pool, book_id, "search_failed").await {
                tracing::warn!(book_id = %book_id, error = %e, "Failed to set want_status to search_failed");
            }
            let _ = library_events.send(crate::LibraryEvent::library_updated(Some(book_id)));
            let _ = library_events.send(crate::LibraryEvent::wanted_status_updated(
                book_id,
                "search_failed".to_string(),
            ));
            return Ok(());
        }
    };

    let _ = wanted_attempts_repo::upsert_by_external_id(
        pool,
        wanted_attempts_repo::BookAttemptUpsert {
            book_id,
            source: "want",
            external_id: &external_id,
            status,
            user_id: requester,
            ..Default::default()
        },
    )
    .await;
    info!(book_id = %book_id, status = status, external_id = %external_id, "Created wanted attempt from book want response");
    let _ = library_events.send(crate::LibraryEvent::wanted_status_updated(
        book_id,
        status.to_string(),
    ));
    Ok(())
}

async fn handle_want_response_movie(
    pool: &PgPool,
    library_events: &tokio::sync::broadcast::Sender<crate::LibraryEvent>,
    response: &nats_jobs::WantResponse,
    movie_id: uuid::Uuid,
    requester: Option<uuid::Uuid>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let want_status_str = match response.status {
        nats_jobs::WantStatus::Downloading => "downloading",
        nats_jobs::WantStatus::Watching => "watching",
        nats_jobs::WantStatus::Failed => "search_failed",
    };

    if let Err(e) = movies_repo::set_want_status(pool, movie_id, want_status_str).await {
        tracing::warn!(movie_id = %movie_id, error = %e, "Failed to set movie want_status to {}", want_status_str);
    }
    let _ = library_events.send(crate::LibraryEvent::movie_updated(movie_id));

    if response.status == nats_jobs::WantStatus::Downloading {
        let external_id = response
            .download_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| response.watch_pattern_id.to_string());

        let _ = wanted_attempts_repo::create(
            pool,
            wanted_attempts_repo::WantedAttemptInput {
                movie_id: Some(movie_id),
                source: "want".to_string(),
                external_id: Some(external_id.clone()),
                status: "downloading".to_string(),
                user_id: requester,
                ..Default::default()
            },
        )
        .await;
        info!(movie_id = %movie_id, external_id = %external_id, "Created wanted attempt from movie want response");
    } else {
        info!(movie_id = %movie_id, status = want_status_str, "Movie want response processed");
    }

    Ok(())
}

async fn handle_want_response_episode(
    pool: &PgPool,
    library_events: &tokio::sync::broadcast::Sender<crate::LibraryEvent>,
    response: &nats_jobs::WantResponse,
    episode_id: uuid::Uuid,
    requester: Option<uuid::Uuid>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let want_status_str = match response.status {
        nats_jobs::WantStatus::Downloading => "downloading",
        nats_jobs::WantStatus::Watching => "watching",
        nats_jobs::WantStatus::Failed => "search_failed",
    };

    if let Err(e) = episodes_repo::set_want_status(pool, episode_id, want_status_str).await {
        tracing::warn!(episode_id = %episode_id, error = %e, "Failed to set episode want_status to {}", want_status_str);
    }

    // Extract show_id from metadata for targeted SSE event
    let show_id = response
        .metadata
        .as_ref()
        .and_then(|m| m.get("show_id"))
        .and_then(|v| v.as_str())
        .and_then(|s| uuid::Uuid::parse_str(s).ok());
    if let Some(show_id) = show_id {
        let _ = library_events.send(crate::LibraryEvent::tv_show_updated(show_id));
    }

    if response.status == nats_jobs::WantStatus::Downloading {
        let external_id = response
            .download_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| response.watch_pattern_id.to_string());

        let _ = wanted_attempts_repo::create(
            pool,
            wanted_attempts_repo::WantedAttemptInput {
                episode_id: Some(episode_id),
                source: "want".to_string(),
                external_id: Some(external_id.clone()),
                status: "downloading".to_string(),
                user_id: requester,
                ..Default::default()
            },
        )
        .await;
        info!(episode_id = %episode_id, external_id = %external_id, "Created wanted attempt from episode want response");
    } else {
        info!(episode_id = %episode_id, status = want_status_str, "Episode want response processed");
    }

    Ok(())
}

async fn handle_want_response_season(
    pool: &PgPool,
    library_events: &tokio::sync::broadcast::Sender<crate::LibraryEvent>,
    response: &nats_jobs::WantResponse,
    _season_id: uuid::Uuid,
    requester: Option<uuid::Uuid>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // For season-level want responses, the metadata contains an episode_map
    // mapping episode identifiers to episode UUIDs. Update all affected episodes.
    let episode_map = response
        .metadata
        .as_ref()
        .and_then(|m| m.get("episode_map"))
        .cloned();

    if let Some(episode_map) = episode_map {
        // Parse episode_map: { "episode_identifier": "episode_uuid", ... }
        if let Some(map) = episode_map.as_object() {
            match response.status {
                nats_jobs::WantStatus::Downloading => {
                    // If episodes_downloaded is present, only mark those as "downloading"
                    // and mark the rest as "searching" (still being watched via RSS).
                    if let Some(ref downloaded) = response.episodes_downloaded
                        && !downloaded.is_empty()
                    {
                        let mut downloading_ids = Vec::new();
                        let mut searching_ids = Vec::new();

                        for (ep_ident, ep_uuid_val) in map {
                            if let Some(uuid) = ep_uuid_val
                                .as_str()
                                .and_then(|s| uuid::Uuid::parse_str(s).ok())
                            {
                                if downloaded.contains(ep_ident) {
                                    downloading_ids.push(uuid);
                                } else {
                                    searching_ids.push(uuid);
                                }
                            }
                        }

                        if !downloading_ids.is_empty()
                            && let Err(e) = episodes_repo::set_want_status_batch(
                                pool,
                                &downloading_ids,
                                "downloading",
                            )
                            .await
                        {
                            tracing::warn!(error = %e, "Failed to set downloading episodes");
                        }
                        if !searching_ids.is_empty()
                            && let Err(e) = episodes_repo::set_want_status_batch(
                                pool,
                                &searching_ids,
                                "searching",
                            )
                            .await
                        {
                            tracing::warn!(error = %e, "Failed to set searching episodes");
                        }

                        // Create wanted_attempts for downloading episodes (for idempotency)
                        let external_id = response
                            .download_id
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| response.watch_pattern_id.to_string());
                        for &ep_id in &downloading_ids {
                            let _ = wanted_attempts_repo::create(
                                pool,
                                wanted_attempts_repo::WantedAttemptInput {
                                    episode_id: Some(ep_id),
                                    source: "want".to_string(),
                                    external_id: Some(external_id.clone()),
                                    status: "downloading".to_string(),
                                    user_id: requester,
                                    ..Default::default()
                                },
                            )
                            .await;
                        }
                        if !downloading_ids.is_empty() {
                            info!(
                                external_id = %external_id,
                                count = downloading_ids.len(),
                                "Created wanted attempts for season download episodes"
                            );
                        }
                    } else {
                        // No episode detail — mark all as downloading (legacy behavior)
                        let episode_ids: Vec<uuid::Uuid> = map
                            .values()
                            .filter_map(|v| v.as_str().and_then(|s| uuid::Uuid::parse_str(s).ok()))
                            .collect();
                        if !episode_ids.is_empty()
                            && let Err(e) = episodes_repo::set_want_status_batch(
                                pool,
                                &episode_ids,
                                "downloading",
                            )
                            .await
                        {
                            tracing::warn!(error = %e, "Failed to batch update episodes to downloading");
                        }

                        // Create wanted_attempts for all episodes (for idempotency)
                        let external_id = response
                            .download_id
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| response.watch_pattern_id.to_string());
                        for &ep_id in &episode_ids {
                            let _ = wanted_attempts_repo::create(
                                pool,
                                wanted_attempts_repo::WantedAttemptInput {
                                    episode_id: Some(ep_id),
                                    source: "want".to_string(),
                                    external_id: Some(external_id.clone()),
                                    status: "downloading".to_string(),
                                    user_id: requester,
                                    ..Default::default()
                                },
                            )
                            .await;
                        }
                        if !episode_ids.is_empty() {
                            info!(
                                external_id = %external_id,
                                count = episode_ids.len(),
                                "Created wanted attempts for season download episodes (legacy)"
                            );
                        }
                    }
                }
                nats_jobs::WantStatus::Watching | nats_jobs::WantStatus::Failed => {
                    let status_str = match response.status {
                        nats_jobs::WantStatus::Watching => "watching",
                        nats_jobs::WantStatus::Failed => "search_failed",
                        _ => unreachable!(),
                    };
                    let episode_ids: Vec<uuid::Uuid> = map
                        .values()
                        .filter_map(|v| v.as_str().and_then(|s| uuid::Uuid::parse_str(s).ok()))
                        .collect();
                    if !episode_ids.is_empty()
                        && let Err(e) =
                            episodes_repo::set_want_status_batch(pool, &episode_ids, status_str)
                                .await
                    {
                        tracing::warn!(error = %e, "Failed to batch update episode want_status");
                    }
                }
            }
        }
    }

    // Extract show_id from metadata for targeted SSE event
    let show_id = response
        .metadata
        .as_ref()
        .and_then(|m| m.get("show_id"))
        .and_then(|v| v.as_str())
        .and_then(|s| uuid::Uuid::parse_str(s).ok());
    if let Some(show_id) = show_id {
        let _ = library_events.send(crate::LibraryEvent::tv_show_updated(show_id));
    }
    info!(season_id = %_season_id, status = %response.status, "Season want response processed");
    Ok(())
}

/// Subscribe to ingest-ready events from Kobload.
///
/// When Kobload finishes metadata extraction, it publishes `events.ingest.ready`
/// containing the callback payload. This subscriber enqueues a `kobload_ingest`
/// job so kobbler can process the files asynchronously.
pub async fn subscribe_ingest_ready_events(
    nats: NatsClient,
    pool: PgPool,
    nats_publisher: nats_jobs::Publisher,
) {
    use futures::StreamExt;

    let mut reconnect_rx = nats.subscribe_reconnect();
    let mut backoff = std::time::Duration::from_secs(1);

    let consumer_config = async_nats::jetstream::consumer::pull::Config {
        durable_name: Some("kobbler-ingest-ready".to_string()),
        filter_subject: "events.ingest.ready".to_string(),
        ..Default::default()
    };

    'reconnect: loop {
        let stream = match nats.jetstream().get_stream("EVENTS").await {
            Ok(s) => s,
            Err(e) => {
                error!(
                    "Failed to get EVENTS stream for ingest-ready: {}, retrying in {:?}",
                    e, backoff
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                continue 'reconnect;
            }
        };

        let consumer = match stream
            .get_or_create_consumer("kobbler-ingest-ready", consumer_config.clone())
            .await
        {
            Ok(c) => c,
            Err(e) => {
                error!(
                    "Failed to create ingest-ready consumer: {}, retrying in {:?}",
                    e, backoff
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                continue 'reconnect;
            }
        };

        let mut messages = match consumer.messages().await {
            Ok(m) => m,
            Err(e) => {
                error!(
                    "Failed to get ingest-ready messages stream: {}, retrying in {:?}",
                    e, backoff
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                continue 'reconnect;
            }
        };

        info!("Subscribed to ingest-ready events from Kobload");
        backoff = std::time::Duration::from_secs(1);

        loop {
            tokio::select! {
                _ = reconnect_rx.recv() => {
                    info!("Ingest-ready consumer received reconnect signal, recreating consumer");
                    continue 'reconnect;
                }

                msg_result = messages.next() => {
                    match msg_result {
                        Some(Ok(msg)) => {
                            let event: nats_jobs::Event = match serde_json::from_slice(&msg.payload) {
                                Ok(e) => e,
                                Err(e) => {
                                    error!("Failed to deserialize ingest-ready event: {}", e);
                                    let _ = msg.ack().await;
                                    continue;
                                }
                            };

                            // Check idempotency key before enqueuing to prevent duplicate jobs
                            // (defense-in-depth against event redelivery)
                            let idempotency_key = event.payload.get("idempotency_key")
                                .and_then(|v| v.as_str())
                                .and_then(|s| uuid::Uuid::parse_str(s).ok());

                            if let Some(key) = idempotency_key
                                && let Ok(Some(attempt_id)) = crate::repositories::wanted_attempts::check_idempotency_key(&pool, key).await
                            {
                                info!(
                                    event_id = %event.id,
                                    idempotency_key = %key,
                                    attempt_id = %attempt_id,
                                    "Ingest-ready event already processed (idempotency key found), skipping"
                                );
                                if let Err(e) = msg.ack().await {
                                    error!("Failed to ack duplicate ingest-ready event: {}", e);
                                }
                                continue;
                            }

                            info!(
                                event_id = %event.id,
                                "Received ingest-ready event, enqueuing kobload_ingest job"
                            );

                            // Enqueue a kobload_ingest job with the callback payload and NATS response mode
                            let job_payload = serde_json::json!({
                                "request": event.payload,
                                "response_mode": "nats",
                            });

                            let download_entity_id = event.payload.get("download_id")
                                    .and_then(|v| v.as_str())
                                    .and_then(|s| uuid::Uuid::parse_str(s).ok());
                            let display = if let Some(eid) = download_entity_id {
                                crate::jobs::fetch_job_display(&pool, "download", eid).await
                            } else {
                                None
                            };
                            let context = nats_jobs::JobContext {
                                description: "Processing Kobload ingest via NATS".to_string(),
                                entity_type: Some("download".to_string()),
                                entity_id: download_entity_id,
                                display,
                            };

                            match crate::jobs::enqueue(
                                &pool,
                                &nats_publisher,
                                "kobload_ingest",
                                job_payload,
                                context,
                            ).await {
                                Ok(job_id) => {
                                    info!(job_id = %job_id, "Enqueued kobload_ingest job");
                                }
                                Err(e) => {
                                    error!("Failed to enqueue kobload_ingest job: {}", e);
                                }
                            }

                            if let Err(e) = msg.ack().await {
                                error!("Failed to ack ingest-ready event: {}", e);
                            }
                        }
                        Some(Err(e)) => {
                            error!(
                                "Error receiving ingest-ready event: {}, reconnecting",
                                e
                            );
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                            continue 'reconnect;
                        }
                        None => {
                            error!("Ingest-ready events stream ended, reconnecting");
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                            continue 'reconnect;
                        }
                    }
                }
            }
        }
    }
}

/// Subscribe to watch pattern deleted events from Kobload.
///
/// When a watch pattern is deleted in Kobload, reset the corresponding entity's
/// want_status back to "none" so it no longer appears as downloading/watching/searching.
pub async fn subscribe_watch_pattern_deleted_events(
    nats: NatsClient,
    pool: PgPool,
    library_events: tokio::sync::broadcast::Sender<crate::LibraryEvent>,
) {
    use futures::StreamExt;

    let mut reconnect_rx = nats.subscribe_reconnect();
    let mut backoff = std::time::Duration::from_secs(1);

    let consumer_config = async_nats::jetstream::consumer::pull::Config {
        durable_name: Some("kobbler-watch-pattern-deleted".to_string()),
        filter_subject: "events.watch_pattern.>".to_string(),
        ..Default::default()
    };

    'reconnect: loop {
        let stream = match nats.jetstream().get_stream("EVENTS").await {
            Ok(s) => s,
            Err(e) => {
                error!(
                    "Failed to get EVENTS stream for watch pattern events: {}, retrying in {:?}",
                    e, backoff
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                continue 'reconnect;
            }
        };

        let consumer = match stream
            .get_or_create_consumer("kobbler-watch-pattern-deleted", consumer_config.clone())
            .await
        {
            Ok(c) => c,
            Err(e) => {
                error!(
                    "Failed to create watch pattern events consumer: {}, retrying in {:?}",
                    e, backoff
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                continue 'reconnect;
            }
        };

        let mut messages = match consumer.messages().await {
            Ok(m) => m,
            Err(e) => {
                error!(
                    "Failed to get watch pattern messages stream: {}, retrying in {:?}",
                    e, backoff
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                continue 'reconnect;
            }
        };

        info!("Subscribed to watch pattern events from Kobload");
        backoff = std::time::Duration::from_secs(1);

        loop {
            tokio::select! {
                _ = reconnect_rx.recv() => {
                    info!("Watch pattern events consumer received reconnect signal, recreating consumer");
                    continue 'reconnect;
                }

                msg_result = messages.next() => {
                    match msg_result {
                        Some(Ok(msg)) => {
                            let event: nats_jobs::Event = match serde_json::from_slice(&msg.payload) {
                                Ok(e) => e,
                                Err(e) => {
                                    error!("Failed to deserialize watch pattern event: {}", e);
                                    let _ = msg.ack().await;
                                    continue;
                                }
                            };

                            if event.event_type == "watch_pattern.deleted" {
                                if let Err(e) =
                                    handle_watch_pattern_deleted(&pool, &library_events, event.payload).await
                                {
                                    error!("Failed to handle watch pattern deleted event: {}", e);
                                }
                            } else {
                                tracing::warn!("Unknown watch pattern event type: {}", event.event_type);
                            }

                            let _ = msg.ack().await;
                        }
                        Some(Err(e)) => {
                            error!(
                                "Error receiving watch pattern event: {}, reconnecting",
                                e
                            );
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                            continue 'reconnect;
                        }
                        None => {
                            error!("Watch pattern events stream ended, reconnecting");
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                            continue 'reconnect;
                        }
                    }
                }
            }
        }
    }
}

async fn handle_watch_pattern_deleted(
    pool: &PgPool,
    library_events: &tokio::sync::broadcast::Sender<crate::LibraryEvent>,
    payload: serde_json::Value,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let event: nats_jobs::WatchPatternDeletedEvent = serde_json::from_value(payload)?;

    info!(
        source_system = %event.source_system,
        source_id = %event.source_id,
        "Processing watch pattern deleted event"
    );

    let Some(source_uuid) = crate::wanted::book_source::entity_id(&event.source_id) else {
        tracing::warn!(
            source_id = %event.source_id,
            "Invalid UUID in watch pattern deleted source_id"
        );
        return Ok(());
    };

    let source_type = event
        .metadata
        .as_ref()
        .and_then(|m| m.get("source_type"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match source_type {
        "movie" => {
            if let Err(e) = movies_repo::set_want_status(pool, source_uuid, "none").await {
                tracing::warn!(movie_id = %source_uuid, error = %e, "Failed to reset movie want_status");
            }
            let _ = library_events.send(crate::LibraryEvent::library_updated(Some(source_uuid)));
        }
        "tv_episode" | "sport_event" => {
            if let Err(e) = episodes_repo::set_want_status(pool, source_uuid, "none").await {
                tracing::warn!(episode_id = %source_uuid, error = %e, "Failed to reset episode want_status");
            }
            let _ = library_events.send(crate::LibraryEvent::library_updated(Some(source_uuid)));
        }
        "tv_season" => {
            // For seasons, the episode_map in metadata maps identifiers to episode UUIDs
            if let Some(episode_map) = event
                .metadata
                .as_ref()
                .and_then(|m| m.get("episode_map"))
                .and_then(|v| v.as_object())
            {
                let episode_ids: Vec<uuid::Uuid> = episode_map
                    .values()
                    .filter_map(|v| v.as_str().and_then(|s| uuid::Uuid::parse_str(s).ok()))
                    .collect();

                if !episode_ids.is_empty()
                    && let Err(e) =
                        episodes_repo::set_want_status_batch(pool, &episode_ids, "none").await
                {
                    tracing::warn!(error = %e, "Failed to reset season episode want_statuses");
                }
            }
            let _ = library_events.send(crate::LibraryEvent::library_updated(Some(source_uuid)));
        }
        _ => {
            // Default: treat as book
            if let Ok(Some(book)) = books_repo::get_wanted_metadata(pool, source_uuid).await
                && !crate::wanted::book_source::is_current_source(
                    source_uuid,
                    &book.want_format,
                    &event.source_id,
                )
            {
                info!(
                    source_id = %event.source_id,
                    "Deleted pattern was under a key the book no longer uses; keeping it wanted"
                );
                return Ok(());
            }
            if let Err(e) = books_repo::set_want_status(pool, source_uuid, "none").await {
                tracing::warn!(book_id = %source_uuid, error = %e, "Failed to reset book want_status");
            }
            let _ = library_events.send(crate::LibraryEvent::library_updated(Some(source_uuid)));
        }
    }

    info!(
        source_system = %event.source_system,
        source_id = %event.source_id,
        source_type = source_type,
        "Reset entity want_status after watch pattern deletion"
    );

    Ok(())
}
