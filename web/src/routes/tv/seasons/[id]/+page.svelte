<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import ContentPageView from '$lib/components/ContentPageView.svelte';
  import EntityHero from '$lib/components/shared/EntityHero.svelte';
  import SeasonForm from '$lib/components/tv/SeasonForm.svelte';
  import type { SeasonFormData } from '$lib/components/tv/SeasonForm.svelte';
  import EpisodeForm from '$lib/components/tv/EpisodeForm.svelte';
  import type { EpisodeFormData } from '$lib/components/tv/EpisodeForm.svelte';
  import VideoMetadataFetchModal from '$lib/components/video/VideoMetadataFetchModal.svelte';
  import ReleaseSearchModal from '$lib/components/ReleaseSearchModal.svelte';
  import TvEpisodeRow from '$lib/components/tv/TvEpisodeRow.svelte';
  import EpisodeBatchActions from '$lib/components/tv/EpisodeBatchActions.svelte';
  import { EpisodeBulkSelection } from '$lib/utils/episode-bulk.svelte';
  import BulkSelectToggle from '$lib/components/shared/BulkSelectToggle.svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import { getStatsState } from '$lib/stores/stats.svelte';
  import * as api from '$lib/api/client';
  import { SearchSelect } from '@koblibs/ui';
  import { watchStateVersion } from '$lib/stores/watchState.svelte';
  import { formatEpisodeSearchTitle } from '$lib/utils/format';
  import { hasOutstandingWants, unwantMessage } from '$lib/utils/want-status';
  import type { TvSeason, TvShow, SeasonDetailResult } from '$lib/types';

  let season = $state<TvSeason | null>(null);
  let loading = $state(true);
  let editing = $state(false);
  let saving = $state(false);
  let deleting = $state(false);
  let addingEpisode = $state(false);
  let savingEpisode = $state(false);
  let showFetchMetadata = $state(false);
  let editSearchResults = $state<SeasonDetailResult[]>([]);
  let parentShow = $state<TvShow | null>(null);
  let formValid = $state(false);
  let triggerSave = $state<(() => void) | undefined>();
  let releaseSearchEpisodeId = $state<string | null>(null);

  const bulk = new EpisodeBulkSelection({
    onDone: async () => {
      if (season) await loadSeason(season.id);
    },
    notify: showMessage
  });

  const seasonId = $derived($page.params.id);
  const statsState = getStatsState();

  const releaseSearchTitle = $derived.by(() => {
    const episode = season?.episodes.find((e) => e.id === releaseSearchEpisodeId);
    if (!episode || !season) return 'Episode';
    return formatEpisodeSearchTitle(
      parentShow?.title,
      season.season_identifier,
      episode.episode_identifier
    );
  });

  async function loadSeason(id: string) {
    loading = true;
    try {
      season = await api.getSeason(id);
      // Load parent show eagerly for hero backdrop/logo
      if (season?.show_id && !parentShow) {
        try {
          parentShow = await api.getTvShow(season.show_id);
        } catch {
          // Non-critical
        }
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load season');
    } finally {
      loading = false;
    }
  }

  let lastWatchVersion = -1;
  $effect(() => {
    const v = watchStateVersion();
    if (lastWatchVersion !== -1 && v !== lastWatchVersion && season) loadSeason(season.id);
    lastWatchVersion = v;
  });

  const handleBack = () => {
    if (season?.show_id) {
      goto(`/tv/${season.show_id}`);
    } else {
      goto('/tv');
    }
  };

  async function handleWantSeason() {
    if (!season) return;
    try {
      await api.wantSeason(season.id);
      showMessage('Season episodes wanted');
      await loadSeason(season.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want season');
    }
  }

  async function handleUnwantSeason() {
    if (!season) return;
    const label = season.is_special ? 'Specials' : `Season ${season.season_identifier}`;
    if (
      !confirm(
        `Unwant every outstanding episode in ${label}? Episodes already in your library are unaffected.`
      )
    )
      return;
    try {
      const result = await api.unwantSeason(season.id);
      showMessage(unwantMessage(result, 'season'));
      await loadSeason(season.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant season');
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

  const handleEditDelete = async () => {
    if (!season) return;
    if (!confirm('Delete this season? This cannot be undone.')) return;
    deleting = true;
    try {
      await api.deleteSeason(season.id);
      showMessage('Season deleted');
      if (season.show_id) {
        goto(`/tv/${season.show_id}`);
      } else {
        goto('/tv');
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete season');
    } finally {
      deleting = false;
    }
  };

  const handleClose = () => handleBack();

  const handleSave = async (data: SeasonFormData, posterFile?: File) => {
    if (!season) return;
    saving = true;
    try {
      await api.updateSeason(season.id, data as unknown as Record<string, unknown>);
      if (posterFile) {
        await api.uploadSeasonPoster(season.id, posterFile);
      }
      showMessage('Season updated');
      editing = false;
      editSearchResults = [];
      await loadSeason(season.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update season');
    } finally {
      saving = false;
    }
  };

  const handleFetchMetadataSelect = (results: SeasonDetailResult[]) => {
    editSearchResults = results;
    showFetchMetadata = false;
  };

  const handleAddEpisode = () => {
    addingEpisode = true;
  };

  const handleReanalyze = async (force = false) => {
    if (!season) return;
    try {
      const result = await api.runMediaAnalyzeSeason(season.id, force);
      if (result.enqueued > 0) {
        const label = force ? 'Queued force re-analyze' : 'Queued re-analyze';
        showMessage(`${label} for ${result.enqueued} file${result.enqueued === 1 ? '' : 's'}`);
      } else {
        showMessage('No files to analyze in this season');
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to run media analysis');
    }
  };

  const handleSaveEpisode = async (data: EpisodeFormData) => {
    if (!season) return;
    savingEpisode = true;
    try {
      await api.createEpisode(season.id, data as Parameters<typeof api.createEpisode>[1]);
      showMessage('Episode created');
      addingEpisode = false;
      await loadSeason(season.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to create episode');
    } finally {
      savingEpisode = false;
    }
  };

  onMount(() => {
    if (seasonId) {
      loadSeason(seasonId);
    }
  });

  $effect(() => {
    if (seasonId && (!season || season.id !== seasonId)) {
      loadSeason(seasonId);
    }
  });

  const sortedEpisodes = $derived(
    season ? [...season.episodes].sort((a, b) => a.episode_sort_order - b.episode_sort_order) : []
  );

  const allWatched = $derived(
    sortedEpisodes.length > 0 && sortedEpisodes.every((e) => e.watched === true)
  );

  const anyWanted = $derived(hasOutstandingWants(sortedEpisodes));

  const metaSnippet = $derived(() => {
    const parts: string[] = [];
    if (season?.air_date) parts.push(season.air_date);
    const count = season?.episodes.length ?? 0;
    parts.push(`${count} episode${count !== 1 ? 's' : ''}`);
    return parts.join(' · ');
  });

  const autoWantOptions = [
    { value: 'inherit', label: 'Inherit' },
    { value: 'none', label: 'None' },
    { value: 'all', label: 'All Episodes' },
    { value: 'new_only', label: 'New Only' },
    { value: 'fill_gaps', label: 'Fill Gaps' }
  ];

  const autoWantMode = $derived(season?.auto_want_mode || 'inherit');

  const handleBulkSeasonWatch = async (watched: boolean) => {
    if (!season) return;
    try {
      const result = await api.bulkUpdateSeasonWatchState(season.id, watched);
      showMessage(
        `Marked ${result.updated_count} episode${result.updated_count === 1 ? '' : 's'} as ${watched ? 'watched' : 'unwatched'}`
      );
      await loadSeason(season.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update watch state');
    }
  };

  const handleWantEpisode = async (episodeId: string) => {
    try {
      await api.wantEpisode(episodeId);
      showMessage('Episode wanted');
      if (season) await loadSeason(season.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want episode');
    }
  };

  const handleUnwantEpisode = async (episodeId: string) => {
    try {
      await api.unwantEpisode(episodeId);
      showMessage('Episode removed from wanted');
      if (season) await loadSeason(season.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant episode');
    }
  };

  const handleEpisodeWatchToggle = async (episodeId: string) => {
    try {
      const ep = season?.episodes.find((e) => e.id === episodeId);
      const isWatched = ep?.watched ?? false;
      await api.updateEpisodeWatchState(episodeId, {
        watched: !isWatched,
        watched_at: !isWatched ? new Date().toISOString() : null
      });
      showMessage(!isWatched ? 'Marked as watched' : 'Marked as unwatched');
      if (season) await loadSeason(season.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to toggle watch state');
    }
  };

  const handleSearchReleases = (episodeId: string) => {
    releaseSearchEpisodeId = episodeId;
  };

  const handleAutoWantChange = async (value: string) => {
    if (!season) return;
    try {
      await api.setSeasonAutoWant(season.id, value);
      showMessage('Season auto-want updated');
      await loadSeason(season.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update season auto-want');
    }
  };
</script>

<svelte:head>
  <title>{season?.title || `Season ${season?.season_sort_order ?? ''}`} — Kobbler</title>
</svelte:head>

{#if loading && !season}
  <div class="loading-state"><p>Loading season...</p></div>
{:else if season && editing}
  <ContentPageView
    noHeader={true}
    noPadding={true}
    pageBackdropUrl={parentShow?.backdrop_url || season.poster_url}
    onClose={handleClose}
  >
    {#snippet headerActions()}
      {#if parentShow?.identifiers && Object.keys(parentShow.identifiers).length > 0}
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
          Editing: {season.title || `Season ${season.season_sort_order}`}
        </h2>
      </div>
    </div>

    <div class="page-container">
      <SeasonForm
        {season}
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

  {#if showFetchMetadata && parentShow?.identifiers}
    <VideoMetadataFetchModal
      showIdentifiers={parentShow.identifiers}
      seasonNumber={season.season_sort_order}
      onSelect={handleFetchMetadataSelect}
      onCancel={() => (showFetchMetadata = false)}
    />
  {/if}
{:else if season && addingEpisode}
  <ContentPageView
    noHeader={true}
    noPadding={true}
    pageBackdropUrl={parentShow?.backdrop_url || season.poster_url}
    onClose={() => (addingEpisode = false)}
  >
    {#snippet headerActions()}
      <button type="button" class="glass-btn" onclick={() => (addingEpisode = false)} title="Close">
        <span class="material-icons-outlined">close</span>
      </button>
    {/snippet}

    <div class="edit-hero">
      <div class="edit-hero-content">
        <h2 class="edit-hero-title">Add Episode</h2>
      </div>
    </div>

    <div class="page-container">
      <EpisodeForm
        createMode
        onSave={handleSaveEpisode}
        onCancel={() => (addingEpisode = false)}
        saving={savingEpisode}
      />
    </div>
  </ContentPageView>
{:else if season}
  <ContentPageView
    noHeader={true}
    noPadding={true}
    pageBackdropUrl={parentShow?.backdrop_url || season.poster_url}
    onBack={handleBack}
    onClose={handleBack}
    onEdit={handleEdit}
  >
    <EntityHero
      title={season.title || `Season ${season.season_sort_order}`}
      subtitle={parentShow?.title}
      coverUrl={season.poster_url}
      backdropUrl={parentShow?.backdrop_url || undefined}
      logoUrl={parentShow?.logo_url}
      description={season.description}
      hideRating={true}
    >
      {#snippet meta()}
        <div class="hero-meta-snippet">
          {metaSnippet()}
        </div>
      {/snippet}

      {#snippet actions()}
        <button class="btn btn-primary" type="button" onclick={handleWantSeason}>
          <span class="material-icons-outlined">bookmark_add</span>
          Want Missing
        </button>
        {#if anyWanted}
          <button
            class="btn btn-outline"
            type="button"
            title="Drop every outstanding want in this season"
            onclick={handleUnwantSeason}
          >
            <span class="material-icons-outlined">playlist_remove</span>
            Unwant All
          </button>
        {/if}
        <button
          class="btn btn-outline"
          type="button"
          onclick={() => handleBulkSeasonWatch(!allWatched)}
        >
          <span class="material-icons-outlined">
            {allWatched ? 'visibility_off' : 'visibility'}
          </span>
          {allWatched ? 'Mark Unwatched' : 'Mark Watched'}
        </button>
      {/snippet}
    </EntityHero>

    <div class="full-page-main-grid page-container">
      <div class="details-column">
        <div class="card-section">
          <div class="section-head">
            <div class="label">Episodes</div>
            {#if sortedEpisodes.length > 0}
              <BulkSelectToggle
                active={bulk.active}
                title="Select episodes"
                onToggle={() => bulk.toggleMode()}
              />
            {/if}
          </div>
          {#if sortedEpisodes.length === 0}
            <p class="muted">No episodes available.</p>
          {:else}
            {#if bulk.active}
              <EpisodeBatchActions
                selectedCount={bulk.count}
                totalCount={sortedEpisodes.length}
                loading={bulk.busy}
                onSelectAll={() => bulk.selectMany(sortedEpisodes)}
                onClearSelection={() => bulk.clear()}
                onApply={(action) => bulk.apply(action)}
              />
            {/if}
            <div class="episodes-list">
              {#each sortedEpisodes as episode (episode.id)}
                <TvEpisodeRow
                  {episode}
                  href="/tv/episodes/{episode.id}"
                  onWant={() => handleWantEpisode(episode.id)}
                  onUnwant={() => handleUnwantEpisode(episode.id)}
                  onWatchToggle={() => handleEpisodeWatchToggle(episode.id)}
                  onSearchReleases={statsState.stats.kobload_configured
                    ? () => handleSearchReleases(episode.id)
                    : undefined}
                  selectionMode={bulk.active}
                  selected={bulk.has(episode.id)}
                  onToggleSelect={(shiftKey: boolean) =>
                    bulk.toggle(episode.id, sortedEpisodes, shiftKey)}
                />
              {/each}
            </div>
          {/if}
        </div>
      </div>

      <div class="actions-column">
        <div class="card-section">
          <div class="label">Manage</div>
          <div class="manage-actions">
            <div class="manage-row">
              <span class="manage-label">Auto-want</span>
              <div class="auto-want-wrapper">
                <SearchSelect
                  items={autoWantOptions}
                  value={autoWantMode}
                  onchange={handleAutoWantChange}
                />
              </div>
            </div>
            <button class="btn btn-outline full-width" type="button" onclick={handleAddEpisode}>
              <span class="material-icons-outlined">add</span>
              Add Episode
            </button>
            <button
              class="btn btn-outline full-width"
              type="button"
              onclick={() => handleReanalyze(false)}
            >
              <span class="material-icons-outlined">science</span>
              Re-analyze Media
            </button>
            <button
              class="btn btn-outline full-width"
              type="button"
              onclick={() => handleReanalyze(true)}
              title="Rebuild cache from scratch, ignoring existing manifest"
            >
              <span class="material-icons-outlined">autorenew</span>
              Force Re-analyze
            </button>
          </div>
        </div>

        <div class="card-section">
          <div class="label">Stats</div>
          <div class="stats-grid">
            <div class="stat">
              <span class="stat-value">{season.episodes.length}</span>
              <span class="stat-label">Episodes</span>
            </div>
            <div class="stat">
              <span class="stat-value">{season.episodes.filter((e) => e.is_in_library).length}</span
              >
              <span class="stat-label">In Library</span>
            </div>
            <div class="stat">
              <span class="stat-value"
                >{season.episodes.filter((e) => e.watched === true).length}</span
              >
              <span class="stat-label">Watched</span>
            </div>
            <div class="stat">
              <span class="stat-value"
                >{season.episodes.filter((e) => e.want_status === 'wanted').length}</span
              >
              <span class="stat-label">Wanted</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  </ContentPageView>
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
      if (season) await loadSeason(season.id);
    }}
    wantFn={async () => {
      await api.wantEpisode(releaseSearchEpisodeId!);
      if (season) await loadSeason(season.id);
    }}
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
    grid-template-columns: 1fr 340px;
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

  .hero-meta-snippet {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--muted);
    font-size: 14px;
    font-weight: 500;
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

  .episodes-list {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .manage-actions {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .manage-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  .manage-label {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
  }

  .auto-want-wrapper {
    min-width: 140px;
  }

  .full-width {
    width: 100%;
    justify-content: center;
  }

  .stats-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 12px;
  }

  .stat {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
  }

  .stat-value {
    font-size: 20px;
    font-weight: 800;
    color: var(--ink);
  }

  .stat-label {
    font-size: 11px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
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

  @media (max-width: 1100px) {
    .full-page-main-grid {
      grid-template-columns: 1fr;
    }

    .actions-column {
      order: -1;
    }
  }

  /* Phone widths: trim the section padding so episode rows have room. */
  @media (max-width: 600px) {
    .card-section {
      padding: 16px 10px;
    }
  }

  .section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
</style>
