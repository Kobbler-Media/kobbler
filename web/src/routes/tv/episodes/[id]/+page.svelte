<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import ContentPageView from '$lib/components/ContentPageView.svelte';
  import { isActivelyWanted } from '$lib/utils/want-status';
  import EntityHero from '$lib/components/shared/EntityHero.svelte';
  import PlaybackArea from '$lib/components/shared/PlaybackArea.svelte';
  import HeroActions from '$lib/components/shared/HeroActions.svelte';
  import MetaLine from '$lib/components/shared/MetaLine.svelte';
  import FilesList from '$lib/components/shared/FilesList.svelte';
  import EpisodeForm from '$lib/components/tv/EpisodeForm.svelte';
  import type { EpisodeFormData } from '$lib/components/tv/EpisodeForm.svelte';
  import VideoMetadataFetchModal from '$lib/components/video/VideoMetadataFetchModal.svelte';
  import VideoPlayer from '$lib/components/VideoPlayer/VideoPlayer.svelte';
  import MoveFileModal from '$lib/components/shared/MoveFileModal.svelte';
  import SubtitlesList from '$lib/components/shared/SubtitlesList.svelte';
  import SubtitleSearchModal from '$lib/components/shared/SubtitleSearchModal.svelte';
  import ReleaseSearchModal from '$lib/components/ReleaseSearchModal.svelte';
  import type { SearchSelectItem } from '@koblibs/ui';
  import { showMessage } from '$lib/context/app.svelte';
  import { getStatsState } from '$lib/stores/stats.svelte';
  import { usePlayback } from '$lib/composables/usePlayback.svelte';
  import { useStreamPlayback } from '$lib/composables/useStreamPlayback.svelte';
  import StreamPlaybackOverlays from '$lib/components/StreamPlaybackOverlays.svelte';
  import { useTitleKeep } from '$lib/composables/useKeep.svelte';
  import * as api from '$lib/api/client';
  import { formatCalendarDate, formatEpisodeSearchTitle } from '$lib/utils/format';
  import type {
    TvEpisode,
    TvEpisodeFile,
    TvSeason,
    TvShow,
    TvWatchState,
    SeasonDetailResult,
    EpisodeDetailResult,
    SubtitleTrackInfo,
    ExternalSubtitleInfo
  } from '$lib/types';

  let episode = $state<TvEpisode | null>(null);
  let files = $state<TvEpisodeFile[]>([]);
  let loading = $state(true);
  let editing = $state(false);
  let saving = $state(false);
  let deleting = $state(false);
  let showFetchMetadata = $state(false);
  let editSearchResults = $state<EpisodeDetailResult[]>([]);
  let parentSeason = $state<TvSeason | null>(null);
  let parentShow = $state<TvShow | null>(null);
  let watchState = $state<TvWatchState | null>(null);
  let loadingWatch = $state(false);
  let deletingFileId = $state<string | null>(null);
  let rejecting = $state(false);
  let movingFileId = $state<string | null>(null);
  let siblingEpisodes = $state<SearchSelectItem[]>([]);
  let subtitleTracks = $state<SubtitleTrackInfo[]>([]);
  let externalSubtitles = $state<ExternalSubtitleInfo[]>([]);
  let showSubtitleSearch = $state(false);
  let showReleaseSearch = $state(false);
  let formValid = $state(false);
  let triggerSave = $state<(() => void) | undefined>(undefined);

  const episodeId = $derived($page.params.id);
  const statsState = getStatsState();

  const playback = usePlayback({
    mediaType: 'episode',
    refreshWatchState: async () => {
      if (episode) {
        watchState = await api.getEpisodeWatchState(episode.id);
      }
    }
  });

  const stream = useStreamPlayback({
    mediaType: 'episode',
    getMediaId: () => episode?.id ?? '',
    getSearchParams: () => ({ episode_id: episode?.id }),
    getResumePosition: () => watchState?.position_seconds ?? undefined,
    getEstimatedDuration: () => (episode?.runtime ? episode.runtime * 60 : undefined),
    playback,
    onMessage: showMessage,
    want: async () => handleWant(),
    canWant: () => !isActivelyWanted(episode?.want_status)
  });

  const titleKeep = useTitleKeep(showMessage);

  function closePlayer() {
    const streamed = playback.playingFileId?.startsWith('debrid-');
    stream.cancel();
    if (streamed && episode) void titleKeep.refresh(episode.id);
  }

  async function loadEpisode(id: string) {
    loading = true;
    try {
      const [ep, f, ws] = await Promise.all([
        api.getEpisode(id),
        api.getEpisodeFiles(id),
        api.getEpisodeWatchState(id)
      ]);
      episode = ep;
      files = f;
      watchState = ws;
      if (f.length === 0) void titleKeep.refresh(id);
      loadTracks(id);
      if (ep.season_id && parentSeason?.id !== ep.season_id) {
        try {
          parentSeason = await api.getSeason(ep.season_id);
          if (parentSeason?.show_id && parentShow?.id !== parentSeason.show_id) {
            parentShow = await api.getTvShow(parentSeason.show_id);
          }
        } catch {
          // Non-critical
        }
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load episode');
    } finally {
      loading = false;
    }
  }

  async function loadTracks(id: string) {
    try {
      const tracks = await api.getEpisodeTracks(id);
      subtitleTracks = tracks.files.flatMap((f) => f.subtitle_tracks);
      externalSubtitles = tracks.files.flatMap((f) => f.external_subtitles);
    } catch {
      // Non-critical
    }
  }

  async function toggleWatched() {
    if (!episode) return;
    loadingWatch = true;
    try {
      const newWatched = !watchState?.watched;
      const result = await api.updateEpisodeWatchState(episode.id, {
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

  const handleBack = () => {
    if (episode?.season_id) {
      goto(`/tv/seasons/${episode.season_id}`);
    } else {
      goto('/tv');
    }
  };

  const handleClose = () => {
    if (episode?.season_id) {
      goto(`/tv/seasons/${episode.season_id}`);
    } else {
      goto('/tv');
    }
  };

  async function handleWant() {
    if (!episode) return;
    try {
      await api.wantEpisode(episode.id);
      showMessage('Episode added to wanted');
      await loadEpisode(episode.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want episode');
    }
  }

  async function handleUnwant() {
    if (!episode) return;
    try {
      await api.unwantEpisode(episode.id);
      showMessage('Episode removed from wanted');
      await loadEpisode(episode.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant episode');
    }
  }

  const handleEdit = () => {
    editing = true;
    editSearchResults = [];
  };

  const handleEditCancel = () => {
    editing = false;
    editSearchResults = [];
  };

  const handleSave = async (data: EpisodeFormData, stillFile?: File) => {
    if (!episode) return;
    saving = true;
    try {
      await api.updateEpisode(episode.id, { ...data });
      if (stillFile) {
        await api.uploadEpisodeStill(episode.id, stillFile);
      }
      showMessage('Episode updated');
      editing = false;
      editSearchResults = [];
      await loadEpisode(episode.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update episode');
    } finally {
      saving = false;
    }
  };

  const handleFetchMetadataSelect = (results: SeasonDetailResult[]) => {
    const ep = episode;
    if (!ep) return;
    const epResults: EpisodeDetailResult[] = [];
    for (const seasonResult of results) {
      const match = seasonResult.episodes.find(
        (e) => String(e.episode_number) === ep.episode_identifier
      );
      if (match) {
        epResults.push(match);
      }
    }
    editSearchResults = epResults;
    showFetchMetadata = false;
    if (epResults.length === 0) {
      showMessage('No matching episode found in fetched metadata');
    }
  };

  const handleEditDelete = async () => {
    if (!episode) return;
    if (!confirm('Delete this episode? This cannot be undone.')) return;
    deleting = true;
    try {
      const seasonId = episode.season_id;
      await api.deleteEpisode(episode.id);
      showMessage('Episode deleted');
      if (seasonId) {
        goto(`/tv/seasons/${seasonId}`);
      } else {
        goto('/tv');
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete episode');
    } finally {
      deleting = false;
    }
  };

  async function handleDeleteFile(fileId: string) {
    if (!episode) return;
    if (!confirm('Delete this file? This cannot be undone.')) return;
    deletingFileId = fileId;
    try {
      await api.deleteEpisodeFile(episode.id, fileId);
      files = files.filter((f) => f.id !== fileId);
      showMessage('File deleted');
      await loadEpisode(episode.id);
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
      await api.replaceFile('tv_episode', episode!.id, fileId);
      showMessage('Replacement search started');
      await loadEpisode(episode!.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Replace failed');
    }
  }

  async function handleRejectAndResearch() {
    rejecting = true;
    try {
      await api.rejectDownload('tv_episode', episode!.id);
      showMessage('Download rejected — searching for new release');
      showReleaseSearch = true;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Reject failed');
    } finally {
      rejecting = false;
    }
  }

  async function handleMoveFile(fileId: string) {
    if (!episode) return;
    try {
      const season = await api.getSeason(episode.season_id);
      siblingEpisodes = season.episodes
        .filter((ep) => ep.id !== episode!.id)
        .map((ep) => ({
          value: ep.id,
          label: ep.title || `Episode ${ep.episode_sort_order}`
        }));
      movingFileId = fileId;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load season episodes');
    }
  }

  async function doMoveFile(targetEpisodeId: string) {
    if (!episode || !movingFileId) return;
    await api.moveEpisodeFile(episode.id, movingFileId, targetEpisodeId);
    movingFileId = null;
    siblingEpisodes = [];
    await loadEpisode(episode.id);
  }

  onMount(() => {
    if (episodeId) {
      loadEpisode(episodeId).then(() => {
        if (files.length > 0) {
          playback.preWarm(files[0].id, watchState?.position_seconds ?? undefined);
        } else {
          stream.preWarmStream();
        }
      });
    }
    return () => {
      playback.teardownPreWarm();
      titleKeep.dispose();
    };
  });

  // Reload only when the *route param* changes. Comparing against `episode.id`
  // instead would fight the player: an in-place Up Next switch advances `episode`
  // while the mounted route param still names the episode the page opened with,
  // and this effect would immediately load that old episode back.
  let loadedRouteId: string | undefined = untrack(() => episodeId);
  $effect(() => {
    const id = episodeId;
    if (id && id !== loadedRouteId) {
      loadedRouteId = id;
      loadEpisode(id);
    }
  });
</script>

<svelte:head>
  <title>{episode?.title || `Episode ${episode?.episode_sort_order ?? ''}`} — Kobbler</title>
</svelte:head>

{#if loading && !episode}
  <div class="loading-state"><p>Loading episode...</p></div>
{:else if episode && editing}
  <ContentPageView
    noHeader={true}
    noPadding={true}
    pageBackdropUrl={episode.still_url || parentShow?.backdrop_url}
    onClose={handleClose}
  >
    {#snippet headerActions()}
      {#if parentShow?.identifiers && Object.keys(parentShow.identifiers).length > 0 && parentSeason}
        <button
          type="button"
          class="glass-btn"
          onclick={() => (showFetchMetadata = true)}
          title="Search Metadata"
        >
          <span class="material-icons-outlined">search</span>
        </button>
      {/if}
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
          Editing: {episode.title || `Episode ${episode.episode_sort_order}`}
        </h2>
      </div>
    </div>

    <div class="page-container">
      <EpisodeForm
        {episode}
        searchResults={editSearchResults}
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
  {#if showFetchMetadata && parentShow?.identifiers && parentSeason}
    <VideoMetadataFetchModal
      showIdentifiers={parentShow.identifiers}
      seasonNumber={parentSeason.season_sort_order}
      onSelect={handleFetchMetadataSelect}
      onCancel={() => (showFetchMetadata = false)}
    />
  {/if}
{:else if episode}
  <ContentPageView
    noHeader={true}
    noPadding={true}
    pageBackdropUrl={episode.still_url || parentShow?.backdrop_url}
    onBack={handleBack}
    onClose={handleClose}
    onEdit={handleEdit}
  >
    <EntityHero
      title={episode.title || `Episode ${episode.episode_sort_order}`}
      coverUrl={episode.still_url}
      backdropUrl={episode.still_url}
      landscape={true}
      description={episode.description}
      hideRating={true}
    >
      {#snippet actions()}
        <HeroActions
          actions={[
            {
              label: watchState?.watched ? 'Watched' : 'Not Watched',
              icon: watchState?.watched ? 'visibility' : 'visibility_off',
              active: watchState?.watched ?? false,
              busy: loadingWatch,
              pinned: true,
              onSelect: toggleWatched
            },
            isActivelyWanted(episode!.want_status)
              ? { label: 'Remove from Wanted', icon: 'close', onSelect: handleUnwant }
              : !episode!.is_in_library && {
                  label: 'Want',
                  icon: 'bookmark_add',
                  tone: 'primary',
                  onSelect: handleWant
                },
            files.length === 0 && titleKeep.action,
            files.length === 0 &&
              statsState.stats.kobload_configured && {
                label: 'Search Releases',
                icon: 'search',
                onSelect: () => (showReleaseSearch = true)
              }
          ]}
        >
          {#snippet primary()}
            {#if files.length > 0}
              <PlaybackArea
                positionSeconds={watchState?.position_seconds ?? null}
                totalSeconds={episode!.runtime ? episode!.runtime * 60 : null}
                watched={watchState?.watched ?? false}
                loading={playback.startingFileId !== null}
                variant="hero"
                onPlay={() =>
                  playback.play(files[0].id, undefined, watchState?.position_seconds ?? undefined)}
              />
            {:else if statsState.stats.kobload_configured}
              <PlaybackArea
                mode="stream"
                positionSeconds={watchState?.position_seconds ?? null}
                totalSeconds={episode!.runtime ? episode!.runtime * 60 : null}
                watched={watchState?.watched ?? false}
                loading={stream.busy}
                variant="hero"
                onPlay={stream.autoPlay}
                onPickStream={stream.openModal}
              />
            {/if}
          {/snippet}
        </HeroActions>
      {/snippet}

      {#snippet meta()}
        <div class="episode-meta-stack">
          {#if parentShow && parentSeason}
            <div class="breadcrumb">
              <a href="/tv/{parentShow.id}">{parentShow.title}</a>
              <span class="breadcrumb-sep">&rsaquo;</span>
              <a href="/tv/seasons/{parentSeason.id}">Season {parentSeason.season_sort_order}</a>
            </div>
          {/if}
          <MetaLine
            items={[
              `Episode ${episode!.episode_sort_order}`,
              formatCalendarDate(episode!.air_date),
              episode!.runtime && `${episode!.runtime}m`
            ]}
          />
        </div>
      {/snippet}
    </EntityHero>

    <div class="full-page-main-grid page-container">
      <div class="details-column">
        {#if episode.description}
          <div class="card-section">
            <div class="label">Overview</div>
            <p class="overview">{episode.description}</p>
          </div>
        {/if}
      </div>

      <div class="actions-column side-first">
        {#if files.length > 0}
          <div class="card-section">
            <div class="label">Files</div>
            <FilesList
              {files}
              noun="episode"
              {deletingFileId}
              startingPlaybackFileId={playback.startingFileId}
              onPlay={(fileId) => playback.play(fileId)}
              onDelete={handleDeleteFile}
              onMove={handleMoveFile}
              onReplace={handleReplace}
              onSearchReleases={statsState.stats.kobload_configured
                ? () => (showReleaseSearch = true)
                : undefined}
              onRejectAndResearch={handleRejectAndResearch}
              {rejecting}
            />
          </div>
        {/if}

        {#if files.length > 0}
          <div class="card-section">
            <SubtitlesList
              {subtitleTracks}
              {externalSubtitles}
              onSearchSubtitles={() => (showSubtitleSearch = true)}
            />
          </div>
        {/if}
      </div>
    </div>
  </ContentPageView>
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
    mediaId={episode?.id ?? ''}
    mediaInfo={{
      title: parentShow?.title ?? '',
      subtitle: `S${parentSeason?.season_sort_order ?? '?'}E${episode?.episode_sort_order ?? '?'}${episode?.title ? ` \u00b7 ${episode.title}` : ''}`,
      seasonId: episode?.season_id,
      artworkUrl: episode?.still_url ?? parentShow?.poster_url ?? undefined,
      backdropUrl: episode?.still_url ?? parentShow?.backdrop_url ?? undefined
    }}
    initialPosition={watchState?.position_seconds ?? undefined}
    onClose={closePlayer}
    {...stream.playerProps}
    onEpisodeChange={(id) => loadEpisode(id)}
  />
{/if}

{#if movingFileId}
  <MoveFileModal
    title="Move File to Episode"
    items={siblingEpisodes}
    onMove={doMoveFile}
    onClose={() => {
      movingFileId = null;
      siblingEpisodes = [];
    }}
  />
{/if}

{#if showSubtitleSearch && episode && files.length > 0}
  <SubtitleSearchModal
    mediaType="episode"
    entityId={episode.id}
    fileId={files[0].id}
    onClose={() => (showSubtitleSearch = false)}
    onDownloaded={() => {
      showSubtitleSearch = false;
      if (episode) loadTracks(episode.id);
    }}
  />
{/if}

{#if showReleaseSearch && episode}
  <ReleaseSearchModal
    entityTitle={episode.title || `Episode ${episode.episode_sort_order}`}
    initialQuery={formatEpisodeSearchTitle(
      parentShow?.title,
      parentSeason?.season_identifier ?? '',
      episode.episode_identifier
    )}
    onClose={() => (showReleaseSearch = false)}
    searchFn={(query, scope) => api.searchEpisodeReleases(episode!.id, query, scope)}
    scopeLabel="TV"
    smartSearchFn={() => api.searchEpisodeReleases(episode!.id)}
    queueFn={async (release, source) => {
      await api.queueEpisodeRelease(episode!.id, release, source);
      await loadEpisode(episode!.id);
    }}
    wantFn={async () => {
      await api.wantEpisode(episode!.id);
      await loadEpisode(episode!.id);
    }}
  />
{/if}

<StreamPlaybackOverlays {stream} />

<style>
  .loading-state {
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 200px;
    color: var(--muted);
  }

  /* Two-column layout */
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

  .overview {
    font-size: 14px;
    line-height: 1.7;
    color: var(--ink);
    margin: 0;
  }

  /* Hero meta */
  .episode-meta-stack {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  /* Breadcrumb in meta snippet */
  .breadcrumb {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--muted);
    margin-bottom: 4px;
  }

  .breadcrumb a {
    color: var(--muted);
    text-decoration: none;
    transition: color 0.15s;
  }

  .breadcrumb a:hover {
    color: var(--accent);
  }

  .breadcrumb-sep {
    font-size: 11px;
    opacity: 0.5;
  }

  /* Edit mode hero */
  .edit-hero {
    position: relative;
    z-index: 1;
    height: 56px;
  }

  .edit-hero-content {
    display: flex;
    align-items: center;
    height: 100%;
    padding: 0 clamp(24px, 5vw, 80px);
    max-width: 1600px;
    margin: 0 auto;
  }

  .edit-hero-title {
    font-size: 15px;
    font-weight: 600;
    color: var(--ink);
    margin: 0;
    letter-spacing: -0.01em;
  }

  .page-container {
    position: relative;
    z-index: 1;
  }

  .page-container :global(.form-inner) {
    padding-top: 20px;
  }

  /* Edit footer */
  .edit-footer {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
  }

  .spacer {
    flex: 1;
  }

  /* Responsive */
</style>
