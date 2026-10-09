<script lang="ts">
  import { onMount } from 'svelte';
  import type { TvEpisode, TvEpisodeFile, TvWatchState } from '$lib/types';
  import {
    getEpisodeFiles,
    getEpisodeWatchState,
    updateEpisodeWatchState,
    deleteEpisodeFile,
    rejectDownload,
    replaceFile
  } from '$lib/api/client';
  import VideoPlayer from '$lib/components/VideoPlayer/VideoPlayer.svelte';
  import { isActivelyWanted } from '$lib/utils/want-status';
  import { showMessage } from '$lib/context/app.svelte';
  import { usePlayback } from '$lib/composables/usePlayback.svelte';
  import { useStreamPlayback } from '$lib/composables/useStreamPlayback.svelte';
  import PlaybackArea from '$lib/components/shared/PlaybackArea.svelte';
  import WatchToggle from '$lib/components/shared/WatchToggle.svelte';
  import FilesList from '$lib/components/shared/FilesList.svelte';
  import StreamOptionsModal from '$lib/components/StreamOptionsModal.svelte';
  import StreamCachingOverlay from '$lib/components/StreamCachingOverlay.svelte';
  import { getStatsState } from '$lib/stores/stats.svelte';

  interface Props {
    event: TvEpisode;
    leagueTitle?: string;
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
    event,
    leagueTitle,
    seasonTitle,
    onWant,
    onUnwant,
    onSearchReleases,
    onFileChanged,
    onMoveFile,
    onEdit,
    onDelete
  }: Props = $props();

  let files = $state<TvEpisodeFile[]>([]);
  let watchState = $state<TvWatchState | null>(null);
  let loadingWatch = $state(false);
  let deletingFileId = $state<string | null>(null);
  let rejecting = $state(false);

  const playback = usePlayback({
    mediaType: 'episode',
    refreshWatchState: async () => {
      watchState = await getEpisodeWatchState(event.id);
    }
  });

  const statsState = getStatsState();
  const stats = $derived(statsState.stats);

  const stream = useStreamPlayback({
    mediaType: 'episode',
    getMediaId: () => event.id,
    getSearchParams: () => ({ episode_id: event.id }),
    getResumePosition: () =>
      watchState?.watched ? undefined : (watchState?.position_seconds ?? undefined),
    getEstimatedDuration: () => (event.runtime ? event.runtime * 60 : undefined),
    playback,
    onMessage: showMessage
  });

  async function loadDetails() {
    loadingWatch = true;
    try {
      const [f, ws] = await Promise.all([
        getEpisodeFiles(event.id),
        getEpisodeWatchState(event.id)
      ]);
      files = f;
      watchState = ws;
    } catch (e) {
      console.error('Failed to load event details:', e);
    } finally {
      loadingWatch = false;
    }
  }

  async function preWarmSession() {
    if (!event.is_in_library) return;
    if (files.length > 0) {
      playback.preWarm(files[0].id, watchState?.position_seconds ?? undefined);
    }
  }

  onMount(() => {
    loadDetails().then(() => preWarmSession());
    return () => playback.teardownPreWarm();
  });

  let prevEventId: string | undefined;
  $effect(() => {
    const id = event.id;
    if (prevEventId !== undefined && prevEventId !== id) {
      playback.teardownPreWarm();
      loadDetails().then(() => preWarmSession());
    }
    prevEventId = id;
  });

  async function toggleWatched() {
    loadingWatch = true;
    try {
      const newWatched = !watchState?.watched;
      const result = await updateEpisodeWatchState(event.id, {
        watched: newWatched,
        watched_at: newWatched ? new Date().toISOString() : null
      });
      watchState = {
        id: result.id,
        episode_id: event.id,
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
      await deleteEpisodeFile(event.id, fileId);
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
      await replaceFile('tv_episode', event.id, fileId);
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
      await rejectDownload('tv_episode', event.id);
      showMessage('Download rejected — searching for new release');
      onSearchReleases?.();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Reject failed');
    } finally {
      rejecting = false;
    }
  }
</script>

<div class="event-detail">
  {#if event.still_url}
    <div class="still-section">
      <img class="event-still" src={event.still_url} alt="" />
    </div>
  {/if}

  <div class="header-section">
    {#if leagueTitle || seasonTitle}
      <div class="breadcrumb">
        {#if leagueTitle}{leagueTitle}{/if}
        {#if leagueTitle && seasonTitle}
          &middot;
        {/if}
        {#if seasonTitle}{seasonTitle}{/if}
      </div>
    {/if}
    <h2 class="event-title">
      {event.title || `Event ${event.episode_sort_order}`}
    </h2>
    <div class="panel-meta-row">
      <span class="panel-meta-item">Round {event.episode_sort_order}</span>
      {#if event.air_date}
        <span class="panel-meta-item">{event.air_date}</span>
      {/if}
      {#if event.runtime}
        <span class="panel-meta-item">{event.runtime}m</span>
      {/if}
    </div>
  </div>

  {#if event.description}
    <div class="panel-section">
      <h4 class="panel-section-title">Overview</h4>
      <p class="panel-overview">{event.description}</p>
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
      {#if isActivelyWanted(event.want_status)}
        <button class="panel-pill active" type="button" onclick={onUnwant}>
          <span class="material-icons-outlined">bookmark</span>
          Wanted
        </button>
      {:else if !event.is_in_library}
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
        totalSeconds={event.runtime ? event.runtime * 60 : null}
        watched={watchState?.watched ?? false}
        loading={playback.startingFileId !== null}
        variant="panel"
        onPlay={() =>
          playback.play(
            files[0].id,
            undefined,
            watchState?.watched ? undefined : (watchState?.position_seconds ?? undefined)
          )}
      />
    </div>
  {:else if stats.kobload_configured}
    <div class="panel-section">
      <PlaybackArea
        mode="stream"
        positionSeconds={watchState?.position_seconds ?? null}
        totalSeconds={event.runtime ? event.runtime * 60 : null}
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

{#if playback.isActive}
  <VideoPlayer
    sessionResponse={playback.session!}
    creationParams={{
      media_type: 'episode',
      file_id: playback.playingFileId!,
      capabilities: playback.capabilities!
    }}
    mediaId={event.id}
    initialPosition={watchState?.watched ? undefined : (watchState?.position_seconds ?? undefined)}
    onClose={playback.close}
  />
{/if}

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

<style>
  .event-detail {
    display: flex;
    flex-direction: column;
    gap: 0;
  }

  .still-section {
    margin: -20px -20px 0;
  }

  .event-still {
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

  .event-title {
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
