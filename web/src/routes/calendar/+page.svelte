<script lang="ts">
  import CalendarTimeline from '#lib/components/calendar/CalendarTimeline.svelte';
  import CalendarGrid from '#lib/components/calendar/CalendarGrid.svelte';
  import { mediaTypeChip } from '#lib/utils/chip-mappings.js';

  type ViewMode = 'timeline' | 'grid';
  type MediaFilter = 'tv' | 'movie' | 'book' | 'sport';

  const MEDIA_FILTERS: MediaFilter[] = ['tv', 'movie', 'book', 'sport'];

  // Persist view mode to localStorage; force timeline on mobile (grid is too cramped)
  const isMobile = typeof window !== 'undefined' && window.innerWidth <= 768;
  const savedView =
    typeof localStorage !== 'undefined'
      ? (localStorage.getItem('calendar-view') as ViewMode | null)
      : null;

  let viewMode = $state<ViewMode>(isMobile ? 'timeline' : (savedView ?? 'timeline'));

  let activeFilters = $state<MediaFilter[]>([]);

  function setViewMode(mode: ViewMode) {
    viewMode = mode;
    if (typeof localStorage !== 'undefined') {
      localStorage.setItem('calendar-view', mode);
    }
  }

  function toggleFilter(type: MediaFilter) {
    if (activeFilters.includes(type)) {
      activeFilters = activeFilters.filter((f) => f !== type);
    } else {
      activeFilters = [...activeFilters, type];
    }
  }

  // Pass empty array = show all, otherwise pass active filters
  const filterList = $derived(activeFilters);
</script>

<svelte:head>
  <title>Calendar — Kobbler</title>
</svelte:head>

<div class="calendar-page page-container">
  <div class="toolbar">
    <div class="toolbar-left">
      <div class="view-controls">
        {#each MEDIA_FILTERS as type (type)}
          {@const chip = mediaTypeChip(type)}
          {@const active = activeFilters.length === 0 || activeFilters.includes(type)}
          <button
            class="view-tab type-tab"
            class:active
            type="button"
            style="--tab-color: {chip.color}"
            onclick={() => toggleFilter(type)}
          >
            <span class="material-icons-outlined">{chip.icon}</span>
            {chip.label}
          </button>
        {/each}
      </div>
    </div>

    <div class="toolbar-right">
      <div class="view-controls">
        <button
          class="view-tab icon-tab"
          class:active={viewMode === 'timeline'}
          type="button"
          aria-label="Timeline view"
          onclick={() => setViewMode('timeline')}
        >
          <span class="material-icons-outlined">view_list</span>
        </button>
        <button
          class="view-tab icon-tab"
          class:active={viewMode === 'grid'}
          type="button"
          aria-label="Grid view"
          onclick={() => setViewMode('grid')}
        >
          <span class="material-icons-outlined">calendar_month</span>
        </button>
      </div>
    </div>
  </div>

  <div class="calendar-content" class:timeline-mode={viewMode === 'timeline'}>
    {#if viewMode === 'timeline'}
      <CalendarTimeline filter={filterList} />
    {:else}
      <CalendarGrid filter={filterList} />
    {/if}
  </div>
</div>

<style>
  /* Sticky so the filters + view toggle stay reachable while scrolling
     through the timeline (which auto-scrolls to yesterday on load).
     Same bg as the page — no border, no band — so it doesn't read as a
     differently-coloured header. */
  .toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
    position: sticky;
    top: 0;
    z-index: 10;
    background: var(--bg);
    padding: 12px 0;
    margin-bottom: 4px;
  }

  .toolbar-left,
  .toolbar-right {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .view-controls {
    display: flex;
    gap: 2px;
    background: var(--card);
    padding: 4px;
    border-radius: 8px;
    border: 1px solid var(--ring);
  }

  .view-tab {
    padding: 6px 12px;
    border-radius: 6px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    color: var(--muted);
    border: none;
    background: none;
    transition:
      background 0.15s,
      color 0.15s;
  }

  .view-tab:hover {
    background: rgba(255, 255, 255, 0.05);
    color: var(--ink);
  }

  .view-tab.active {
    background: var(--bg-2);
    color: var(--ink);
  }

  /* Media-type tabs colour their active state with the media-type accent.
     When all filters are off (active for every tab) the colours read as
     "everything is on"; clicking one mutes the rest. */
  .type-tab {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .type-tab .material-icons-outlined {
    font-size: 16px;
    color: var(--muted);
    transition: color 0.15s;
  }

  .type-tab.active {
    color: var(--tab-color);
    background: color-mix(in srgb, var(--tab-color) 14%, transparent);
  }

  .type-tab.active .material-icons-outlined {
    color: var(--tab-color);
  }

  .type-tab:hover {
    color: var(--tab-color);
  }

  .icon-tab {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px 8px;
  }

  .icon-tab .material-icons-outlined {
    font-size: 18px;
  }

  /* Timeline view: narrower centered column for readable row width.
     Grid view fills the page-container. */
  .calendar-content.timeline-mode {
    max-width: 880px;
    margin: 0 auto;
  }
</style>
