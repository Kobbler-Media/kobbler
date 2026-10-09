<script lang="ts">
  import type { CalendarItem } from '$lib/types';
  import { getCalendar } from '$lib/api/client';
  import { mediaTypeChip } from '$lib/utils/chip-mappings';
  import CalendarItemCard from './CalendarItemCard.svelte';

  interface Props {
    filter: string[];
  }

  let { filter }: Props = $props();

  // Current month being viewed
  let viewYear = $state(new Date().getFullYear());
  let viewMonth = $state(new Date().getMonth()); // 0-based

  let items = $state<CalendarItem[]>([]);
  let loading = $state(false);

  // Expanded day (click to see full list)
  let expandedDate = $state<string | null>(null);

  const MAX_VISIBLE = 3;

  function toDateStr(d: Date): string {
    const y = d.getFullYear();
    const m = String(d.getMonth() + 1).padStart(2, '0');
    const day = String(d.getDate()).padStart(2, '0');
    return `${y}-${m}-${day}`;
  }

  function today(): string {
    return toDateStr(new Date());
  }

  const monthLabel = $derived(
    new Date(viewYear, viewMonth).toLocaleDateString('en-US', {
      month: 'long',
      year: 'numeric'
    })
  );

  // Build calendar grid data
  const calendarWeeks = $derived.by(() => {
    const firstDay = new Date(viewYear, viewMonth, 1);
    const lastDay = new Date(viewYear, viewMonth + 1, 0);

    // Start on Monday (ISO week)
    let startDay = firstDay.getDay() - 1;
    if (startDay < 0) startDay = 6;

    const weeks: { date: string; day: number; inMonth: boolean; isToday: boolean }[][] = [];
    let currentWeek: (typeof weeks)[0] = [];

    // Fill leading days from previous month
    const prevMonth = new Date(viewYear, viewMonth, 0);
    for (let i = startDay - 1; i >= 0; i--) {
      const d = new Date(viewYear, viewMonth, -i);
      currentWeek.push({
        date: toDateStr(d),
        day: prevMonth.getDate() - i,
        inMonth: false,
        isToday: toDateStr(d) === today()
      });
    }

    // Fill days of current month
    for (let day = 1; day <= lastDay.getDate(); day++) {
      const d = new Date(viewYear, viewMonth, day);
      currentWeek.push({
        date: toDateStr(d),
        day,
        inMonth: true,
        isToday: toDateStr(d) === today()
      });
      if (currentWeek.length === 7) {
        weeks.push(currentWeek);
        currentWeek = [];
      }
    }

    // Fill trailing days
    if (currentWeek.length > 0) {
      let nextDay = 1;
      while (currentWeek.length < 7) {
        const d = new Date(viewYear, viewMonth + 1, nextDay);
        currentWeek.push({
          date: toDateStr(d),
          day: nextDay,
          inMonth: false,
          isToday: toDateStr(d) === today()
        });
        nextDay++;
      }
      weeks.push(currentWeek);
    }

    return weeks;
  });

  // Filter items
  const filteredItems = $derived(
    filter.length === 0 ? items : items.filter((i) => filter.includes(i.media_type))
  );

  // Group filtered items by date
  const itemsByDate = $derived.by(() => {
    const map = new Map<string, CalendarItem[]>();
    for (const item of filteredItems) {
      const existing = map.get(item.date);
      if (existing) {
        existing.push(item);
      } else {
        map.set(item.date, [item]);
      }
    }
    return map;
  });

  // Items for expanded date
  const expandedItems = $derived(expandedDate ? (itemsByDate.get(expandedDate) ?? []) : []);

  async function fetchMonth() {
    loading = true;
    // Fetch a bit wider range to include leading/trailing days
    const from = toDateStr(new Date(viewYear, viewMonth - 1, 20));
    const to = toDateStr(new Date(viewYear, viewMonth + 1, 15));
    try {
      items = await getCalendar(from, to);
    } finally {
      loading = false;
    }
  }

  function prevMonth() {
    expandedDate = null;
    if (viewMonth === 0) {
      viewMonth = 11;
      viewYear--;
    } else {
      viewMonth--;
    }
  }

  function nextMonth() {
    expandedDate = null;
    if (viewMonth === 11) {
      viewMonth = 0;
      viewYear++;
    } else {
      viewMonth++;
    }
  }

  function goToday() {
    expandedDate = null;
    const now = new Date();
    viewYear = now.getFullYear();
    viewMonth = now.getMonth();
  }

  function toggleExpand(date: string) {
    expandedDate = expandedDate === date ? null : date;
  }

  // Fetch on mount and month change
  $effect(() => {
    void viewYear;
    void viewMonth;
    fetchMonth();
  });

  const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
