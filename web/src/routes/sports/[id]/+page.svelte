<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import ContentPageView from '$lib/components/ContentPageView.svelte';
  import TvShowForm from '$lib/components/tv/TvShowForm.svelte';
  import type { TvShowFormData } from '$lib/components/tv/TvShowForm.svelte';
  import SeasonForm from '$lib/components/tv/SeasonForm.svelte';
  import type { SeasonFormData } from '$lib/components/tv/SeasonForm.svelte';
  import SportsLeagueDetailPage from '$lib/components/sports/SportsLeagueDetailPage.svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import * as api from '$lib/api/client';
  import { on, off } from '$lib/services/eventstream.svelte';
  import { getJobsState } from '$lib/stores/jobs.svelte';
  import type { TvShow, TvSeason } from '$lib/types';
  import { unwantMessage } from '$lib/utils/want-status';

  let league = $state<TvShow | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let editing = $state(false);
  let saving = $state(false);
  let deleting = $state(false);
  let addingSeason = $state(false);
  let savingSeason = $state(false);
  let triggerSave = $state<(() => void) | undefined>(undefined);
  let formValid = $state(false);

  // Paginated seasons
  let seasons = $state<TvSeason[]>([]);
  let seasonsLoading = $state(false);
  let totalSeasons = $state(0);
  let seasonsOffset = $state(0);
  const SEASONS_LIMIT = 20;

  const leagueId = $derived($page.params.id);
  const jobsState = getJobsState();
  const syncing = $derived(
    jobsState.jobs.some(
      (j) =>
        j.entity_type === 'tv_show' &&
        j.entity_id === leagueId &&
        j.job_type === 'sports_league_enrich' &&
        (j.status === 'pending' || j.status === 'running')
    )
  );

  async function loadSeasons(leagueId: string, reset = false) {
    if (reset) {
      seasonsOffset = 0;
      seasons = [];
    }
    seasonsLoading = true;
    try {
      const result = await api.getSportsLeagueSeasons(leagueId, {
        limit: SEASONS_LIMIT,
        offset: seasonsOffset,
        sort: 'desc'
      });
      if (reset) {
        seasons = result.items;
      } else {
        seasons = [...seasons, ...result.items];
      }
      totalSeasons = result.total_count;
    } catch (e) {
      console.error('Failed to load seasons:', e);
    } finally {
      seasonsLoading = false;
    }
  }

  function loadMoreSeasons() {
    if (!league) return;
    seasonsOffset += SEASONS_LIMIT;
    loadSeasons(league.id);
  }

  const hasMoreSeasons = $derived(seasons.length < totalSeasons);

  async function loadAllSeasons() {
    if (!league) return;
    while (seasons.length < totalSeasons) {
      const before = seasons.length;
      seasonsOffset = before;
      await loadSeasons(league.id);
      if (seasons.length === before) break;
    }
  }

  const loadLeague = async (id: string) => {
    loading = true;
    error = null;
    try {
      league = await api.getSportsLeague(id);
      await loadSeasons(id, true);
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load league';
      showMessage(error);
    } finally {
      loading = false;
    }
  };

  const handleMonitorToggle = async (monitored: boolean, mode: string) => {
    if (!league) return;
    try {
      await api.setSportsLeagueMonitored(league.id, monitored, mode);
      showMessage(monitored ? 'League monitored' : 'League unmonitored');
      await loadLeague(league.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update monitoring');
    }
  };

  const handleWantAllMissing = async () => {
    if (!league) return;
    try {
      await api.wantAllMissingSports(league.id);
      showMessage('Wanted all missing events');
      await loadLeague(league.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want missing events');
    }
  };

  const handleUnwantAll = async () => {
    if (!league) return;
    if (
      !confirm(
        `Unwant every outstanding event in "${league.title}"? Events already in your library are unaffected.`
      )
    )
      return;
    try {
      const result = await api.unwantAllSports(league.id);
      showMessage(unwantMessage(result, 'league', 'event'));
      await loadLeague(league.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant events');
    }
  };

  const watchedMessage = (n: number, watched: boolean) =>
    `Marked ${n} event${n === 1 ? '' : 's'} as ${watched ? 'watched' : 'unwatched'}`;

  const handleBulkWatchToggle = async (watched: boolean) => {
    if (!league) return;
    try {
      const result = await api.bulkUpdateShowWatchState(league.id, watched);
      showMessage(watchedMessage(result.updated_count, watched));
      await loadLeague(league.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update watch state');
    }
  };

  const handleSeasonWatchToggle = async (seasonId: string, watched: boolean) => {
    try {
      const result = await api.bulkUpdateSeasonWatchState(seasonId, watched);
      showMessage(watchedMessage(result.updated_count, watched));
      if (league) await loadLeague(league.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update watch state');
    }
  };

  const handleRefresh = async () => {
    if (!league) return;
    try {
      await api.refreshSportsMetadata(league.id);
      showMessage('Metadata refresh queued');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to refresh metadata');
    }
  };

  const handleWantSeason = async (seasonId: string) => {
    try {
      await api.wantSportsSeason(seasonId);
      showMessage('Season events wanted');
      if (league) await loadLeague(league.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want season');
    }
  };

  const handleUnwantSeason = async (seasonId: string) => {
    if (
      !confirm(
        'Unwant every outstanding event in this season? Events already in your library are unaffected.'
      )
    )
      return;
    try {
      const result = await api.unwantSportsSeason(seasonId);
      showMessage(unwantMessage(result, 'season', 'event'));
      if (league) await loadLeague(league.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant season');
    }
  };

  const handleSeasonAutoWantChange = async (seasonId: string, mode: string) => {
    try {
      await api.setSportsSeasonAutoWant(seasonId, mode);
      showMessage('Season auto-want updated');
      if (league) await loadLeague(league.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update season auto-want');
    }
  };

  const handleUpdateSportMetadata = async (metadata: Record<string, unknown>) => {
    if (!league) return;
    try {
      await api.updateSportsLeague(league.id, { sport_metadata: metadata });
      showMessage('Search settings saved');
      await loadLeague(league.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to save search settings');
    }
  };

  const handleEdit = () => {
    editing = true;
  };

  const handleEditCancel = () => {
    editing = false;
  };

  const handleSave = async (
    data: TvShowFormData,
    posterFile?: File,
    backdropFile?: File,
    logoFile?: File
  ) => {
    if (!league) return;
    saving = true;
    try {
      const { cast: _cast, crew: _crew, ...metadata } = data;
      await api.updateSportsLeague(league.id, metadata as unknown as Record<string, unknown>);
      if (posterFile) await api.uploadSportsLeaguePoster(league.id, posterFile);
      if (backdropFile) await api.uploadSportsLeagueBackdrop(league.id, backdropFile);
      if (logoFile) await api.uploadSportsLeagueLogo(league.id, logoFile);
      showMessage('League updated');
      editing = false;
      await loadLeague(league.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update league');
    } finally {
      saving = false;
    }
  };

  const handleDelete = async () => {
    if (!league) return;
    if (!confirm(`Delete "${league.title}"? This cannot be undone.`)) return;
    deleting = true;
    try {
      await api.deleteSportsLeague(league.id);
      showMessage('League deleted');
      goto('/sports');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete league');
    } finally {
      deleting = false;
    }
  };

  const handleClose = () => {
    goto('/sports');
  };

  const handleAddSeason = () => {
    addingSeason = true;
  };

  const handleSaveSeason = async (data: SeasonFormData) => {
    if (!league) return;
    savingSeason = true;
    try {
      await api.createSportsSeason(league.id, {
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
      await loadLeague(league.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to create season');
    } finally {
      savingSeason = false;
    }
  };

  // SSE: auto-refresh when enrichment completes
  const tvUpdatedCb = on('tv_show_updated', (data: { show_id: string }) => {
    if (data.show_id === leagueId) {
      loadLeague(leagueId);
    }
  });

  onMount(() => {
    if (leagueId) loadLeague(leagueId);
    return () => {
      off('tv_show_updated', tvUpdatedCb);
    };
  });

  $effect(() => {
    if (leagueId && (!league || league.id !== leagueId)) {
      loadLeague(leagueId);
    }
  });
</script>

<svelte:head>
  <title>{league?.title ?? 'League'} — Kobbler</title>
</svelte:head>

{#if loading && !league}
  <div class="loading-state">
    <p>Loading league...</p>
  </div>
{:else if error && !league}
  <div class="error-state">
    <p>{error}</p>
    <button type="button" class="btn-back" onclick={handleClose}>Back to Sports</button>
  </div>
{:else if league && editing}
  <ContentPageView
    noHeader={true}
    noPadding={true}
    pageBackdropUrl={league.backdrop_url || league.poster_url}
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
        <h2 class="edit-hero-title">Editing: {league.title}</h2>
      </div>
    </div>

    <div class="page-container">
      <TvShowForm
        show={league}
        hideFooter
        hideCastCrew
        mediaType="sport"
        bind:requestSave={triggerSave}
        bind:canSave={formValid}
        onSave={handleSave}
        onCancel={handleEditCancel}
        onDelete={handleDelete}
        {saving}
        {deleting}
      />
    </div>

    {#snippet footer()}
      <div class="edit-footer">
        <button
          type="button"
          class="btn btn-danger"
          onclick={handleDelete}
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
{:else if league && addingSeason}
  <div class="edit-container">
    <h2>Add Season</h2>
    <SeasonForm
      onSave={handleSaveSeason}
      onCancel={() => (addingSeason = false)}
      saving={savingSeason}
    />
  </div>
{:else if league}
  <SportsLeagueDetailPage
    {league}
    {syncing}
    {seasons}
    {seasonsLoading}
    {hasMoreSeasons}
    {totalSeasons}
    onLoadAllSeasons={loadAllSeasons}
    onBulkWatchToggle={handleBulkWatchToggle}
    onSeasonWatchToggle={handleSeasonWatchToggle}
    onMonitorToggle={handleMonitorToggle}
    onWantAllMissing={handleWantAllMissing}
    onUnwantAll={handleUnwantAll}
    onRefresh={handleRefresh}
    onClose={handleClose}
    onEdit={handleEdit}
    onAddSeason={handleAddSeason}
    onWantSeason={handleWantSeason}
    onUnwantSeason={handleUnwantSeason}
    onSeasonAutoWantChange={handleSeasonAutoWantChange}
    onLoadMore={loadMoreSeasons}
    onUpdateSportMetadata={handleUpdateSportMetadata}
    onReload={() => loadLeague(league!.id)}
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

  .btn-sec {
    background: var(--bg-2);
    border: 1px solid var(--ring);
    color: var(--ink);
  }

  .btn-danger {
    background: var(--error, #ef4444);
    color: #fff;
  }

  .page-container {
    position: relative;
    z-index: 1;
  }

  .page-container :global(.form-inner) {
    padding-top: 20px;
  }
</style>
