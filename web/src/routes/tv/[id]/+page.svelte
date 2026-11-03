<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import TvShowDetailPage from '#lib/components/tv/TvShowDetailPage.svelte';
  import TvShowForm from '#lib/components/tv/TvShowForm.svelte';
  import type { TvShowFormData } from '#lib/components/tv/TvShowForm.svelte';
  import SeasonForm from '#lib/components/tv/SeasonForm.svelte';
  import type { SeasonFormData } from '#lib/components/tv/SeasonForm.svelte';
  import VideoMetadataSearchModal from '#lib/components/video/VideoMetadataSearchModal.svelte';
  import ReleaseSearchModal from '#lib/components/ReleaseSearchModal.svelte';
  import ContentPageView from '#lib/components/ContentPageView.svelte';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { getStatsState } from '#lib/stores/stats.svelte.js';
  import * as api from '#lib/api/client.js';
  import { on, off } from '#lib/services/eventstream.svelte.js';
  import { getJobsState } from '#lib/stores/jobs.svelte.js';
  import { watchStateVersion } from '#lib/stores/watchState.svelte.js';
  import { formatEpisodeSearchTitle } from '#lib/utils/format.js';
  import { unwantMessage } from '#lib/utils/want-status.js';
  import type { TvShow, TvSearchResult, MovieSearchResult } from '#lib/types.js';

  let show = $state<TvShow | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let editing = $state(false);
  let saving = $state(false);
  let deleting = $state(false);
  let addingSeason = $state(false);
  let savingSeason = $state(false);
  let showMetadataSearch = $state(false);
  let releaseSearchEpisodeId = $state<string | null>(null);
  let editSearchResults = $state<(TvSearchResult | MovieSearchResult)[]>([]);
  let triggerSave = $state<(() => void) | undefined>(undefined);
  let formValid = $state(false);

  const showId = $derived(page.params.id);

  const releaseSearchTitle = $derived.by(() => {
    if (!releaseSearchEpisodeId || !show) return 'Episode';
    for (const season of show.seasons ?? []) {
      const episode = season.episodes?.find((e) => e.id === releaseSearchEpisodeId);
      if (episode) {
        return formatEpisodeSearchTitle(
          show.title,
          season.season_identifier,
          episode.episode_identifier
        );
      }
    }
    return 'Episode';
  });
  const statsState = getStatsState();
  const pageState = $derived(page.state as { editing?: boolean });
  const jobsState = getJobsState();
  const syncing = $derived(
    jobsState.jobs.some(
      (j) =>
        j.entity_type === 'tv_show' &&
        j.entity_id === showId &&
        j.job_type === 'tv_show_enrich' &&
        (j.status === 'pending' || j.status === 'running')
    )
  );

  const loadShow = async (id: string) => {
    loading = true;
    error = null;
    try {
      show = await api.getTvShow(id);
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load show';
      showMessage(error);
    } finally {
      loading = false;
    }
  };

  let lastWatchVersion = -1;
  $effect(() => {
    const v = watchStateVersion();
    if (lastWatchVersion !== -1 && v !== lastWatchVersion && show) loadShow(show.id);
    lastWatchVersion = v;
  });

  const handleMonitorToggle = async (monitored: boolean, mode: string) => {
    if (!show) return;
    try {
      await api.setTvShowMonitored(show.id, monitored, mode);
      showMessage(monitored ? 'Show monitored' : 'Show unmonitored');
      await loadShow(show.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update monitoring');
    }
  };

  const handleWatchlistToggle = async (inWatchlist: boolean) => {
    if (!show) return;
    try {
      if (inWatchlist) {
        await api.addToWatchlist('show', show.id);
        showMessage('Added to your watch list');
      } else {
        await api.removeFromWatchlist('show', show.id);
        showMessage('Removed from your watch list');
      }
      await loadShow(show.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update your watch list');
    }
  };

  const handleWantAllMissing = async () => {
    if (!show) return;
    try {
      await api.wantAllMissing(show.id);
      showMessage('Wanted all missing episodes');
      await loadShow(show.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want missing episodes');
    }
  };

  const handleUnwantAll = async () => {
    if (!show) return;
    if (
      !confirm(
        `Unwant every outstanding episode of "${show.title}"? Episodes already in your library are unaffected.`
      )
    )
      return;
    try {
      const result = await api.unwantAll(show.id);
      showMessage(unwantMessage(result, 'show'));
      await loadShow(show.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant episodes');
    }
  };

  const handleRefresh = async () => {
    if (!show) return;
    try {
      await api.refreshTvShowMetadata(show.id);
      showMessage('Metadata refresh queued');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to refresh metadata');
    }
  };

  const handleReload = async () => {
    if (!show) return;
    await loadShow(show.id);
  };

  const handleReanalyze = async (force = false) => {
    if (!show) return;
    try {
      const result = await api.runMediaAnalyzeShow(show.id, force);
      if (result.enqueued > 0) {
        const label = force ? 'Queued force re-analyze' : 'Queued re-analyze';
        showMessage(`${label} for ${result.enqueued} file${result.enqueued === 1 ? '' : 's'}`);
      } else {
        showMessage('No files to analyze');
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to run media analysis');
    }
  };

  const handleWantSeason = async (seasonId: string) => {
    try {
      await api.wantSeason(seasonId);
      showMessage('Season episodes wanted');
      if (show) await loadShow(show.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want season');
    }
  };

  const handleUnwantSeason = async (seasonId: string) => {
    const season = show?.seasons?.find((s) => s.id === seasonId);
    const label = season?.is_special
      ? 'Specials'
      : `Season ${season?.season_identifier ?? ''}`.trim();
    if (
      !confirm(
        `Unwant every outstanding episode in ${label}? Episodes already in your library are unaffected.`
      )
    )
      return;
    try {
      const result = await api.unwantSeason(seasonId);
      showMessage(unwantMessage(result, 'season'));
      if (show) await loadShow(show.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant season');
    }
  };

  const handleWantEpisode = async (episodeId: string) => {
    try {
      await api.wantEpisode(episodeId);
      showMessage('Episode wanted');
      if (show) await loadShow(show.id);
      // Check if streaming is available
      api
        .streamSearch({ episode_id: episodeId })
        .then((result) => {
          if (result.options.length > 0) {
            showMessage('This is also available for streaming!');
          }
        })
        .catch(() => {
          /* non-critical */
        });
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want episode');
    }
  };

  const handleUnwantEpisode = async (episodeId: string) => {
    try {
      await api.unwantEpisode(episodeId);
      showMessage('Episode removed from wanted');
      if (show) await loadShow(show.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant episode');
    }
  };

  const handleBulkShowWatch = async (watched: boolean) => {
    if (!show) return;
    try {
      const result = await api.bulkUpdateShowWatchState(show.id, watched);
      showMessage(
        `Marked ${result.updated_count} episode${result.updated_count === 1 ? '' : 's'} as ${watched ? 'watched' : 'unwatched'}`
      );
      await loadShow(show.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update watch state');
    }
  };

  const handleRewatch = async () => {
    if (!show) return;
    try {
      await api.bulkUpdateShowWatchState(show.id, false);
      showMessage(`${show.title} reset — Resume starts from the first episode`);
      await loadShow(show.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to reset watch progress');
    }
  };

  const handleBulkSeasonWatch = async (seasonId: string, watched: boolean) => {
    try {
      const result = await api.bulkUpdateSeasonWatchState(seasonId, watched);
      showMessage(
        `Marked ${result.updated_count} episode${result.updated_count === 1 ? '' : 's'} as ${watched ? 'watched' : 'unwatched'}`
      );
      if (show) await loadShow(show.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update watch state');
    }
  };

  const handleWatchToggle = async (episodeId: string) => {
    try {
      const episode = show?.seasons?.flatMap((s) => s.episodes).find((e) => e.id === episodeId);
      const isWatched = episode?.watched ?? false;
      await api.updateEpisodeWatchState(episodeId, {
        watched: !isWatched,
        watched_at: !isWatched ? new Date().toISOString() : null
      });
      showMessage(!isWatched ? 'Marked as watched' : 'Marked as unwatched');
      if (show) await loadShow(show.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to toggle watch state');
    }
  };

  const handleClose = () => {
    goto('/tv');
  };

  const handleCastClick = (personId: string, _personName: string) => {
    goto(`/people/${personId}`);
  };

  const handleEdit = () => {
    editing = true;
    editSearchResults = [];
  };

  const handleEditCancel = () => {
    editing = false;
    editSearchResults = [];
  };

  const handleSave = async (
    data: TvShowFormData,
    posterFile?: File,
    backdropFile?: File,
    logoFile?: File
  ) => {
    if (!show) return;
    saving = true;
    try {
      const { cast, crew, ...metadata } = data;
      await api.updateTvShow(show.id, metadata as unknown as Record<string, unknown>);
      if (cast) await api.replaceTvShowCast(show.id, cast);
      if (crew) await api.replaceTvShowCrew(show.id, crew);
      if (posterFile) {
        await api.uploadTvShowPoster(show.id, posterFile);
      }
      if (backdropFile) {
        await api.uploadTvShowBackdrop(show.id, backdropFile);
      }
      if (logoFile) {
        await api.uploadTvShowLogo(show.id, logoFile);
      }
      showMessage('Show updated');
      editing = false;
      editSearchResults = [];
      await loadShow(show.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update show');
    } finally {
      saving = false;
    }
  };

  const handleMetadataSearchSelect = (results: (TvSearchResult | MovieSearchResult)[]) => {
    editSearchResults = results;
    showMetadataSearch = false;
  };

  const handleDeleteShow = async () => {
    if (!show) return;
    if (!confirm(`Delete "${show.title}"? This cannot be undone.`)) return;
    deleting = true;
    try {
      await api.deleteTvShow(show.id);
      showMessage('Show deleted');
      goto('/tv');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete show');
    } finally {
      deleting = false;
    }
  };

  const handleSeasonAutoWantChange = async (seasonId: string, mode: string) => {
    try {
      await api.setSeasonAutoWant(seasonId, mode);
      showMessage('Season auto-want updated');
      if (show) await loadShow(show.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update season auto-want');
    }
  };

  const handleSearchReleases = (episodeId: string) => {
    releaseSearchEpisodeId = episodeId;
  };

  const handleAddSeason = () => {
    addingSeason = true;
  };

  const handleSaveSeason = async (data: SeasonFormData) => {
    if (!show) return;
    savingSeason = true;
    try {
      await api.createSeason(show.id, {
        season_identifier: data.season_identifier,
        season_sort_order: data.season_sort_order,
        title: data.title ?? undefined,
        description: data.description ?? undefined,
        air_date: data.air_date ?? undefined,
        episode_count: data.episode_count ?? undefined,
        is_special: data.is_special
      });
      showMessage('Season created');
      addingSeason = false;
      await loadShow(show.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to create season');
    } finally {
      savingSeason = false;
    }
  };

  // SSE: auto-refresh when enrichment completes
  const tvUpdatedCb = on('tv_show_updated', (data: { show_id: string }) => {
    if (data.show_id === showId) {
      loadShow(showId);
    }
  });

  onMount(() => {
    if (pageState?.editing) {
      editing = true;
    }
    if (showId) {
      loadShow(showId);
    }
    return () => {
      off('tv_show_updated', tvUpdatedCb);
    };
  });

  $effect(() => {
    if (showId && (!show || show.id !== showId)) {
      loadShow(showId);
    }
  });
</script>

<svelte:head>
  <title>{show?.title ?? 'TV Show'} — Kobbler</title>
</svelte:head>

{#if loading && !show}
  <div class="loading-state">
    <p>Loading show...</p>
  </div>
{:else if error && !show}
  <div class="error-state">
    <p>{error}</p>
    <button type="button" class="btn-back" onclick={handleClose}>Back to TV Shows</button>
  </div>
{:else if show && editing}
  <ContentPageView
    noHeader={true}
    noPadding={true}
    pageBackdropUrl={show.backdrop_url || show.poster_url}
    onClose={handleClose}
  >
    {#snippet headerActions()}
      <button
        type="button"
        class="glass-btn"
        onclick={() => (showMetadataSearch = true)}
        title="Search Metadata"
      >
        <span class="material-icons-outlined">search</span>
      </button>
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
        <h2 class="edit-hero-title">Editing: {show.title}</h2>
      </div>
    </div>

    <div class="page-container">
      <TvShowForm
        {show}
        searchResults={editSearchResults}
        hideFooter
        bind:requestSave={triggerSave}
        bind:canSave={formValid}
        onSave={handleSave}
        onCancel={handleEditCancel}
        onDelete={handleDeleteShow}
        {saving}
        {deleting}
      />
    </div>

    {#snippet footer()}
      <div class="edit-footer">
        <button
          type="button"
          class="btn btn-danger"
          onclick={handleDeleteShow}
          disabled={deleting || saving}
        >
          <span class="material-icons-outlined">delete</span>
          {deleting ? 'Deleting...' : 'Delete'}
        </button>
        <div class="spacer"></div>

        <button type="button" class="btn btn-sec" onclick={handleEditCancel} disabled={saving}
          >Cancel</button
        >

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
  {#if showMetadataSearch}
    <VideoMetadataSearchModal
      mode="tv"
      initialTitle={show.title}
      onSelect={handleMetadataSearchSelect}
      onCancel={() => (showMetadataSearch = false)}
    />
  {/if}
{:else if show && addingSeason}
  <div class="edit-container">
    <h2>Add Season</h2>
    <SeasonForm
      onSave={handleSaveSeason}
      onCancel={() => (addingSeason = false)}
      saving={savingSeason}
    />
  </div>
{:else if show}
  <TvShowDetailPage
    {show}
    {syncing}
    onMonitorToggle={handleMonitorToggle}
    onWatchlistToggle={handleWatchlistToggle}
    onWantAllMissing={handleWantAllMissing}
    onUnwantAll={handleUnwantAll}
    onRefresh={handleRefresh}
    onWantSeason={handleWantSeason}
    onUnwantSeason={handleUnwantSeason}
    onWantEpisode={handleWantEpisode}
    onUnwantEpisode={handleUnwantEpisode}
    onWatchToggle={handleWatchToggle}
    onBulkWatchToggle={handleBulkShowWatch}
    onRewatch={handleRewatch}
    onBulkSeasonWatchToggle={handleBulkSeasonWatch}
    onSearchReleases={statsState.stats.kobload_configured ? handleSearchReleases : undefined}
    onClose={handleClose}
    onCastClick={handleCastClick}
    onGenreClick={(genre) => goto(`/tv?q=${encodeURIComponent(genre)}`)}
    onEdit={handleEdit}
    onAddSeason={handleAddSeason}
    onSeasonAutoWantChange={handleSeasonAutoWantChange}
    onReanalyze={handleReanalyze}
    onReload={handleReload}
  />
{/if}

{#if releaseSearchEpisodeId}
  <ReleaseSearchModal
    entityTitle={releaseSearchTitle}
    initialQuery={releaseSearchTitle}
    onClose={() => (releaseSearchEpisodeId = null)}
    searchFn={(query, scope) => api.searchEpisodeReleases(releaseSearchEpisodeId!, query, scope)}
    scopeLabel="TV"
    smartSearchFn={() => api.searchEpisodeReleases(releaseSearchEpisodeId!)}
    queueFn={async (release, source) => {
      await api.queueEpisodeRelease(releaseSearchEpisodeId!, release, source);
      if (show) await loadShow(show.id);
    }}
    wantFn={async () => {
      await api.wantEpisode(releaseSearchEpisodeId!);
      if (show) await loadShow(show.id);
    }}
  />
{/if}

<style>
  .loading-state,
  .error-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    min-height: 200px;
    color: var(--muted);
    gap: 16px;
  }

  .error-state p {
    color: var(--error, #ef4444);
  }

  .btn-back {
    padding: 8px 16px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--card);
    color: var(--ink);
    cursor: pointer;
    font-size: 13px;
  }

  .btn-back:hover {
    background: var(--bg-2);
  }

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

  /* Footer */
  .edit-footer {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
  }

  .spacer {
    flex: 1;
  }

  .edit-container {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    max-width: 100%;
  }

  .edit-container h2 {
    margin: 24px 0;
    font-size: 20px;
    font-weight: 700;
  }
</style>
