<script lang="ts">
  import type { CalendarItem } from '$lib/types';
  import { getCalendar } from '$lib/api/client';
  import { tick } from 'svelte';
  import BidirectionalScroll from '$lib/components/BidirectionalScroll.svelte';
  import CalendarItemCard from './CalendarItemCard.svelte';

  interface Props {
    filter: string[];
  }

  let { filter }: Props = $props();

  // Date range tracking
  let earliestDate = $state('');
  let latestDate = $state('');
  let items = $state<CalendarItem[]>([]);
  let loading = $state(false);
  let hasMore = $state(true);
  let hasPrevious = $state(false);
  let initialLoaded = $state(false);

  const PAGE_DAYS = 30;

  function toDateStr(d: Date): string {
    const y = d.getFullYear();
    const m = String(d.getMonth() + 1).padStart(2, '0');
    const day = String(d.getDate()).padStart(2, '0');
    return `${y}-${m}-${day}`;
  }

  function addDays(dateStr: string, days: number): string {
    const d = new Date(dateStr + 'T00:00:00');
    d.setDate(d.getDate() + days);
    return toDateStr(d);
  }

  function today(): string {
    return toDateStr(new Date());
  }

  async function scrollToYesterday() {
    await tick();
    await new Promise<void>((resolve) => {
      requestAnimationFrame(() => {
        requestAnimationFrame(() => resolve());
      });
    });
    const yesterday = addDays(today(), -1);
    // Try yesterday first, then fall back to the closest date group at or before today
    let el = document.querySelector(`[data-date="${yesterday}"]`);
    if (!el) {
      const todayStr = today();
      const groups = document.querySelectorAll('[data-date]');
      for (const g of groups) {
        const d = g.getAttribute('data-date');
        if (d && d <= todayStr) el = g;
        else break; // groups are sorted chronologically
      }
    }
    if (el) {
      el.scrollIntoView({ block: 'start' });
    }
  }

  async function loadInitial() {
    loading = true;
    hasPrevious = false;
    const from = addDays(today(), -7);
    const to = addDays(today(), PAGE_DAYS);
    try {
      const result = await getCalendar(from, to);
      items = result;
      earliestDate = from;
      latestDate = to;
      initialLoaded = true;
      await scrollToYesterday();
      hasPrevious = true;
    } finally {
      loading = false;
    }
  }

  async function loadMore() {
    if (loading) return;
    loading = true;
    const from = latestDate;
    const to = addDays(latestDate, PAGE_DAYS);
    try {
      const result = await getCalendar(from, to);
      if (result.length === 0) {
        hasMore = false;
      } else {
        items = [...items, ...result];
        latestDate = to;
      }
    } finally {
      loading = false;
    }
  }

  async function loadPrevious() {
    if (loading) return;
    loading = true;
    const to = earliestDate;
    const from = addDays(earliestDate, -PAGE_DAYS);
    try {
      const result = await getCalendar(from, to);
      if (result.length === 0) {
        hasPrevious = false;
      } else {
        items = [...result, ...items];
        earliestDate = from;
      }
    } finally {
      loading = false;
    }
  }

  // Reload when filter changes
  $effect(() => {
    // Access filter to track it
    void filter;
    loadInitial();
  });

  // Group items by date, applying filter
  const filteredItems = $derived(
    filter.length === 0 ? items : items.filter((i) => filter.includes(i.media_type))
  );

  const grouped = $derived.by(() => {
    const groups: { date: string; label: string; isToday: boolean; items: CalendarItem[] }[] = [];
    const map = new Map<string, CalendarItem[]>();

    for (const item of filteredItems) {
      const existing = map.get(item.date);
      if (existing) {
        existing.push(item);
      } else {
        const arr = [item];
        map.set(item.date, arr);
      }
    }

    const todayStr = today();

    for (const [date, dateItems] of map) {
      groups.push({
        date,
        label: formatDateLabel(date, todayStr),
        isToday: date === todayStr,
        items: dateItems
      });
    }

    groups.sort((a, b) => a.date.localeCompare(b.date));
    return groups;
  });

  function formatDateLabel(dateStr: string, todayStr: string): string {
    const tomorrow = addDays(todayStr, 1);
    const yesterday = addDays(todayStr, -1);

    if (dateStr === todayStr) return 'Today';
    if (dateStr === tomorrow) return 'Tomorrow';
    if (dateStr === yesterday) return 'Yesterday';

    const d = new Date(dateStr + 'T00:00:00');

    const weekday = d.toLocaleDateString('en-US', { weekday: 'long' });
    const month = d.toLocaleDateString('en-US', { month: 'short' });
    const day = d.getDate();

    return `${weekday}, ${month} ${day}`;
  }

  function isPast(dateStr: string): boolean {
    return dateStr < today();
  }
</script>

{#if !initialLoaded}
  <div class="loading-state">
    <span class="spinner"></span>
    Loading calendar...
  </div>
{:else if filteredItems.length === 0}
  <div class="empty-state">
    <span class="material-icons-outlined empty-icon">event_busy</span>
    <p>No upcoming releases.</p>
    <p class="empty-hint">Monitor TV shows and want movies/books to see them here.</p>
  </div>
{:else}
  <BidirectionalScroll
    {hasMore}
    {hasPrevious}
    {loading}
    onLoadMore={loadMore}
    onLoadPrevious={loadPrevious}
  >
    {#each grouped as group (group.date)}
      <div
        class="date-group"
        class:past={isPast(group.date)}
        class:today={group.isToday}
        data-date={group.date}
      >
        <div class="date-header" class:today-header={group.isToday}>
          <span class="date-label">{group.label}</span>
          {#if group.isToday}
            <span class="today-dot"></span>
          {/if}
          <span class="date-full">{group.date}</span>
        </div>
        <div class="date-items">
          {#each group.items as item (item.id)}
            <CalendarItemCard {item} />
          {/each}
        </div>
      </div>
    {/each}
  </BidirectionalScroll>
{/if}

<style>
  .loading-state {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 48px 16px;
    color: var(--muted);
    font-size: 14px;
  }

  .spinner {
    display: inline-block;
    width: 16px;
    height: 16px;
    border: 2px solid currentColor;
    border-right-color: transparent;
    border-radius: 50%;
    animation: spin 0.75s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 64px 16px;
    text-align: center;
    color: var(--muted);
  }

  .empty-icon {
    font-size: 48px;
    opacity: 0.3;
  }

  .empty-state p {
    margin: 0;
    font-size: 14px;
  }

  .empty-hint {
    font-size: 12px;
    opacity: 0.7;
  }

  .date-group {
    margin-bottom: 12px;
    /* scrollIntoView (used to land on yesterday on load) honours this:
       leaves room for the sticky toolbar above. */
    scroll-margin-top: 72px;
  }

  .date-group.past {
    opacity: 0.55;
  }

  .date-group.today {
    opacity: 1;
  }

  .date-header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 4px 8px;
    position: sticky;
    /* Stack under the page-level sticky toolbar */
    top: 64px;
    z-index: 2;
    background: var(--bg);
  }

  .date-label {
    font-size: 14px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--muted);
  }

  .today-header .date-label {
    color: var(--accent);
  }

  .today-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
  }

  .date-full {
    font-size: 11px;
    color: var(--muted);
    opacity: 0.6;
    margin-left: auto;
  }

  .date-items {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  @media (max-width: 768px) {
    .date-header {
      top: 56px;
      padding: 10px 4px 6px;
    }

    .date-items {
      gap: 4px;
    }

    .date-group {
      margin-bottom: 4px;
    }
  }
</style>
