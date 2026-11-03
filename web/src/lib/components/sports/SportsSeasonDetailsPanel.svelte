<script lang="ts">
  import type { TvSeason } from '#lib/types.js';
  import { getWantStatusDisplay, hasOutstandingWants } from '#lib/utils/want-status.js';
  import { SearchSelect } from '@koblibs/ui';

  interface Props {
    season: TvSeason;
    leagueTitle?: string;
    onEventClick?: (eventId: string) => void;
    onPlayEvent?: (eventId: string) => void;
    onWantSeason?: () => void;
    onUnwantSeason?: () => void;
    onAutoWantChange?: (mode: string) => void;
    onEdit?: () => void;
    onDelete?: () => void;
  }

  let {
    season,
    leagueTitle,
    onEventClick,
    onPlayEvent,
    onWantSeason,
    onUnwantSeason,
    onAutoWantChange,
    onEdit,
    onDelete
  }: Props = $props();

  const sortedEvents = $derived(
    [...season.episodes].sort((a, b) => a.episode_sort_order - b.episode_sort_order)
  );

  const anyWanted = $derived(hasOutstandingWants(season.episodes));

  const autoWantMode = $derived(season.auto_want_mode || 'inherit');

  const autoWantOptions = [
    { value: 'inherit', label: 'Inherit' },
    { value: 'none', label: 'None' },
    { value: 'all', label: 'All Events' },
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
    {#if leagueTitle}
      <div class="league-name">{leagueTitle}</div>
    {/if}
    <h2 class="season-title">{season.title || `Season ${season.season_sort_order}`}</h2>
    <div class="panel-meta-row">
      {#if season.air_date}
        <span class="panel-meta-item">{season.air_date}</span>
      {/if}
      <span class="panel-meta-item">{season.episodes.length} events</span>
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
      <h4 class="panel-section-title">Events</h4>
      <div class="panel-section-actions">
        {#if onWantSeason}
          <button class="panel-icon-btn" type="button" title="Want missing" onclick={onWantSeason}>
            <span class="material-icons-outlined">playlist_add</span>
          </button>
        {/if}
        {#if onUnwantSeason && anyWanted}
          <button
            class="panel-icon-btn"
            type="button"
            title="Unwant every outstanding event in this season"
            onclick={onUnwantSeason}
          >
            <span class="material-icons-outlined">playlist_remove</span>
          </button>
        {/if}
      </div>
    </div>
    <div class="events-list">
      {#each sortedEvents as event (event.id)}
        {@const status = getWantStatusDisplay(event.is_in_library, event.want_status)}
        <div
          class="event-item"
          role="button"
          tabindex="0"
          onclick={() => onEventClick?.(event.id)}
          onkeydown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              onEventClick?.(event.id);
            }
          }}
        >
          <div class="event-number">{event.episode_sort_order}</div>
          <div class="event-info">
            <span class="event-title">{event.title || `Event ${event.episode_sort_order}`}</span>
            {#if event.air_date}
              <span class="event-date">{event.air_date}</span>
            {/if}
          </div>
          {#if onPlayEvent && event.is_in_library}
            <button
              class="play-event-btn"
              type="button"
              title="Play"
              onclick={(e) => {
                e.stopPropagation();
                onPlayEvent(event.id);
              }}
            >
              <span class="material-icons-outlined" style="font-size: 16px;">play_arrow</span>
            </button>
          {/if}
          <div class="event-status">
            {#if status}
              <span class="material-icons-outlined status-icon {status.cssClass}"
                >{status.icon}</span
              >
            {/if}
          </div>
        </div>
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

  .league-name {
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

  .events-list {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .event-item {
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

  .event-item:hover {
    background: var(--bg-2);
  }

  .event-number {
    width: 28px;
    font-size: 14px;
    font-weight: 700;
    color: var(--muted);
    text-align: center;
    flex-shrink: 0;
  }

  .event-info {
    display: flex;
    flex-direction: column;
    gap: 1px;
    flex: 1;
    min-width: 0;
  }

  .event-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .event-date {
    font-size: 11px;
    color: var(--muted);
  }

  .play-event-btn {
    width: 28px;
    height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--accent);
    border-radius: 6px;
    background: transparent;
    color: var(--accent);
    cursor: pointer;
    transition: all 0.15s ease;
    flex-shrink: 0;
  }

  .play-event-btn:hover {
    background: var(--accent-dim);
  }

  .event-status {
    flex-shrink: 0;
  }

  .event-status :global(.status-icon) {
    font-size: 18px;
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
