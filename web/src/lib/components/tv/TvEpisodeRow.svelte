<script lang="ts">
  import type { TvEpisode } from '$lib/types';
  import { getWantStatusDisplay, isActivelyWanted } from '$lib/utils/want-status';

  interface Props {
    episode: TvEpisode;
    href?: string;
    onWant: () => void;
    onUnwant: () => void;
    onWatchToggle: () => void;
    onSearchReleases?: () => void;
    onPlay?: () => void;
    selectionMode?: boolean;
    selected?: boolean;
    onToggleSelect?: (shiftKey: boolean) => void;
    number?: string | number;
    title?: string;
    noun?: string;
  }

  let {
    episode,
    href,
    onWant,
    onUnwant,
    onWatchToggle,
    onSearchReleases,
    onPlay,
    selectionMode = false,
    selected = false,
    onToggleSelect,
    number,
    title,
    noun = 'episode'
  }: Props = $props();

  // In selection mode the whole row is the checkbox target — navigating away
  // mid-selection loses the set, and the rows are too small for a checkbox-only
  // hit area to feel right.
  const selectable = $derived(selectionMode && onToggleSelect !== undefined);

  const statusDisplay = $derived(getWantStatusDisplay(episode.is_in_library, episode.want_status));
  const fileStatus = $derived(statusDisplay?.cssClass ?? 'missing');
  const statusLabel = $derived(
    statusDisplay
      ? statusDisplay.cssClass.replace(/-/g, ' ').replace(/\b\w/g, (c) => c.toUpperCase())
      : 'Missing'
  );

  const formattedAirDate = $derived(
    episode.air_date
      ? new Date(episode.air_date).toLocaleDateString(undefined, {
          year: 'numeric',
          month: 'short',
          day: 'numeric'
        })
      : null
  );

  const runtimeDisplay = $derived(episode.runtime ? `${episode.runtime}m` : null);
  const isWatched = $derived(episode.watched === true);
</script>

