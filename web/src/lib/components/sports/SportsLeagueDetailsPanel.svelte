<script lang="ts">
  import type { TvShow, TvSeason } from '#lib/types.js';
  import { getSportsLeagueSeasons } from '#lib/api/client.js';
  import { SearchSelect } from '@koblibs/ui';
  import { InfiniteScroll } from '@koblibs/ui';
  import EntityHero from '../shared/EntityHero.svelte';

  interface Props {
    league: TvShow;
    onMonitorToggle: (monitored: boolean, mode: string) => void;
    onAutoWantChange: (mode: string) => void;
    onWantAllMissing: () => void;
    onUnwantAll?: () => void;
    onRefreshMetadata?: () => void;
    onSeasonClick?: (seasonId: string) => void;
  }

  let {
    league,
    onMonitorToggle,
    onAutoWantChange,
    onWantAllMissing,
    onUnwantAll,
    onRefreshMetadata,
    onSeasonClick
  }: Props = $props();

  let descriptionExpanded = $state(false);
  let seasons = $state<TvSeason[]>([]);
  // Only the loaded page of seasons carries a count; see SportsLeagueDetailPage.
  const anyWanted = $derived(seasons.some((s) => (s.wanted_event_count ?? 0) > 0));
  let seasonsLoading = $state(false);
  let totalSeasons = $state(0);
  let seasonsOffset = $state(0);
  const SEASONS_LIMIT = 20;

  async function loadSeasons(reset = false) {
    if (reset) {
      seasonsOffset = 0;
      seasons = [];
    }
    seasonsLoading = true;
    try {
      const result = await getSportsLeagueSeasons(league.id, {
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
    seasonsOffset += SEASONS_LIMIT;
    loadSeasons();
  }

  $effect(() => {
    league.id;
    loadSeasons(true);
  });

  const hasMoreSeasons = $derived(seasons.length < totalSeasons);
  const descriptionNeedsExpand = $derived(!!league.description && league.description.length > 200);
  const autoWantMode = $derived(league.auto_want_mode || 'none');

  const autoWantOptions = [
    { value: 'none', label: 'None' },
    { value: 'all', label: 'All Events' },
    { value: 'new_only', label: 'New Only' },
    { value: 'fill_gaps', label: 'Fill Gaps' }
  ];
</script>

<div class="league-detail">
  <EntityHero
    title={league.title}
    coverUrl={league.poster_url}
    backdropUrl={league.backdrop_url}
    logoUrl={league.logo_url}
    embedded={true}
  >
    {#snippet subtitle()}
      <div class="panel-meta-row">
        {#if league.year}
          <span class="panel-meta-item">{league.year}</span>
        {/if}
        {#if league.status}
          <span class="meta-item status-pill">{league.status}</span>
        {/if}
        {#if league.network}
          <span class="panel-meta-item">{league.network}</span>
        {/if}
      </div>
    {/snippet}

    {#snippet actions()}
      <button
        class="monitor-pill"
        class:active={league.is_monitored}
        type="button"
        onclick={() => onMonitorToggle(!league.is_monitored, autoWantMode)}
      >
        <span class="material-icons-outlined">
          {league.is_monitored ? 'star' : 'star_outline'}
        </span>
        {league.is_monitored ? 'Monitored' : 'Monitor'}
      </button>
    {/snippet}
  </EntityHero>

  <!-- Auto-want & Actions -->
  <div class="panel-section auto-want-section">
    <div class="auto-want-row">
      <div class="auto-want-select">
        <h4 class="panel-section-title">Auto-want</h4>
        <SearchSelect value={autoWantMode} items={autoWantOptions} onchange={onAutoWantChange} />
      </div>
      <button
        class="icon-btn small"
        type="button"
        title="Want all missing events"
        onclick={onWantAllMissing}
      >
        <span class="material-icons-outlined">playlist_add</span>
      </button>
      {#if onUnwantAll && anyWanted}
        <button
          class="icon-btn small"
          type="button"
          title="Unwant every outstanding event"
          onclick={onUnwantAll}
        >
          <span class="material-icons-outlined">playlist_remove</span>
        </button>
      {/if}
      {#if onRefreshMetadata}
        <button
          class="icon-btn small"
          type="button"
          title="Refresh metadata"
          onclick={onRefreshMetadata}
        >
          <span class="material-icons-outlined">refresh</span>
        </button>
      {/if}
    </div>
  </div>

  <!-- Overview -->
  {#if league.description}
    <div class="panel-section">
      <h4 class="panel-section-title">Overview</h4>
      <p class="panel-overview" class:collapsed={!descriptionExpanded && descriptionNeedsExpand}>
        {league.description}
      </p>
      {#if descriptionNeedsExpand}
        <button
          class="panel-expand-link"
          type="button"
          onclick={() => (descriptionExpanded = !descriptionExpanded)}
        >
          {descriptionExpanded ? 'Show less' : 'Show more'}
        </button>
      {/if}
    </div>
  {/if}

  <!-- Seasons -->
  <div class="panel-section">
    <h4 class="panel-section-title">Seasons ({totalSeasons})</h4>
    {#if seasons.length > 0}
      <div class="seasons-list">
        {#each seasons as season (season.id)}
          <button class="season-item" type="button" onclick={() => onSeasonClick?.(season.id)}>
            {#if season.poster_url}
              <img class="season-poster" src={season.poster_url} alt="" />
            {:else}
              <div class="season-poster-placeholder">{season.season_sort_order}</div>
            {/if}
            <div class="season-info">
              <span class="season-title"
                >{season.title || `Season ${season.season_sort_order}`}</span
              >
              <span class="season-meta">
                {season.actual_episode_count ?? season.episode_count ?? 0} events
              </span>
            </div>
            <span class="material-icons-outlined chevron">chevron_right</span>
          </button>
        {/each}
      </div>
      <InfiniteScroll
        hasMore={hasMoreSeasons}
        loading={seasonsLoading}
        onLoadMore={loadMoreSeasons}
      />
    {:else if seasonsLoading}
      <div class="loading-text">Loading seasons...</div>
    {:else}
      <div class="empty-text">No seasons yet</div>
    {/if}
  </div>
</div>

<style>
  .league-detail {
    display: flex;
    flex-direction: column;
    gap: 0;
  }

  .status-pill {
    padding: 0 5px;
    border-radius: 3px;
    font-weight: 600;
    text-transform: capitalize;
    background: var(--bg-2);
    font-size: 10px;
  }

  .monitor-pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    border-radius: 100px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    font-size: 11px;
    font-weight: 700;
    cursor: pointer;
    transition: all 0.15s;
  }

  .monitor-pill:hover {
    background: rgba(139, 92, 246, 0.15);
    border-color: var(--monitored);
    color: var(--monitored);
  }

  .monitor-pill.active {
    background: rgba(139, 92, 246, 0.15);
    border-color: var(--monitored);
    color: var(--monitored);
  }

  .monitor-pill :global(.material-icons-outlined) {
    font-size: 14px;
  }

  .auto-want-section {
    padding: 12px 0;
  }

  .auto-want-row {
    display: flex;
    align-items: flex-end;
    gap: 8px;
  }

  .auto-want-select {
    flex: 1;
    min-width: 0;
  }

  .icon-btn.small {
    width: 28px;
    height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    flex-shrink: 0;
  }

  .icon-btn.small:hover {
    background: var(--bg);
    color: var(--ink);
  }

  .icon-btn.small :global(.material-icons-outlined) {
    font-size: 16px;
  }

  .seasons-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .season-item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px;
    border: none;
    background: transparent;
    cursor: pointer;
    text-align: left;
    border-radius: 8px;
    transition: background 0.15s;
    color: inherit;
    font: inherit;
    width: 100%;
  }

  .season-item:hover {
    background: var(--bg-2);
  }

  .season-poster {
    width: 40px;
    height: 56px;
    border-radius: 4px;
    object-fit: cover;
    flex-shrink: 0;
  }

  .season-poster-placeholder {
    width: 40px;
    height: 56px;
    border-radius: 4px;
    background: var(--bg-2);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 12px;
    font-weight: 700;
    color: var(--muted);
    flex-shrink: 0;
  }

  .season-info {
    display: flex;
    flex-direction: column;
    gap: 1px;
    flex: 1;
    min-width: 0;
  }

  .season-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
  }

  .season-meta {
    font-size: 12px;
    color: var(--muted);
  }

  .chevron {
    font-size: 18px;
    color: var(--muted);
  }

  .loading-text,
  .empty-text {
    font-size: 13px;
    color: var(--muted);
    text-align: center;
    padding: 16px 0;
  }
</style>