</script>

<div class="grid-container">
  <div class="grid-header">
    <button class="nav-btn" onclick={prevMonth} aria-label="Previous month">
      <span class="material-icons-outlined">chevron_left</span>
    </button>
    <button class="month-label" onclick={goToday}>{monthLabel}</button>
    <button class="nav-btn" onclick={nextMonth} aria-label="Next month">
      <span class="material-icons-outlined">chevron_right</span>
    </button>
  </div>

  {#if loading}
    <div class="loading-overlay">
      <span class="spinner"></span>
    </div>
  {/if}

  <div class="weekday-row">
    {#each WEEKDAYS as day (day)}
      <div class="weekday-label">{day}</div>
    {/each}
  </div>

  <div class="calendar-grid">
    {#each calendarWeeks as week, weekIdx (weekIdx)}
      {#each week as cell (cell.date)}
        {@const dayItems = itemsByDate.get(cell.date) ?? []}
        {@const hasItems = dayItems.length > 0}
        {@const isExpanded = expandedDate === cell.date}
        <button
          class="day-cell"
          class:out-of-month={!cell.inMonth}
          class:today={cell.isToday}
          class:has-items={hasItems}
          class:expanded={isExpanded}
          onclick={() => hasItems && toggleExpand(cell.date)}
          disabled={!hasItems}
          type="button"
        >
          <span class="day-number">{cell.day}</span>
          {#if hasItems}
            <div class="day-items">
              {#each dayItems.slice(0, MAX_VISIBLE) as item (item.id)}
                {@const chip = mediaTypeChip(item.media_type)}
                <div class="day-item" style="--type-color: {chip.color}">
                  {#if item.image_url}
                    <img class="day-poster" src={item.image_url} alt="" loading="lazy" />
                  {:else}
                    <span
                      class="day-poster-placeholder material-icons-outlined"
                      style="color: {chip.color}">{chip.icon}</span
                    >
                  {/if}
                  <span class="day-item-title">{item.title}</span>
                </div>
              {/each}
              {#if dayItems.length > MAX_VISIBLE}
                <span class="overflow-count">+{dayItems.length - MAX_VISIBLE} more</span>
              {/if}
            </div>
          {/if}
        </button>
      {/each}
    {/each}
  </div>

  {#if expandedDate && expandedItems.length > 0}
    <div class="expanded-panel">
      <div class="expanded-header">
        <span class="expanded-date">
          {new Date(expandedDate + 'T00:00:00').toLocaleDateString('en-US', {
            weekday: 'long',
            month: 'long',
            day: 'numeric'
          })}
        </span>
        <button class="close-btn" onclick={() => (expandedDate = null)} aria-label="Close">
          <span class="material-icons-outlined">close</span>
        </button>
      </div>
      <div class="expanded-items">
        {#each expandedItems as item (item.id)}
          <CalendarItemCard {item} />
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .grid-container {
    position: relative;
  }

  .grid-header {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 12px;
    margin-bottom: 16px;
  }

  .nav-btn {
    appearance: none;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 8px;
    color: var(--ink);
    cursor: pointer;
    padding: 4px;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background 0.12s ease;
  }

  .nav-btn:hover {
    background: color-mix(in srgb, var(--accent) 12%, var(--card));
  }

  .nav-btn .material-icons-outlined {
    font-size: 20px;
  }

  .month-label {
    appearance: none;
    background: transparent;
    border: none;
    color: var(--ink);
    font-size: 20px;
    font-weight: 700;
    cursor: pointer;
    padding: 4px 12px;
    border-radius: 8px;
    transition: background 0.12s ease;
    min-width: 220px;
    text-align: center;
  }

  .month-label:hover {
    background: color-mix(in srgb, var(--accent) 8%, transparent);
  }

  .loading-overlay {
    position: absolute;
    top: 50px;
    left: 0;
    right: 0;
    display: flex;
    justify-content: center;
    padding: 8px;
    z-index: 5;
    pointer-events: none;
  }

  .spinner {
    display: inline-block;
    width: 16px;
    height: 16px;
    border: 2px solid var(--accent);
    border-right-color: transparent;
    border-radius: 50%;
    animation: spin 0.75s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .weekday-row {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 2px;
    margin-bottom: 2px;
  }

  .weekday-label {
    font-size: 13px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--muted);
    text-align: center;
    padding: 8px 0;
  }

  .calendar-grid {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 4px;
  }

  .day-cell {
    appearance: none;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 8px;
    padding: 6px;
    min-height: 120px;
    display: flex;
    flex-direction: column;
    gap: 3px;
    cursor: default;
    text-align: left;
    color: inherit;
    font: inherit;
    transition:
      border-color 0.12s ease,
      background 0.12s ease;
    overflow: hidden;
  }

  .day-cell.has-items {
    cursor: pointer;
  }

  .day-cell.has-items:hover {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 4%, var(--card));
  }

  .day-cell.out-of-month {
    opacity: 0.35;
  }

  .day-cell.today {
    border-color: var(--accent);
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  .day-cell.expanded {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 8%, var(--card));
  }

  .day-number {
    font-size: 13px;
    font-weight: 600;
    color: var(--muted);
    padding: 0 2px;
    line-height: 1;
  }

  .today .day-number {
    color: var(--accent);
    font-weight: 800;
  }

  .day-items {
    display: flex;
    flex-direction: column;
    gap: 1px;
    overflow: hidden;
    flex: 1;
  }

  .day-item {
    display: flex;
    align-items: center;
    gap: 3px;
    min-width: 0;
    border-left: 2px solid var(--type-color);
    padding-left: 3px;
    border-radius: 2px;
  }

  .day-poster {
    width: 20px;
    height: 28px;
    border-radius: 2px;
    object-fit: cover;
    flex-shrink: 0;
  }

  .day-poster-placeholder {
    font-size: 16px;
    flex-shrink: 0;
    width: 20px;
    text-align: center;
  }

  .day-item-title {
    font-size: 12px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    line-height: 1.2;
  }

  .overflow-count {
    font-size: 11px;
    color: var(--muted);
    padding-left: 5px;
    font-weight: 600;
  }

  .expanded-panel {
    margin-top: 8px;
    padding: 12px;
    background: var(--card);
    border: 1px solid var(--accent);
    border-radius: 12px;
    animation: slideDown 0.15s ease;
  }

  @keyframes slideDown {
    from {
      opacity: 0;
      transform: translateY(-4px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  .expanded-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 10px;
  }

  .expanded-date {
    font-size: 14px;
    font-weight: 700;
  }

  .close-btn {
    appearance: none;
    background: transparent;
    border: none;
    color: var(--muted);
    cursor: pointer;
    padding: 2px;
    display: flex;
    align-items: center;
    border-radius: 6px;
    transition: color 0.12s ease;
  }

  .close-btn:hover {
    color: var(--ink);
  }

  .close-btn .material-icons-outlined {
    font-size: 18px;
  }

  .expanded-items {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  /* Scale up further on truly large screens */
  @media (min-width: 1200px) {
    .day-cell {
      min-height: 150px;
    }

    .day-item-title {
      font-size: 13px;
    }

    .day-poster {
      width: 24px;
      height: 34px;
    }
  }

  /* Responsive: on smaller screens, reduce cell height */
  @media (max-width: 768px) {
    .day-cell {
      min-height: 60px;
      padding: 3px;
    }

    .day-poster {
      display: none;
    }

    .day-item-title {
      font-size: 9px;
    }
  }
</style>
