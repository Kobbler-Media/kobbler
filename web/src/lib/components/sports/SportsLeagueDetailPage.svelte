<script lang="ts">
  import { untrack } from 'svelte';
  import type { TvShow, TvSeason } from '$lib/types';
  import ContentPageView from '$lib/components/ContentPageView.svelte';
  import EntityHero from '../shared/EntityHero.svelte';
  import HeroActions from '../shared/HeroActions.svelte';
  import MetaLine from '../shared/MetaLine.svelte';
  import GenreChips from '$lib/components/GenreChips.svelte';
  import { SearchSelect } from '@koblibs/ui';
  import { InfiniteScroll } from '@koblibs/ui';
  import { getSportsSeason } from '$lib/api/client';
  import { showMessage } from '$lib/context/app.svelte';
  import { EpisodeBulkSelection } from '$lib/utils/episode-bulk.svelte';
  import EpisodeBatchActions from '../tv/EpisodeBatchActions.svelte';
  import BulkSelectToggle from '../shared/BulkSelectToggle.svelte';
  import SeasonActions from '../shared/SeasonActions.svelte';

  interface Props {
    league: TvShow;
    syncing?: boolean;
    seasons: TvSeason[];
    seasonsLoading: boolean;
    hasMoreSeasons: boolean;
    totalSeasons: number;
    onLoadAllSeasons: () => Promise<void>;
    onBulkWatchToggle: (watched: boolean) => void;
    onSeasonWatchToggle: (seasonId: string, watched: boolean) => void;
    onMonitorToggle: (monitored: boolean, mode: string) => void;
    onWantAllMissing: () => void;
    onUnwantAll: () => void;
    onRefresh: () => void;
    onClose: () => void;
    onBack?: () => void;
    onEdit?: () => void;
    onAddSeason?: () => void;
    onWantSeason: (seasonId: string) => void;
    onUnwantSeason: (seasonId: string) => void;
    onSeasonAutoWantChange: (seasonId: string, mode: string) => void;
    onLoadMore: () => void;
    onUpdateSportMetadata?: (metadata: Record<string, unknown>) => void;
    onReload: () => Promise<void> | void;
  }

  let {
    league,
    syncing = false,
    seasons,
    seasonsLoading,
    hasMoreSeasons,
    totalSeasons,
    onLoadAllSeasons,
    onBulkWatchToggle,
    onSeasonWatchToggle,
    onMonitorToggle,
    onWantAllMissing,
    onUnwantAll,
    onRefresh,
    onClose,
    onBack,
    onEdit,
    onAddSeason,
    onWantSeason,
    onUnwantSeason,
    onSeasonAutoWantChange,
    onLoadMore,
    onUpdateSportMetadata,
    onReload
  }: Props = $props();

  const bulk = new EpisodeBulkSelection({
    onDone: () => onReload(),
    notify: showMessage,
    resolve: async (seasonIds) => {
      const loaded = await Promise.all(seasonIds.map((id) => getSportsSeason(id)));
      return loaded.flatMap((season) => season.episodes.map((e) => e.id));
    },
    noun: 'event'
  });

  const sportMeta = $derived(league.sport_metadata ?? {});
  const eventCount = $derived(league.event_count ?? 0);
  const anyWanted = $derived((league.wanted_event_count ?? 0) > 0);
  const allWatched = $derived(eventCount > 0 && league.watched_event_count === eventCount);

  const MODE_LABELS: Record<string, string> = {
    none: 'None',
    all: 'All',
    new_only: 'New Only',
    fill_gaps: 'Fill Gaps'
  };

  async function selectAllSeasons() {
    await onLoadAllSeasons();
    bulk.selectMany(seasons);
  }
  let searchTitle = $state(untrack(() => (sportMeta.search_title as string) || ''));
  let locationAliasesText = $state(
    untrack(() => {
      const aliases = sportMeta.location_aliases;
      if (aliases && typeof aliases === 'object') {
        return Object.entries(aliases as Record<string, string>)
          .map(([k, v]) => `${k}=${v}`)
          .join('\n');
      }
      return '';
    })
  );

  function saveSearchSettings() {
    const aliases: Record<string, string> = {};
    for (const line of locationAliasesText.split('\n')) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      const eq = trimmed.indexOf('=');
      if (eq > 0) {
        aliases[trimmed.slice(0, eq).trim()] = trimmed.slice(eq + 1).trim();
      }
    }
    const metadata = {
      ...sportMeta,
      search_title: searchTitle.trim() || null,
      location_aliases: Object.keys(aliases).length > 0 ? aliases : null
    };
    onUpdateSportMetadata?.(metadata);
  }

  const autoWantMode = $derived(league.auto_want_mode || 'none');

  const autoWantOptions = [
    { value: 'none', label: 'None' },
    { value: 'all', label: 'All Events' },
    { value: 'new_only', label: 'New Only' },
    { value: 'fill_gaps', label: 'Fill Gaps' }
  ];

  const seasonAutoWantOptions = [
    { value: 'inherit', label: 'Inherit' },
    { value: 'none', label: 'None' },
    { value: 'all', label: 'All Events' },
    { value: 'new_only', label: 'New Only' },
    { value: 'fill_gaps', label: 'Fill Gaps' }
  ];

  const handleMonitorChange = () => {
    onMonitorToggle(!league.is_monitored, autoWantMode);
  };

  const handleAutoWantChange = (value: string) => {
    onMonitorToggle(league.is_monitored, value);
  };
