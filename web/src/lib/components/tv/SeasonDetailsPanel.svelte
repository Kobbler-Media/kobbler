<script lang="ts">
  import type { TvSeason } from '#lib/types.js';
  import { getWantStatusDisplay, hasOutstandingWants } from '#lib/utils/want-status.js';
  import { SearchSelect } from '@koblibs/ui';

  interface Props {
    season: TvSeason;
    showTitle?: string;
    onEpisodeClick?: (episodeId: string) => void;
    onWantSeason?: () => void;
    onUnwantSeason?: () => void;
    onBulkWatchToggle?: (watched: boolean) => void;
    onAutoWantChange?: (mode: string) => void;
    onEdit?: () => void;
    onDelete?: () => void;
  }

  let {
    season,
    showTitle,
    onEpisodeClick,
    onWantSeason,
    onUnwantSeason,
    onBulkWatchToggle,
    onAutoWantChange,
    onEdit,
    onDelete
  }: Props = $props();

  const allWatched = $derived(
    season.episodes.length > 0 && season.episodes.every((e) => e.watched === true)
  );
  const anyWanted = $derived(hasOutstandingWants(season.episodes));

  const sortedEpisodes = $derived(
    [...season.episodes].sort((a, b) => a.episode_sort_order - b.episode_sort_order)
  );

  const autoWantMode = $derived(season.auto_want_mode || 'inherit');

  const autoWantOptions = [
    { value: 'inherit', label: 'Inherit' },
    { value: 'none', label: 'None' },
    { value: 'all', label: 'All Episodes' },
    { value: 'new_only', label: 'New Only' },
    { value: 'fill_gaps', label: 'Fill Gaps' }
  ];
</script>

<div class="season-detail">
  {#if season.poster_url}
    <div class="poster-section">
      <img class="season-poster" src={season.poster_url} alt="" />
    </div>
  {/if}

  <div class="header-section">
    {#if showTitle}
      <div class="show-name">{showTitle}</div>
    {/if}
    <h2 class="season-title">{season.title || `Season ${season.season_sort_order}`}</h2>
    <div class="panel-meta-row">
      {#if season.air_date}
        <span class="panel-meta-item">{season.air_date}</span>
      {/if}
      <span class="panel-meta-item">{season.episodes.length} episodes</span>
    </div>
  </div>

  {#if season.description}
    <div class="panel-section">
      <h4 class="panel-section-title">Overview</h4>
      <p class="panel-overview">{season.description}</p>
    </div>
  {/if}

  {#if onAutoWantChange}
    <div class="panel-section">
      <h4 class="panel-section-title">Auto-want</h4>
      <SearchSelect value={autoWantMode} items={autoWantOptions} onchange={onAutoWantChange} />
    </div>
  {/if}

  <div class="panel-section">
    <div class="panel-section-title-row">
      <h4 class="panel-section-title">Episodes</h4>
      <div class="panel-section-actions">
        {#if onBulkWatchToggle}
          <button
            class="panel-icon-btn"
            class:watched={allWatched}
            type="button"
            title={allWatched ? 'Mark unwatched' : 'Mark watched'}
            onclick={() => onBulkWatchToggle!(!allWatched)}
          >
            <span class="material-icons-outlined">
              {allWatched ? 'visibility_off' : 'visibility'}
            </span>
          </button>
        {/if}
        {#if onWantSeason}
          <button class="panel-icon-btn" type="button" title="Want missing" onclick={onWantSeason}>
            <span class="material-icons-outlined">playlist_add</span>
          </button>
        {/if}
        {#if onUnwantSeason && anyWanted}
          <button
            class="panel-icon-btn"
            type="button"
            title="Unwant every outstanding episode in this season"
            onclick={onUnwantSeason}
          >
            <span class="material-icons-outlined">playlist_remove</span>
          </button>
        {/if}
      </div>
    </div>
    <div class="episodes-list">
      {#each sortedEpisodes as episode (episode.id)}
        {@const status = getWantStatusDisplay(episode.is_in_library, episode.want_status)}
        <button class="episode-item" type="button" onclick={() => onEpisodeClick?.(episode.id)}>
          <div class="episode-number">{episode.episode_sort_order}</div>
          <div class="episode-info">
            <span class="episode-title"
              >{episode.title || `Episode ${episode.episode_sort_order}`}</span
            >
            {#if episode.air_date}
              <span class="episode-date">{episode.air_date}</span>
            {/if}
          </div>
          <div class="episode-status">
            {#if status}
              <span class="material-icons-outlined status-icon {status.cssClass}"
                >{status.icon}</span
              >
            {/if}
          </div>
        </button>
      {/each}
    </div>
  </div>

  {#if onEdit || onDelete}
    <div class="panel-actions">
      {#if onEdit}
        <button class="panel-action-btn" type="button" onclick={onEdit}>
          <span class="material-icons-outlined" style="font-size: 16px;">edit</span>
          Edit
        </button>
      {/if}
      {#if onDelete}
        <button class="panel-action-btn danger" type="button" onclick={onDelete}>
          <span class="material-icons-outlined" style="font-size: 16px;">delete</span>
          Delete
        </button>
      {/if}
    </div>
  {/if}
</div>

<style>
  .season-detail {
    display: flex;
    flex-direction: column;
    gap: 0;
  }

  .poster-section {
    display: flex;
    justify-content: center;
    padding: 0 0 16px;
  }

  .season-poster {
    width: 140px;
    border-radius: 8px;
    object-fit: cover;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
  }

  .header-section {
    padding: 0 0 0;
  }

  .show-name {
    font-size: 12px;
    color: var(--muted);
    margin-bottom: 4px;
  }

  .season-title {
    margin: 0 0 8px;
    font-size: 20px;
    font-weight: 800;
    line-height: 1.2;
  }

  .panel-meta-row {
    margin-bottom: 8px;
    gap: 8px;
  }

  .panel-meta-item {
    font-size: 12px;
  }

  .episodes-list {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .episode-item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 8px;
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

  .episode-item:hover {
    background: var(--bg-2);
  }

  .episode-number {
    width: 28px;
    font-size: 14px;
    font-weight: 700;
    color: var(--muted);
    text-align: center;
    flex-shrink: 0;
  }

  .episode-info {
    display: flex;
    flex-direction: column;
    gap: 1px;
    flex: 1;
    min-width: 0;
  }

  .episode-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .episode-date {
    font-size: 11px;
    color: var(--muted);
  }

  .episode-status {
    flex-shrink: 0;
  }

  .episode-status :global(.status-icon) {
    font-size: 18px;
  }

  .panel-icon-btn.watched {
    color: var(--accent);
  }

  .panel-actions {
    display: flex;
    gap: 8px;
    padding: 16px 0 0;
    border-top: 1px solid var(--ring);
  }

  .panel-action-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 14px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: transparent;
    color: var(--muted);
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
  }

  .panel-action-btn:hover {
    background: var(--bg-2);
    color: var(--ink);
    border-color: var(--ink);
  }

  .panel-action-btn.danger:hover {
    color: var(--error, #ef4444);
    border-color: var(--error, #ef4444);
  }
</style>
