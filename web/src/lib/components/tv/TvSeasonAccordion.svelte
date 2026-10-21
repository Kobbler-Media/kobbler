<script lang="ts">
  import type { TvEpisode, TvSeason } from '$lib/types';
  import { hasOutstandingWants } from '$lib/utils/want-status';
  import TvEpisodeRow from './TvEpisodeRow.svelte';
  import SeasonActions from '../shared/SeasonActions.svelte';
  import SeasonAccordionShell from './SeasonAccordionShell.svelte';

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

<SeasonAccordionShell label={seasonLabel} href="/tv/seasons/{season.id}" {expanded} {onToggle}>
  {#snippet leading()}
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
  {/snippet}
  {#snippet meta()}
    <span class="episode-count">{inLibraryCount} / {totalCount} episodes</span>
    {#if watchedCount > 0}
      <span class="watched-badge">{watchedCount} watched</span>
    {/if}
    {#if autoWantBadge}
      <span class="auto-want-badge">{autoWantBadge}</span>
    {/if}
  {/snippet}
  {#snippet actions()}
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
  {/snippet}
  {#if episodes.length === 0}
    <div class="empty-episodes">No episodes loaded</div>
  {:else}
    {#each episodes as episode (episode.id)}
      <TvEpisodeRow
        {episode}
        href="/tv/episodes/{episode.id}"
        showMedia
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
</SeasonAccordionShell>

<style>
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

  .episode-count {
    white-space: nowrap;
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

  .empty-episodes {
    padding: 20px;
    text-align: center;
    color: var(--muted);
    font-size: 13px;
  }
</style>
