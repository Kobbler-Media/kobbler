<script lang="ts">
  import type { TvEpisode, TvSeason } from '$lib/types';
  import { hasOutstandingWants } from '$lib/utils/want-status';
  import TvEpisodeRow from './TvEpisodeRow.svelte';
  import SeasonActions from '../shared/SeasonActions.svelte';

  interface Props {
    season: TvSeason;
    expanded: boolean;
    onToggle: () => void;
    onWantSeason: () => void;
    onUnwantSeason: () => void;
    onWantEpisode: (episodeId: string) => void;
    onUnwantEpisode: (episodeId: string) => void;
    onWatchToggle: (episodeId: string) => void;
    onBulkWatchToggle?: (seasonId: string, watched: boolean) => void;
    onSearchReleases?: (episodeId: string) => void;
    onPlayEpisode?: (episodeId: string) => void;
    onAutoWantChange?: (seasonId: string, mode: string) => void;
    selectionMode?: boolean;
    isSelected?: (episodeId: string) => boolean;
    onToggleSelect?: (episodeId: string, within: TvEpisode[], shiftKey: boolean) => void;
    seasonAllSelected?: boolean;
    onToggleSelectSeason?: () => void;
  }

  let {
    season,
    expanded,
    onToggle,
    onWantSeason,
    onUnwantSeason,
    onWantEpisode,
    onUnwantEpisode,
    onWatchToggle,
    onBulkWatchToggle,
    onSearchReleases,
    onPlayEpisode,
    onAutoWantChange: _onAutoWantChange,
    selectionMode = false,
    isSelected,
    onToggleSelect,
    seasonAllSelected = false,
    onToggleSelectSeason
  }: Props = $props();

  const episodes = $derived(season.episodes ?? []);

  const allWatched = $derived(episodes.length > 0 && episodes.every((e) => e.watched === true));
  const anyWanted = $derived(hasOutstandingWants(episodes));
  const inLibraryCount = $derived(episodes.filter((e) => e.is_in_library).length);
  const watchedCount = $derived(episodes.filter((e) => e.watched === true).length);
  const totalCount = $derived(season.episode_count ?? episodes.length);

  const seasonLabel = $derived(
    season.is_special ? 'Specials' : `Season ${season.season_identifier}`
  );

  const MODE_LABELS: Record<string, string> = {
    none: 'None',
    new_only: 'New Only',
    all: 'All',
    fill_gaps: 'Fill Gaps'
  };

  const autoWantBadge = $derived(
    season.auto_want_mode && season.auto_want_mode !== 'inherit'
      ? (MODE_LABELS[season.auto_want_mode] ?? season.auto_want_mode)
      : null
  );
</script>

<div class="season-accordion" class:expanded>
  <!-- Use div instead of button to allow nested want-season button -->
  <div
    class="season-header"
    role="button"
    tabindex="0"
    onclick={onToggle}
    onkeydown={(e) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        onToggle();
      }
    }}
  >
    {#if selectionMode && onToggleSelectSeason}
      <span class="season-select">
        <input
          type="checkbox"
          checked={seasonAllSelected}
          onchange={onToggleSelectSeason}
          onclick={(e: MouseEvent) => e.stopPropagation()}
          onkeydown={(e: KeyboardEvent) => e.stopPropagation()}
          aria-label="Select all episodes in {seasonLabel}"
        />
      </span>
    {/if}
    <span class="expand-icon material-icons-outlined" style="font-size: 18px;">
      {expanded ? 'expand_more' : 'chevron_right'}
    </span>
    <div class="season-info">
      <a
        class="season-title"
        href="/tv/seasons/{season.id}"
        onclick={(e: MouseEvent) => e.stopPropagation()}
      >
        {seasonLabel}
      </a>
      <div class="season-meta">
        <span class="episode-count">
          {inLibraryCount} / {totalCount} episodes
        </span>
        {#if watchedCount > 0}
          <span class="watched-badge">{watchedCount} watched</span>
        {/if}
        {#if autoWantBadge}
          <span class="auto-want-badge">{autoWantBadge}</span>
        {/if}
      </div>
    </div>
    {#if !selectionMode}
      <SeasonActions
        noun="episode"
        {allWatched}
        {anyWanted}
        onToggleWatched={onBulkWatchToggle
          ? (watched: boolean) => onBulkWatchToggle(season.id, watched)
          : undefined}
        onWantMissing={onWantSeason}
        onUnwantAll={onUnwantSeason}
      />
    {/if}
  </div>

  {#if expanded}
    <div class="season-episodes">
      {#if episodes.length === 0}
        <div class="empty-episodes">No episodes loaded</div>
      {:else}
        {#each episodes as episode (episode.id)}
          <TvEpisodeRow
            {episode}
            href="/tv/episodes/{episode.id}"
            onWant={() => onWantEpisode(episode.id)}
            onUnwant={() => onUnwantEpisode(episode.id)}
            onWatchToggle={() => onWatchToggle(episode.id)}
            onSearchReleases={onSearchReleases ? () => onSearchReleases(episode.id) : undefined}
            onPlay={onPlayEpisode ? () => onPlayEpisode(episode.id) : undefined}
            {selectionMode}
            selected={isSelected?.(episode.id) ?? false}
            onToggleSelect={onToggleSelect
              ? (shiftKey: boolean) => onToggleSelect(episode.id, episodes, shiftKey)
              : undefined}
          />
        {/each}
      {/if}
    </div>
  {/if}
</div>

<style>
  .season-accordion {
    border: 1px solid var(--ring);
    border-radius: 12px;
    overflow: hidden;
    background: var(--card);
  }

  .season-accordion.expanded {
    border-color: var(--accent-dim);
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

  .season-header {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 14px 16px;
    border: none;
    background: transparent;
    color: var(--ink);
    cursor: pointer;
    font: inherit;
    text-align: left;
    transition: background 0.15s ease;
  }

  .season-header:hover {
    background: var(--bg-2);
  }

  .expand-icon {
    color: var(--muted);
    flex-shrink: 0;
    transition: transform 0.2s ease;
  }

  .season-info {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
  }

  .season-meta {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    flex-wrap: wrap;
  }

  .season-title {
    font-size: 14px;
    font-weight: 700;
    color: var(--ink);
    text-decoration: none;
    transition: color 0.15s ease;
  }

  .season-title:hover {
    color: var(--accent);
  }

  .episode-count {
    font-size: 12px;
    color: var(--muted);
  }

  .watched-badge {
    font-size: 11px;
    color: var(--accent);
    white-space: nowrap;
  }

  .auto-want-badge {
    font-size: 10px;
    font-weight: 700;
    padding: 2px 6px;
    border-radius: 4px;
    background: var(--accent-dim);
    color: var(--accent);
    text-transform: uppercase;
    letter-spacing: 0.3px;
    white-space: nowrap;
  }

  .season-episodes {
    border-top: 1px solid var(--ring);
  }

  .empty-episodes {
    padding: 20px;
    text-align: center;
    color: var(--muted);
    font-size: 13px;
  }

  @media (max-width: 768px) {
    .season-header {
      padding: 12px 10px;
    }

    .season-info {
      flex-direction: column;
      align-items: flex-start;
      gap: 2px;
    }
  }
</style>
