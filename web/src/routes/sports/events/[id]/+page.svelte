<script lang="ts">
  import { onMount } from 'svelte';
  import { on, off } from '$lib/services/eventstream.svelte';
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import ContentPageView from '$lib/components/ContentPageView.svelte';
  import { isActivelyWanted } from '$lib/utils/want-status';
  import MoveFileModal from '$lib/components/shared/MoveFileModal.svelte';
  import EpisodeForm from '$lib/components/tv/EpisodeForm.svelte';
  import type { EpisodeFormData } from '$lib/components/tv/EpisodeForm.svelte';
  import type { SearchSelectItem } from '@koblibs/ui';
  import ReleaseSearchModal from '$lib/components/ReleaseSearchModal.svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import { getStatsState } from '$lib/stores/stats.svelte';
  import * as api from '$lib/api/client';
  import {
    getEpisodeWatchState,
    updateEpisodeWatchState,
    deleteEpisodeFile,
    rejectDownload,
    replaceFile
  } from '$lib/api/client';
  import { usePlayback } from '$lib/composables/usePlayback.svelte';
  import { useStreamPlayback } from '$lib/composables/useStreamPlayback.svelte';
  import VideoPlayer from '$lib/components/VideoPlayer/VideoPlayer.svelte';
  import PlaybackArea from '$lib/components/shared/PlaybackArea.svelte';
  import WatchToggle from '$lib/components/shared/WatchToggle.svelte';
  import FilesList from '$lib/components/shared/FilesList.svelte';
  import EntityHero from '$lib/components/shared/EntityHero.svelte';
  import StreamOptionsModal from '$lib/components/StreamOptionsModal.svelte';
  import StreamCachingOverlay from '$lib/components/StreamCachingOverlay.svelte';
  import type {
    TvEpisode,
    TvEpisodeFile,
    TvWatchState,
    TvSeason,
    TvShow,
    PlaybackSessionResponse
  } from '$lib/types';

  let event = $state<TvEpisode | null>(null);
  let files = $state<TvEpisodeFile[]>([]);
  let watchState = $state<TvWatchState | null>(null);
  let parentSeason = $state<TvSeason | null>(null);
  let parentLeague = $state<TvShow | null>(null);
  let loading = $state(true);
  let loadingWatch = $state(false);
  let editing = $state(false);
  let saving = $state(false);
  let deleting = $state(false);
  let deletingFileId = $state<string | null>(null);
  let rejecting = $state(false);
  let showReleaseSearch = $state(false);
  let movingFileId = $state<string | null>(null);
  let siblingEvents = $state<SearchSelectItem[]>([]);
  let formValid = $state(false);
  let triggerSave = $state<(() => void) | undefined>();

  const stats = getStatsState();
  const eventId = $derived($page.params.id);
  const pageState = $derived($page.state as { playbackSession?: PlaybackSessionResponse });

  const playback = usePlayback({
    mediaType: 'episode',
    refreshWatchState: async () => {
      if (event) watchState = await getEpisodeWatchState(event.id);
    }
  });

  const stream = useStreamPlayback({
    mediaType: 'episode',
    getMediaId: () => event?.id ?? '',
    getSearchParams: () => ({ episode_id: event?.id }),
    getResumePosition: () => watchState?.position_seconds ?? undefined,
    getEstimatedDuration: () => (event?.runtime ? event.runtime * 60 : undefined),
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

  async function loadEvent(id: string) {
    loading = true;
    try {
      const [ev, f, ws] = await Promise.all([
        api.getSportsEvent(id),
        api.getSportsEventFiles(id),
        getEpisodeWatchState(id)
      ]);
      event = ev;
      files = f;
      watchState = ws;

      // Load parent season and league for hero display
      if (ev.season_id) {
        try {
          const season = await api.getSportsSeason(ev.season_id);
          parentSeason = season;
          if (season.show_id) {
            parentLeague = await api.getSportsLeague(season.show_id);
          }
        } catch {
          /* Non-critical */
        }
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load event');
    } finally {
      loading = false;
    }
  }

  const handleBack = () => {
    if (event?.season_id) {
      goto(`/sports/seasons/${event.season_id}`);
    } else {
      goto('/sports');
    }
  };

  async function handleWant() {
    if (!event) return;
    try {
      await api.wantSportsEvent(event.id);
      showMessage('Event added to wanted');
      await loadEvent(event.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want event');
    }
  }

  async function handleUnwant() {
    if (!event) return;
    try {
      await api.unwantSportsEvent(event.id);
      showMessage('Event removed from wanted');
      await loadEvent(event.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant event');
    }
  }

  const handleEdit = () => {
    editing = true;
  };

  const handleEditCancel = () => {
    editing = false;
  };

  const handleEditDelete = async () => {
    if (!event) return;
    if (!confirm('Delete this event? This cannot be undone.')) return;
    deleting = true;
    try {
      const seasonId = event.season_id;
      await api.deleteSportsEvent(event.id);
      showMessage('Event deleted');
      if (seasonId) {
        goto(`/sports/seasons/${seasonId}`);
      } else {
        goto('/sports');
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete event');
    } finally {
      deleting = false;
    }
  };

  const handleClose = () => handleBack();

  const handleSave = async (data: EpisodeFormData, stillFile?: File) => {
    if (!event) return;
    saving = true;
    try {
      await api.updateSportsEvent(event.id, { ...data });
      if (stillFile) {
        await api.uploadSportsEventStill(event.id, stillFile);
      }
      showMessage('Event updated');
      editing = false;
      await loadEvent(event.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update event');
    } finally {
      saving = false;
    }
  };

  const handleDelete = async () => {
    if (!event) return;
    if (!confirm('Delete this event? This cannot be undone.')) return;
    deleting = true;
    try {
      const seasonId = event.season_id;
      await api.deleteSportsEvent(event.id);
      showMessage('Event deleted');
      if (seasonId) {
        goto(`/sports/seasons/${seasonId}`);
      } else {
        goto('/sports');
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete event');
    } finally {
      deleting = false;
    }
  };

  async function handleMoveFile(fileId: string) {
    if (!event?.season_id) return;
    try {
      const season = await api.getSportsSeason(event.season_id);
      siblingEvents = season.episodes
        .filter((ep) => ep.id !== event!.id)
        .map((ep) => ({
          value: ep.id,
          label: ep.title || `Event ${ep.episode_sort_order}`
        }));
      movingFileId = fileId;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load sibling events');
    }
  }

  async function doMoveFile(targetEventId: string) {
    if (!event || !movingFileId) return;
    await api.moveSportsEventFile(event.id, movingFileId, targetEventId);
    await loadEvent(event.id);
  }

  async function toggleWatched() {
    if (!event) return;
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
    if (!event) return;
    if (!confirm('Delete this file? This cannot be undone.')) return;
    deletingFileId = fileId;
    try {
      await deleteEpisodeFile(event.id, fileId);
      files = files.filter((f) => f.id !== fileId);
      showMessage('File deleted');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Delete failed');
    } finally {
      deletingFileId = null;
    }
  }

  async function handleReplace(fileId: string) {
    if (!event) return;
    if (!confirm('Replace this file? The current file will be deleted and a new search started.'))
      return;
    try {
      await replaceFile('tv_episode', event.id, fileId);
      showMessage('Replacement search started');
      await loadEvent(event.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Replace failed');
    }
  }

  async function handleRejectAndResearch() {
    if (!event) return;
    rejecting = true;
    try {
      await rejectDownload('tv_episode', event.id);
      showMessage('Download rejected — searching for new release');
      showReleaseSearch = true;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Reject failed');
    } finally {
      rejecting = false;
    }
  }

  onMount(() => {
    if (eventId) {
      loadEvent(eventId).then(() => {
        if (pageState?.playbackSession) {
          playback.playSession(pageState.playbackSession);
        } else if (files.length > 0) {
          playback.preWarm(files[0].id, watchState?.position_seconds ?? undefined);
        } else {
          stream.preWarmStream();
        }
      });
    }
    return () => {
      playback.teardownPreWarm();
      stream.teardown();
    };
  });

  $effect(() => {
    if (eventId && (!event || event.id !== eventId)) loadEvent(eventId);
  });
</script>

<svelte:head>
  <title>{event?.title || `Event ${event?.episode_sort_order ?? ''}`} — Kobbler</title>
</svelte:head>

{#if loading && !event}
  <div class="loading-state"><p>Loading event...</p></div>
{:else if event && editing}
  <ContentPageView
    noHeader={true}
    noPadding={true}
    pageBackdropUrl={event.still_url || parentLeague?.backdrop_url}
    onClose={handleClose}
  >
    {#snippet headerActions()}
      <button
        type="button"
        class="glass-btn active"
        onclick={handleEditCancel}
        title="Editing (click to cancel)"
      >
        <span class="material-icons-outlined">edit</span>
      </button>
      <button type="button" class="glass-btn" onclick={handleClose} title="Close">
        <span class="material-icons-outlined">close</span>
      </button>
    {/snippet}

    <div class="edit-hero">
      <div class="edit-hero-content">
        <h2 class="edit-hero-title">
          Editing: {event.title || `Event ${event.episode_sort_order}`}
        </h2>
      </div>
    </div>

    <div class="page-container">
      <EpisodeForm
        episode={event}
        hideFooter
        bind:requestSave={triggerSave}
        bind:canSave={formValid}
        onSave={handleSave}
        onCancel={handleEditCancel}
        onDelete={handleEditDelete}
        {saving}
        {deleting}
      />
    </div>

    {#snippet footer()}
      <div class="edit-footer">
        <button
          type="button"
          class="btn btn-danger"
          onclick={handleEditDelete}
          disabled={deleting || saving}
        >
          <span class="material-icons-outlined">delete</span>
          {deleting ? 'Deleting...' : 'Delete'}
        </button>
        <div class="spacer"></div>
        <button type="button" class="btn btn-sec" onclick={handleEditCancel} disabled={saving}>
          Cancel
        </button>
        <button
          type="button"
          class="btn btn-primary"
          onclick={() => triggerSave?.()}
          disabled={!formValid || saving}
        >
          <span class="material-icons-outlined">save</span>
          {saving ? 'Saving...' : 'Save Changes'}
        </button>
      </div>
    {/snippet}
  </ContentPageView>
{:else if event}
  <ContentPageView
    noHeader={true}
    noPadding={true}
    pageBackdropUrl={event.still_url || parentLeague?.backdrop_url}
    onBack={handleBack}
    onClose={handleBack}
    onEdit={handleEdit}
  >
    <EntityHero
      title={event.title || `Event ${event.episode_sort_order}`}
      coverUrl={event.still_url}
      backdropUrl={parentLeague?.backdrop_url}
      description={event.description}
    >
      {#snippet meta()}
        <div class="hero-meta-snippet">
          {#if parentLeague}
            <a href="/sports/{parentLeague.id}" class="breadcrumb-link">{parentLeague.title}</a>
            <span class="sep">&middot;</span>
          {/if}
          {#if parentSeason}
            <a href="/sports/seasons/{parentSeason.id}" class="breadcrumb-link"
              >Season {parentSeason.season_sort_order}</a
            >
            <span class="sep">&middot;</span>
          {/if}
          <span>Event {event!.episode_sort_order}</span>
          {#if event!.air_date}
            <span class="sep">&middot;</span>
            <span>{event!.air_date}</span>
          {/if}
          {#if event!.runtime}
            <span class="sep">&middot;</span>
            <span>{event!.runtime}m</span>
          {/if}
        </div>
      {/snippet}

      {#snippet actions()}
        {#if files.length > 0}
          <PlaybackArea
            positionSeconds={watchState?.position_seconds ?? null}
            totalSeconds={event!.runtime ? event!.runtime * 60 : null}
            watched={watchState?.watched ?? false}
            loading={playback.startingFileId !== null}
            variant="hero"
            onPlay={() =>
              playback.play(files[0].id, undefined, watchState?.position_seconds ?? undefined)}
          />
        {:else if stats.stats.kobload_configured}
          <PlaybackArea
            mode="stream"
            positionSeconds={watchState?.position_seconds ?? null}
            totalSeconds={event!.runtime ? event!.runtime * 60 : null}
            watched={watchState?.watched ?? false}
            loading={stream.resolving || stream.streamLoading}
            variant="hero"
            onPlay={stream.autoPlay}
            onPickStream={stream.openModal}
          />
        {/if}

        <WatchToggle
          watched={watchState?.watched ?? false}
          loading={loadingWatch}
          onToggle={toggleWatched}
        />

        {#if isActivelyWanted(event!.want_status)}
          <button class="btn btn-outline" type="button" onclick={handleUnwant}>
            <span class="material-icons-outlined">close</span> Remove from Wanted
          </button>
        {:else if !event!.is_in_library}
          <button class="btn btn-primary" type="button" onclick={handleWant}>
            <span class="material-icons-outlined">bookmark_add</span> Want
          </button>
        {/if}

        {#if stats.stats.kobload_configured}
          <button class="btn btn-outline" type="button" onclick={() => (showReleaseSearch = true)}>
            <span class="material-icons-outlined">search</span> Search Releases
          </button>
        {/if}

        <button class="btn btn-danger-icon" type="button" onclick={handleDelete} title="Delete">
          <span class="material-icons-outlined">delete</span>
        </button>
      {/snippet}
    </EntityHero>

    <div class="full-page-main-grid page-container">
      <div class="details-column">
        {#if parentLeague || parentSeason}
          <div class="section card-section">
            <div class="label">Breadcrumb</div>
            <div class="breadcrumb-row">
              {#if parentLeague}
                <a href="/sports/{parentLeague.id}" class="breadcrumb-link">{parentLeague.title}</a>
              {/if}
              {#if parentLeague && parentSeason}
                <span class="material-icons-outlined breadcrumb-arrow">chevron_right</span>
              {/if}
              {#if parentSeason}
                <a href="/sports/seasons/{parentSeason.id}" class="breadcrumb-link"
                  >Season {parentSeason.season_sort_order}</a
                >
              {/if}
            </div>
          </div>
        {/if}
      </div>

      <div class="actions-column">
        {#if files.length > 0}
          <div class="section card-section">
            <div class="label">Files</div>
            <FilesList
              {files}
              {deletingFileId}
              startingPlaybackFileId={playback.startingFileId}
              onPlay={(fileId) => playback.play(fileId)}
              onDelete={handleDeleteFile}
              onMove={handleMoveFile}
              onReplace={handleReplace}
              onSearchReleases={stats.stats.kobload_configured
                ? () => (showReleaseSearch = true)
                : undefined}
              onRejectAndResearch={handleRejectAndResearch}
              {rejecting}
            />
          </div>
        {/if}
      </div>
    </div>
  </ContentPageView>
{/if}

{#if playback.isActive && event}
  <VideoPlayer
    sessionResponse={playback.session}
    creationParams={playback.session
      ? {
          media_type: 'episode',
          file_id: playback.playingFileId!,
          capabilities: playback.capabilities!
        }
      : undefined}
    mediaId={event.id}
    mediaInfo={{
      title: parentLeague?.title ?? event.title ?? '',
      subtitle: event.title || `Event ${event.episode_sort_order}`,
      seasonId: event.season_id,
      artworkUrl: event.still_url ?? parentLeague?.poster_url ?? undefined
    }}
    initialPosition={watchState?.position_seconds ?? undefined}
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

{#if showReleaseSearch && event}
  <ReleaseSearchModal
    entityTitle={event.title || `Event ${event.episode_sort_order}`}
    onClose={() => (showReleaseSearch = false)}
    searchFn={(query, scope) => api.searchSportsEventReleases(event!.id, query || undefined, scope)}
    scopeLabel="sports"
    smartSearchFn={() => api.searchSportsEventReleases(event!.id)}
    queueFn={async (release, source) => {
      await api.queueSportsEventRelease(event!.id, source, release);
      await loadEvent(event!.id);
    }}
    wantFn={async () => {
      await api.wantSportsEvent(event!.id);
      await loadEvent(event!.id);
    }}
  />
{/if}

{#if movingFileId}
  <MoveFileModal
    title="Move File to Event"
    items={siblingEvents}
    onMove={doMoveFile}
    onClose={() => (movingFileId = null)}
  />
{/if}

<style>
  .loading-state {
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 200px;
    color: var(--muted);
  }

  .edit-hero {
    position: relative;
    z-index: 1;
    padding: 40px 32px 24px;
  }

  .edit-hero-content {
    max-width: 800px;
    margin: 0 auto;
  }

  .edit-hero-title {
    margin: 0;
    font-size: 24px;
    font-weight: 800;
    color: var(--ink);
  }

  .edit-footer {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 16px 32px;
    background: var(--card);
    border-top: 1px solid var(--ring);
  }

  .spacer {
    flex: 1;
  }

  .btn-sec {
    background: var(--bg-2);
    border: 1px solid var(--ring);
    color: var(--ink);
  }

  .btn-danger {
    background: transparent;
    border: 1px solid var(--ring);
    color: var(--error, #ef4444);
  }

  .btn-danger:hover {
    background: rgba(239, 68, 68, 0.1);
    border-color: var(--error, #ef4444);
  }

  .full-page-main-grid {
    display: grid;
    grid-template-columns: 1fr 400px;
    gap: 32px;
    align-items: start;
  }

  .details-column,
  .actions-column {
    display: flex;
    flex-direction: column;
    gap: 24px;
    min-width: 0;
  }

  .card-section {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 24px;
  }

  .label {
    font-size: 11px;
    font-weight: 800;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.1em;
    margin-bottom: 12px;
  }

  .hero-meta-snippet {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--muted);
    font-size: 14px;
    font-weight: 500;
  }

  .sep {
    opacity: 0.5;
  }

  .breadcrumb-link {
    color: var(--accent);
    text-decoration: none;
    font-weight: 600;
  }

  .breadcrumb-link:hover {
    text-decoration: underline;
  }

  .breadcrumb-row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 14px;
  }

  .breadcrumb-arrow {
    font-size: 18px;
    color: var(--muted);
    opacity: 0.5;
  }

  .btn {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 10px 20px;
    border-radius: 100px;
    font-size: 14px;
    font-weight: 700;
    cursor: pointer;
    border: none;
    transition: all 0.2s;
  }

  .btn-primary {
    background: var(--accent);
    color: var(--bg);
  }

  .btn-danger-icon {
    width: 44px;
    height: 44px;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 50%;
    background: transparent;
    border: 1px solid var(--ring);
    color: var(--muted);
    cursor: pointer;
    transition: all 0.2s;
  }

  .btn-danger-icon:hover {
    border-color: var(--danger);
    color: var(--danger);
    background: rgba(239, 68, 68, 0.1);
  }

  @media (max-width: 1100px) {
    .full-page-main-grid {
      grid-template-columns: 1fr;
    }

    .actions-column {
      order: -1;
    }
  }
</style>
