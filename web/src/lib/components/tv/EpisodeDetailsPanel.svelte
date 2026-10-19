<script lang="ts">
  import MetaLine from '$lib/components/shared/MetaLine.svelte';
  import { formatCalendarDate } from '$lib/utils/format';
  import { onMount } from 'svelte';
  import { on, off } from '$lib/services/eventstream.svelte';
  import type { TvEpisode, TvEpisodeFile, TvWatchState } from '$lib/types';
  import {
    getEpisodeFiles,
    getEpisodeWatchState,
    updateEpisodeWatchState,
    deleteEpisodeFile,
    rejectDownload,
    replaceFile
  } from '$lib/api/client';
  import { isActivelyWanted } from '$lib/utils/want-status';
  import { usePlayback } from '$lib/composables/usePlayback.svelte';
  import { useStreamPlayback } from '$lib/composables/useStreamPlayback.svelte';
  import VideoPlayer from '$lib/components/VideoPlayer/VideoPlayer.svelte';
  import PlaybackArea from '$lib/components/shared/PlaybackArea.svelte';
  import WatchToggle from '$lib/components/shared/WatchToggle.svelte';
  import FilesList from '$lib/components/shared/FilesList.svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import StreamOptionsModal from '$lib/components/StreamOptionsModal.svelte';
  import StreamCachingOverlay from '$lib/components/StreamCachingOverlay.svelte';
  import { getStatsState } from '$lib/stores/stats.svelte';

  interface Props {
    episode: TvEpisode;
    showTitle?: string;
    seasonTitle?: string;
    onWant?: () => void;
    onUnwant?: () => void;
    onSearchReleases?: () => void;
    onFileChanged?: () => void;
    onMoveFile?: (fileId: string) => void;
    onEdit?: () => void;
    onDelete?: () => void;
  }

  let {
    episode,
    showTitle,
    seasonTitle,
    onWant,
    onUnwant,
    onSearchReleases,
    onFileChanged,
    onMoveFile,
    onEdit,
    onDelete
  }: Props = $props();

  const statsState = getStatsState();
  const stats = $derived(statsState.stats);

  let files = $state<TvEpisodeFile[]>([]);
  let watchState = $state<TvWatchState | null>(null);
  let loadingWatch = $state(false);
  let deletingFileId = $state<string | null>(null);
  let rejecting = $state(false);

  const playback = usePlayback({
    mediaType: 'episode',
    refreshWatchState: async () => {
      watchState = await getEpisodeWatchState(episode.id);
    }
  });

  const stream = useStreamPlayback({
    mediaType: 'episode',
    getMediaId: () => episode.id,
    getSearchParams: () => ({ episode_id: episode.id }),
    getResumePosition: () => watchState?.position_seconds ?? undefined,
    getEstimatedDuration: () => (episode.runtime ? episode.runtime * 60 : undefined),
    playback,
    onMessage: showMessage
  });

  // Debrid cache warming finishes after stream search has already returned, so
  // fold its results into the option list: mark verified-cached and promote,
  // otherwise auto-play keeps walking options warming has proven blocked.
  $effect(() => {
    const cb = on(
      'stream_warm_ready',
      (data: { info_hash: string; file_id?: number | null; debrid_provider_id: string }) => {
        stream.markCached(data.info_hash, data.file_id ?? null, data.debrid_provider_id);
      }
    );
    return () => off('stream_warm_ready', cb);
  });

  async function loadDetails() {
    loadingWatch = true;
    try {
      const [f, ws] = await Promise.all([
        getEpisodeFiles(episode.id),
        getEpisodeWatchState(episode.id)
      ]);
      files = f;
      watchState = ws;
    } catch (e) {
      console.error('Failed to load episode details:', e);
    } finally {
      loadingWatch = false;
    }
  }

  /** Pre-warm the right thing: playback session for owned content, stream search for unowned. */
  function preWarm() {
    if (episode.is_in_library && files.length > 0) {
      // Must match the resume logic in the Play button — if watched, start from beginning
      const resumePos = watchState?.watched
        ? undefined
        : (watchState?.position_seconds ?? undefined);
      playback.preWarm(files[0].id, resumePos);
      return;
    }
    stream.preWarmStream();
  }

  onMount(() => {
    loadDetails().then(() => preWarm());
    return () => {
      playback.teardownPreWarm();
      stream.teardown();
    };
  });

  let prevEpisodeId: string | undefined;
  $effect(() => {
    const id = episode.id;
    if (prevEpisodeId !== undefined && prevEpisodeId !== id) {
      stream.reset();
      playback.teardownPreWarm();
      loadDetails().then(() => preWarm());
    }
    prevEpisodeId = id;
  });

  async function toggleWatched() {
    loadingWatch = true;
    try {
      const newWatched = !watchState?.watched;
      const result = await updateEpisodeWatchState(episode.id, {
        watched: newWatched,
        watched_at: newWatched ? new Date().toISOString() : null
      });
      watchState = {
        id: result.id,
        episode_id: episode.id,
        user_id: '',
        watched: newWatched,
        watched_at: newWatched ? new Date().toISOString() : null,
        position_seconds: watchState?.position_seconds ?? null,
        rating: watchState?.rating ?? null,
        created_at: watchState?.created_at ?? new Date().toISOString(),
        updated_at: new Date().toISOString()
      };
      showMessage(newWatched ? 'Marked as watched' : 'Marked as unwatched');
    } catch {
      showMessage('Failed to update watch state');
    } finally {
      loadingWatch = false;
    }
  }

  async function handleDeleteFile(fileId: string) {
    if (!confirm('Delete this file? This cannot be undone.')) return;
    deletingFileId = fileId;
    try {
      await deleteEpisodeFile(episode.id, fileId);
      files = files.filter((f) => f.id !== fileId);
      showMessage('File deleted');
      onFileChanged?.();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Delete failed');
    } finally {
      deletingFileId = null;
    }
  }

  async function handleReplace(fileId: string) {
    if (!confirm('Replace this file? The current file will be deleted and a new search started.'))
      return;
    try {
      await replaceFile('tv_episode', episode.id, fileId);
      showMessage('Replacement search started');
      await loadDetails();
      onFileChanged?.();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Replace failed');
    }
  }

  async function handleRejectAndResearch() {
    rejecting = true;
    try {
      await rejectDownload('tv_episode', episode.id);
      showMessage('Download rejected — searching for new release');
      onSearchReleases?.();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Reject failed');
    } finally {
      rejecting = false;
    }
  }