</script>

<ContentPageView
  noHeader={true}
  noPadding={true}
  pageBackdropUrl={league.backdrop_url || league.poster_url}
  {onBack}
  {onClose}
  onRefresh={() => onRefresh()}
  {onEdit}
  {syncing}
>
  <EntityHero
    title={league.title}
    coverUrl={league.poster_url}
    backdropUrl={league.backdrop_url || undefined}
    logoUrl={league.logo_url}
    description={league.description}
    rating={league.rating}
    ratingCount={league.rating_count}
  >
    {#snippet meta()}
      <MetaLine items={[league.network, league.status, eventCount > 0 && `${eventCount} events`]} />
      {#if syncing}
        <span class="syncing-indicator">
          <span class="material-icons-outlined spin">sync</span> Syncing
        </span>
      {/if}
    {/snippet}

    {#snippet actions()}
      <HeroActions
        actions={[
          {
            label: league.is_monitored ? 'Monitored' : 'Monitor',
            icon: league.is_monitored ? 'star' : 'star_outline',
            active: league.is_monitored,
            pinned: true,
            onSelect: handleMonitorChange
          },
          { label: 'Want Missing', icon: 'library_add', onSelect: onWantAllMissing },
          anyWanted && {
            label: 'Unwant All',
            icon: 'playlist_remove',
            title: 'Drop every outstanding want on this league',
            onSelect: onUnwantAll
          },
          {
            label: allWatched ? 'Mark Unwatched' : 'Mark Watched',
            icon: allWatched ? 'visibility_off' : 'visibility',
            onSelect: () => onBulkWatchToggle(!allWatched)
          },
          onAddSeason && { label: 'Add Season', icon: 'add', onSelect: onAddSeason }
        ]}
        menuItems={autoWantOptions.map((o) => ({
          section: 'Auto-want',
          label: o.label,
          checked: autoWantMode === o.value,
          onSelect: () => handleAutoWantChange(o.value)
        }))}
      />
    {/snippet}
  </EntityHero>

  <div class="full-page-main-grid page-container">
    <div class="details-column">
      <div class="section card-section">
        <div class="section-head">
          <div class="label">Seasons ({totalSeasons})</div>
          {#if seasons.length > 0}
            <BulkSelectToggle
              active={bulk.active}
              title="Select seasons"
              onToggle={() => bulk.toggleMode()}
            />
          {/if}
        </div>
        {#if seasons.length > 0}
          {#if bulk.active}
            <EpisodeBatchActions
              selectedCount={bulk.count}
              totalCount={totalSeasons}
              loading={bulk.busy}
              noun="event"
              onSelectAll={selectAllSeasons}
              onClearSelection={() => bulk.clear()}
              onApply={(action) => bulk.apply(action)}
            />
          {/if}
          <div class="seasons-grid">
            {#each seasons as season (season.id)}
              {@const seasonLabel = season.title || `${season.season_sort_order}`}
              {@const seasonEventCount = season.actual_episode_count ?? season.episode_count ?? 0}
              {@const watchedCount = season.watched_event_count ?? 0}
              {@const autoWantBadge =
                season.auto_want_mode && season.auto_want_mode !== 'inherit'
                  ? (MODE_LABELS[season.auto_want_mode] ?? season.auto_want_mode)
                  : null}
              {#snippet seasonSummary()}
                {#if season.poster_url}
                  <img class="season-poster" src={season.poster_url} alt="" />
                {:else}
                  <div class="season-poster-placeholder">
                    {season.season_sort_order}
                  </div>
                {/if}
                <div class="season-card-info">
                  <span class="season-card-title">{seasonLabel}</span>
                  <div class="season-card-meta">
                    <span>{season.in_library_event_count ?? 0} / {seasonEventCount} events</span>
                    {#if watchedCount > 0}
                      <span class="watched-badge">{watchedCount} watched</span>
                    {/if}
                    {#if autoWantBadge}
                      <span class="auto-want-badge">{autoWantBadge}</span>
                    {/if}
                  </div>
                </div>
              {/snippet}
              <div class="season-card" class:selected={bulk.active && bulk.has(season.id)}>
                {#if bulk.active}
                  <label class="season-select">
                    <input
                      type="checkbox"
                      checked={bulk.has(season.id)}
                      onclick={(e: MouseEvent) => {
                        e.preventDefault();
                        bulk.toggle(season.id, seasons, e.shiftKey);
                      }}
                      aria-label="Select {seasonLabel}"
                    />
                  </label>
                  <div
                    class="season-link"
                    role="button"
                    tabindex="0"
                    onclick={(e: MouseEvent) => bulk.toggle(season.id, seasons, e.shiftKey)}
                    onkeydown={(e: KeyboardEvent) => {
                      if (e.key === 'Enter' || e.key === ' ') {
                        e.preventDefault();
                        bulk.toggle(season.id, seasons, e.shiftKey);
                      }
                    }}
                  >
                    {@render seasonSummary()}
                  </div>
                {:else}
                  <a class="season-link" href="/sports/seasons/{season.id}">
                    {@render seasonSummary()}
                  </a>
                  <div class="season-card-actions">
                    <div class="season-auto-want">
                      <SearchSelect
                        items={seasonAutoWantOptions}
                        value={season.auto_want_mode || 'inherit'}
                        onchange={(value) => {
                          onSeasonAutoWantChange(season.id, value);
                        }}
                      />
                    </div>
                    <SeasonActions
                      noun="event"
                      allWatched={seasonEventCount > 0 && watchedCount === seasonEventCount}
                      anyWanted={(season.wanted_event_count ?? 0) > 0}
                      onToggleWatched={(watched: boolean) =>
                        onSeasonWatchToggle(season.id, watched)}
                      onWantMissing={() => onWantSeason(season.id)}
                      onUnwantAll={() => onUnwantSeason(season.id)}
                    />
                  </div>
                {/if}
              </div>
            {/each}
          </div>

          <InfiniteScroll hasMore={hasMoreSeasons} loading={seasonsLoading} {onLoadMore} />
        {:else if seasonsLoading}
          <p class="muted" style="text-align: center;">Loading seasons...</p>
        {:else}
          <p class="muted" style="text-align: center;">No seasons yet</p>
        {/if}
      </div>
    </div>

    <div class="actions-column">
      <!-- Search Settings -->
      {#if onUpdateSportMetadata}
        <div class="section card-section">
          <div class="label">Search Settings</div>
          <div class="search-settings">
            <label class="field-label">
              Search Title
              <input
                type="text"
                class="field-input"
                placeholder={league.title}
                bind:value={searchTitle}
              />
              <span class="field-hint">Override league name used in search terms</span>
            </label>
            <label class="field-label">
              Location Aliases
              <textarea
                class="field-textarea"
                placeholder="United States=USA&#10;Great Britain=Silverstone"
                bind:value={locationAliasesText}
                rows="4"></textarea>
              <span class="field-hint">One per line: Original=Replacement</span>
            </label>
            <button class="btn btn-sm btn-outline" type="button" onclick={saveSearchSettings}>
              Save Settings
            </button>
          </div>
        </div>
      {/if}

      <!-- Genres -->
      {#if league.genres && league.genres.length > 0}
        <div class="section card-section">
          <div class="label">Genres</div>
          <GenreChips genres={league.genres} size="small" />
        </div>
      {/if}
    </div>
  </div>
</ContentPageView>

<style>
  .card-section {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 24px;
  }

  .syncing-indicator {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--accent);
    font-size: 12px;
    font-weight: 600;
  }

  .syncing-indicator :global(.spin) {
    font-size: 14px;
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
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

  .btn-sm {
    padding: 6px 14px;
    font-size: 12px;
  }

  .label {
    font-size: 11px;
    font-weight: 800;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.1em;
    margin-bottom: 12px;
  }

  .section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  .seasons-grid {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .season-card {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    border-radius: 10px;
    transition: background 0.15s;
  }

  .season-card:hover {
    background: var(--bg-2);
  }

  .season-card.selected {
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }

  .season-select {
    display: flex;
    align-items: center;
    flex-shrink: 0;
    cursor: pointer;
  }

  .season-select input {
    width: 16px;
    height: 16px;
    accent-color: var(--accent);
    cursor: pointer;
  }

  .season-link {
    display: flex;
    align-items: center;
    gap: 12px;
    flex: 1;
    min-width: 0;
    text-decoration: none;
    color: inherit;
    cursor: pointer;
  }

  .season-poster {
    width: 44px;
    height: 60px;
    border-radius: 6px;
    object-fit: cover;
    flex-shrink: 0;
  }

  .season-poster-placeholder {
    width: 44px;
    height: 60px;
    border-radius: 6px;
    background: var(--bg-2);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 13px;
    font-weight: 700;
    color: var(--muted);
    flex-shrink: 0;
  }

  .season-card-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .season-card-title {
    font-size: 14px;
    font-weight: 700;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .season-card-meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px 8px;
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
  }

  .season-card-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .season-auto-want {
    width: 140px;
  }

  .watched-badge {
    color: var(--accent);
  }

  .auto-want-badge {
    font-size: 10px;
    font-weight: 700;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--accent-dim);
    color: var(--accent);
    text-transform: uppercase;
    letter-spacing: 0.3px;
    /* Desktop rows carry the auto-want picker itself; phones get this instead. */
    display: none;
  }

  .muted {
    color: var(--muted);
    font-size: 13px;
  }

  .section {
    margin-bottom: 16px;
  }

  .search-settings {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .field-label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    font-weight: 600;
    color: var(--ink);
  }

  .field-input,
  .field-textarea {
    padding: 8px 10px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--bg-2);
    color: var(--ink);
    font-size: 13px;
    font-family: inherit;
    resize: vertical;
  }

  .field-input:focus,
  .field-textarea:focus {
    outline: none;
    border-color: var(--accent);
  }

  .field-hint {
    font-size: 11px;
    color: var(--muted);
    font-weight: 400;
  }

  @media (max-width: 768px) {
    .full-page-main-grid {
      gap: 16px;
    }

    .card-section {
      padding: 16px;
    }

    .season-card {
      gap: 8px;
      padding: 8px 4px;
    }

    .season-link {
      gap: 8px;
    }

    .season-poster,
    .season-poster-placeholder {
      width: 36px;
      height: 48px;
      font-size: 11px;
    }

    .season-auto-want {
      display: none;
    }

    .auto-want-badge {
      display: inline;
    }
  }
</style>