{#snippet summary()}
  <div class="episode-number">
    {number ?? episode.episode_identifier}
  </div>
  <div class="episode-info">
    <span class="episode-title">{title ?? episode.title ?? 'TBA'}</span>
    <div class="episode-meta">
      {#if formattedAirDate}
        <span>{formattedAirDate}</span>
      {/if}
      {#if runtimeDisplay}
        <span>{runtimeDisplay}</span>
      {/if}
    </div>
  </div>
{/snippet}

<div
  class="episode-row"
  class:has-file={episode.is_in_library}
  class:selected={selectable && selected}
>
  {#if selectable}
    <label class="episode-select">
      <input
        type="checkbox"
        checked={selected}
        onclick={(e: MouseEvent) => {
          e.preventDefault();
          onToggleSelect?.(e.shiftKey);
        }}
        aria-label="Select {title ?? episode.episode_identifier}"
      />
    </label>
  {/if}
  {#if href && !selectable}
    <a class="episode-link" {href}>
      {@render summary()}
    </a>
  {:else if selectable}
    <div
      class="episode-link episode-toggle"
      role="button"
      tabindex="0"
      onclick={(e: MouseEvent) => onToggleSelect?.(e.shiftKey)}
      onkeydown={(e: KeyboardEvent) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onToggleSelect?.(e.shiftKey);
        }
      }}
    >
      {@render summary()}
    </div>
  {:else}
    {@render summary()}
  {/if}

  <div class="episode-status">
    <span class="status-indicator {fileStatus}" title={statusLabel}></span>
  </div>

  <div class="episode-actions" class:hidden={selectable}>
    {#if onPlay && episode.is_in_library}
      <button class="action-btn play-btn" type="button" title="Play" onclick={onPlay}>
        <span class="material-icons-outlined" style="font-size: 16px;">play_arrow</span>
      </button>
    {/if}

    {#if onSearchReleases && (episode.is_in_library || episode.want_status === 'wanted' || episode.want_status === 'searching' || episode.want_status === 'search_failed' || episode.want_status === 'downloading' || episode.want_status === 'ingesting' || episode.want_status === 'failed')}
      <button
        class="action-btn search-btn"
        type="button"
        title="Search releases"
        onclick={onSearchReleases}
      >
        <span class="material-icons-outlined" style="font-size: 16px;">search</span>
      </button>
    {/if}

    {#if episode.is_in_library || episode.watched}
      <button
        class="action-btn watch-btn"
        class:watched={isWatched}
        type="button"
        title={isWatched ? 'Mark as unwatched' : 'Mark as watched'}
        onclick={onWatchToggle}
      >
        <span class="material-icons-outlined" style="font-size: 16px;"
          >{isWatched ? 'visibility' : 'visibility_off'}</span
        >
      </button>
    {/if}

    {#if !episode.is_in_library && (episode.want_status === 'downloading' || episode.want_status === 'ingesting' || episode.want_status === 'failed')}
      <button class="action-btn unwant-btn" type="button" title="Reset status" onclick={onUnwant}>
        <span class="material-icons-outlined" style="font-size: 16px;">restart_alt</span>
      </button>
    {:else if isActivelyWanted(episode.want_status)}
      <button
        class="action-btn unwant-btn"
        type="button"
        title="Remove from wanted"
        onclick={onUnwant}
      >
        <span class="material-icons-outlined" style="font-size: 16px;">remove_circle_outline</span>
      </button>
    {:else if !episode.is_in_library}
      <button class="action-btn want-btn" type="button" title="Want this {noun}" onclick={onWant}>
        <span class="material-icons-outlined" style="font-size: 16px;">add_circle_outline</span>
      </button>
    {/if}
  </div>
</div>

<style>
  .episode-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--ring);
    transition: background 0.15s ease;
  }

  .episode-row:last-child {
    border-bottom: none;
  }

  .episode-row:hover {
    background: var(--bg-2);
  }

  .episode-row.has-file {
    opacity: 1;
  }

  .episode-row.selected {
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }

  .episode-select {
    display: flex;
    align-items: center;
    flex-shrink: 0;
    cursor: pointer;
  }

  .episode-toggle {
    cursor: pointer;
    user-select: none;
  }

  .episode-actions.hidden {
    display: none;
  }

  .episode-select input {
    width: 16px;
    height: 16px;
    accent-color: var(--accent);
    cursor: pointer;
  }

  .episode-link {
    display: flex;
    align-items: center;
    gap: 12px;
    flex: 1;
    min-width: 0;
    text-decoration: none;
    color: inherit;
    border-radius: 6px;
    padding: 2px 4px;
    margin: -2px -4px;
    transition: background 0.15s ease;
  }

  .episode-link:hover .episode-title {
    color: var(--accent);
  }

  .episode-number {
    min-width: 32px;
    font-size: 13px;
    font-weight: 700;
    color: var(--muted);
    text-align: center;
    flex-shrink: 0;
  }

  .episode-info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .episode-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .episode-meta {
    display: flex;
    gap: 8px;
    font-size: 11px;
    color: var(--muted);
  }

  .episode-status {
    flex-shrink: 0;
  }

  .status-indicator {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }

  .status-indicator.in-library {
    background: var(--success, #22c55e);
  }

  .status-indicator.wanted {
    background: var(--wanted, #f59e0b);
  }

  .status-indicator.searching,
  .status-indicator.downloading,
  .status-indicator.ingesting,
  .status-indicator.watching {
    background: var(--accent);
  }

  .status-indicator.search-failed,
  .status-indicator.failed {
    background: var(--error, #ef4444);
  }

  .status-indicator.skipped {
    background: var(--muted);
    opacity: 0.6;
  }

  .status-indicator.missing {
    background: var(--muted);
    opacity: 0.4;
  }

  .episode-actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
  }

  .action-btn {
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
    transition: all 0.15s ease;
  }

  .action-btn:hover {
    background: var(--bg-2);
    color: var(--ink);
  }

  .play-btn {
    border-color: var(--accent);
    color: var(--accent);
  }

  .play-btn:hover {
    background: var(--accent-dim);
  }

  .search-btn:hover {
    color: var(--accent);
    border-color: var(--accent);
  }

  .want-btn:hover {
    color: var(--wanted, #f59e0b);
    border-color: var(--wanted, #f59e0b);
  }

  .unwant-btn:hover {
    color: var(--error, #ef4444);
    border-color: var(--error, #ef4444);
  }

  .watch-btn:hover {
    color: var(--accent);
    border-color: var(--accent);
  }

  .watch-btn.watched {
    color: var(--accent);
    border-color: var(--accent);
  }

  /* Tighter layout on phones: shrink padding/gaps so the title keeps room
     next to the action cluster on ~360px viewports. */
  @media (max-width: 480px) {
    .episode-row {
      gap: 8px;
      padding: 8px 10px;
    }

    .episode-link {
      gap: 8px;
    }

    .episode-number {
      min-width: 24px;
      font-size: 12px;
    }

    .episode-actions {
      gap: 3px;
    }

    .action-btn {
      width: 26px;
      height: 26px;
    }
  }
</style>