</script>

<div class="episode-detail">
  {#if episode.still_url}
    <div class="still-section">
      <img class="episode-still" src={episode.still_url} alt="" />
    </div>
  {/if}

  <div class="header-section">
    {#if showTitle || seasonTitle}
      <div class="breadcrumb">
        <MetaLine items={[showTitle, seasonTitle]} />
      </div>
    {/if}
    <h2 class="episode-title">
      {episode.title || `Episode ${episode.episode_sort_order}`}
    </h2>
    <div class="panel-meta-row">
      <span class="panel-meta-item">Episode {episode.episode_sort_order}</span>
      {#if episode.air_date}
        <span class="panel-meta-item">{formatCalendarDate(episode.air_date, 'short')}</span>
      {/if}
      {#if episode.runtime}
        <span class="panel-meta-item">{episode.runtime}m</span>
      {/if}
    </div>
  </div>

  {#if episode.description}
    <div class="panel-section">
      <h4 class="panel-section-title">Overview</h4>
      <p class="panel-overview">{episode.description}</p>
    </div>
  {/if}

  <div class="panel-section">
    <h4 class="panel-section-title">Watch Status</h4>
    <div class="watch-status-row">
      <WatchToggle
        watched={watchState?.watched ?? false}
        loading={loadingWatch}
        size="small"
        onToggle={toggleWatched}
      />
      {#if isActivelyWanted(episode.want_status)}
        <button class="panel-pill active" type="button" onclick={onUnwant}>
          <span class="material-icons-outlined">bookmark</span>
          Wanted
        </button>
      {:else if !episode.is_in_library}
        <button class="panel-pill" type="button" onclick={onWant}>
          <span class="material-icons-outlined">bookmark_add</span>
          Want
        </button>
      {/if}
    </div>
  </div>

  {#if files.length > 0}
    <div class="panel-section">
      <PlaybackArea
        positionSeconds={watchState?.position_seconds ?? null}
        totalSeconds={episode.runtime ? episode.runtime * 60 : null}
        watched={watchState?.watched ?? false}
        loading={playback.startingFileId !== null}
        variant="panel"
        onPlay={() => {
          if (files.length > 0)
            playback.play(
              files[0].id,
              undefined,
              watchState?.watched ? undefined : (watchState?.position_seconds ?? undefined)
            );
        }}
      />
    </div>
  {:else if stats.kobload_configured}
    <div class="panel-section">
      <PlaybackArea
        mode="stream"
        positionSeconds={watchState?.position_seconds ?? null}
        totalSeconds={episode.runtime ? episode.runtime * 60 : null}
        watched={watchState?.watched ?? false}
        loading={stream.resolving || stream.streamLoading}
        variant="panel"
        onPlay={stream.autoPlay}
        onPickStream={stream.openModal}
      />
    </div>
  {/if}

  <div class="panel-section">
    <h4 class="panel-section-title">File</h4>
    {#if files.length > 0}
      <FilesList
        {files}
        noun="episode"
        {deletingFileId}
        startingPlaybackFileId={playback.startingFileId}
        onPlay={(fileId) => playback.play(fileId)}
        onDelete={handleDeleteFile}
        onMove={onMoveFile}
        onReplace={handleReplace}
        onSearchReleases={onSearchReleases ? () => onSearchReleases?.() : undefined}
        onRejectAndResearch={handleRejectAndResearch}
        {rejecting}
      />
    {:else}
      <div class="panel-file-empty">
        <span class="muted">No file</span>
        {#if onSearchReleases}
          <button
            class="panel-icon-btn"
            type="button"
            title="Search for releases"
            onclick={onSearchReleases}
          >
            <span class="material-icons-outlined">search</span>
          </button>
        {/if}
      </div>
    {/if}
  </div>

  {#if onEdit || onDelete}
    <div class="panel-actions">
      {#if onEdit}
        <button class="panel-action-btn" type="button" onclick={onEdit}>
          <span class="material-icons-outlined" style="font-size: 16px;">edit</span>
          Edit
        </button>
      {/if}
      {#if onDelete}
        <button class="panel-action-btn danger" type="button" onclick={onDelete}>
          <span class="material-icons-outlined" style="font-size: 16px;">delete</span>
          Delete
        </button>
      {/if}
    </div>
  {/if}
</div>

{#if stream.showStreamModal}
  <StreamOptionsModal
    options={stream.streamOptions}
    loading={stream.streamLoading}
    errors={stream.streamErrors}
    onClose={stream.closeModal}
    onPlay={stream.playOption}
  />
{/if}

{#if stream.isCaching && !playback.isActive}
  <StreamCachingOverlay
    progress={stream.cachingProgress ?? 0}
    error={stream.cachingError}
    onCancel={stream.closeCachingOverlay}
    onRetry={stream.retryCaching}
    onPickStream={() => {
      stream.closeCachingOverlay();
      stream.openModal();
    }}
  />
{/if}

{#if playback.isActive}
  <VideoPlayer
    sessionResponse={playback.session}
    creationParams={playback.session
      ? {
          media_type: 'episode',
          file_id: playback.playingFileId!,
          capabilities: playback.capabilities!
        }
      : undefined}
    mediaId={episode.id}
    mediaInfo={{
      title: showTitle ?? '',
      subtitle: `E${episode.episode_sort_order}${episode.title ? ` \u00b7 ${episode.title}` : ''}`,
      seasonId: episode.season_id,
      artworkUrl: episode.still_url ?? undefined
    }}
    initialPosition={watchState?.watched ? undefined : (watchState?.position_seconds ?? undefined)}
    pendingStatus={stream.pendingStatus ?? undefined}
    pendingError={stream.cachingError ?? undefined}
    onClose={() => {
      stream.abort();
      playback.close();
    }}
    onSwitchStream={stream.handleSwitchStream}
    onPickStream={() => {
      stream.closeCachingOverlay();
      stream.openModal();
    }}
    onRetryStream={stream.retryCaching}
  />
{/if}

<style>
  .episode-detail {
    display: flex;
    flex-direction: column;
    gap: 0;
  }

  .still-section {
    margin: -20px -20px 0;
  }

  .episode-still {
    width: 100%;
    height: 180px;
    object-fit: cover;
  }

  .header-section {
    padding: 16px 0 0;
  }

  .breadcrumb {
    font-size: 12px;
    color: var(--muted);
    margin-bottom: 4px;
  }

  .episode-title {
    margin: 0 0 8px;
    font-size: 20px;
    font-weight: 800;
    line-height: 1.2;
  }

  .panel-meta-row {
    margin-bottom: 8px;
    gap: 8px;
  }

  .panel-meta-item {
    font-size: 12px;
  }

  .watch-status-row {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .panel-actions {
    display: flex;
    gap: 8px;
    padding: 16px 0 0;
    border-top: 1px solid var(--ring);
  }

  .panel-action-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 14px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: transparent;
    color: var(--muted);
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
  }

  .panel-action-btn:hover {
    background: var(--bg-2);
    color: var(--ink);
    border-color: var(--ink);
  }

  .panel-action-btn.danger:hover {
    color: var(--error, #ef4444);
    border-color: var(--error, #ef4444);
  }
</style>
