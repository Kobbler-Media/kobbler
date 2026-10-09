<script lang="ts">
  import type { ActivityEvent, LoadState } from '$lib/types';
  import { InfiniteScroll } from '@koblibs/ui';
  import { createDelayedLoading } from '$lib/utils/delayed-loading.svelte';

  type ViewEntityHandler = (url: string) => void;

  interface Props {
    events: ActivityEvent[];
    nextCursor: string | null;
    loading: boolean;
    loadState: LoadState;
    activityTotal: number;
    onLoadMore: () => void;
    onViewEntity: ViewEntityHandler;
  }

  let { events, nextCursor, loading, loadState, activityTotal, onLoadMore, onViewEntity }: Props =
    $props();

  const initialLoadingState = createDelayedLoading(false);
  $effect(() => {
    initialLoadingState.loading = loadState.status === 'loading';
  });

  const EVENT_CONFIG: Record<string, { icon: string; color: string; label: string }> = {
    imported: { icon: 'download_done', color: 'var(--success)', label: 'Imported' },
    deleted: { icon: 'delete', color: 'var(--danger)', label: 'Deleted' },
    grabbed: { icon: 'cloud_download', color: '#60a5fa', label: 'Grabbed' },
    failed: { icon: 'error', color: 'var(--danger)', label: 'Failed' },
    synced: { icon: 'sync', color: 'var(--accent)', label: 'Synced' },
    merged: { icon: 'merge_type', color: '#a78bfa', label: 'Merged' },
    delivered: { icon: 'send', color: 'var(--accent)', label: 'Delivered' }
  };

  const ENTITY_LABELS: Record<string, string> = {
    book: 'Book',
    movie: 'Movie',
    episode: 'Episode',
    show: 'TV Show',
    season: 'Season',
    author: 'Author',
    series: 'Series',
    device: 'Device'
  };

  function getDayLabel(dateStr: string): string {
    const date = new Date(dateStr);
    const now = new Date();
    const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    const eventDay = new Date(date.getFullYear(), date.getMonth(), date.getDate());
    const diffDays = Math.floor((today.getTime() - eventDay.getTime()) / 86400000);

    if (diffDays === 0) return 'Today';
    if (diffDays === 1) return 'Yesterday';
    if (diffDays < 7) {
      return date.toLocaleDateString(undefined, { weekday: 'long' });
    }
    return date.toLocaleDateString(undefined, {
      weekday: 'long',
      day: 'numeric',
      month: 'long'
    });
  }

  function relativeTime(dateStr: string): string {
    const now = Date.now();
    const then = new Date(dateStr).getTime();
    const diffMs = now - then;
    const diffMin = Math.floor(diffMs / 60000);
    if (diffMin < 1) return 'just now';
    if (diffMin < 60) return `${diffMin}m ago`;
    const diffHr = Math.floor(diffMin / 60);
    if (diffHr < 24) return `${diffHr}h ago`;
    return new Date(dateStr).toLocaleTimeString(undefined, {
      hour: 'numeric',
      minute: '2-digit'
    });
  }

  type DayGroup = { label: string; events: ActivityEvent[] };

  const grouped = $derived.by(() => {
    const groups: DayGroup[] = [];
    let currentLabel = '';
    let currentGroup: ActivityEvent[] = [];

    for (const event of events) {
      const label = getDayLabel(event.created_at);
      if (label !== currentLabel) {
        if (currentGroup.length > 0) {
          groups.push({ label: currentLabel, events: currentGroup });
        }
        currentLabel = label;
        currentGroup = [event];
      } else {
        currentGroup.push(event);
      }
    }
    if (currentGroup.length > 0) {
      groups.push({ label: currentLabel, events: currentGroup });
    }
    return groups;
  });
</script>

<div class="activity-page page-container">
  {#if initialLoadingState.showLoading && events.length === 0}
    <div class="loading-card">Loading activity...</div>
  {:else if loadState.status === 'error'}
    <div class="error-card">{loadState.error}</div>
  {:else}
    <div class="section-header">
      <h2>Recent Activity</h2>
      {#if activityTotal > 0}
        <span class="event-count">{activityTotal} events</span>
      {/if}
    </div>

    {#if grouped.length === 0}
      <div class="empty-state">
        <span class="material-icons-outlined empty-icon">history</span>
        <h3>No activity yet</h3>
        <p>Imports, downloads, syncs, and other events will appear here.</p>
      </div>
    {:else}
      {#each grouped as group (group.label)}
        <div class="day-group">
          <div class="day-label">{group.label}</div>
          <div class="day-events">
            {#each group.events as event (event.id)}
              {@const config = EVENT_CONFIG[event.event_kind] ?? {
                icon: 'info',
                color: 'var(--muted)',
                label: event.event_kind
              }}
              <div class="event-row">
                <div class="event-icon" style:--icon-color={config.color}>
                  <span class="material-icons-outlined">{config.icon}</span>
                </div>

                {#if event.image_url}
                  <div
                    class="event-thumb"
                    class:poster={event.entity_type === 'movie' || event.entity_type === 'show'}
                    style:background-image="url({event.image_url})"
                  ></div>
                {/if}

                <div class="event-body">
                  <div class="event-title-row">
                    {#if event.url}
                      <button
                        class="event-title clickable"
                        type="button"
                        onclick={() => onViewEntity(event.url!)}
                      >
                        {event.title}
                      </button>
                    {:else}
                      <span class="event-title">{event.title}</span>
                    {/if}
                  </div>
                  <div class="event-meta">
                    <span class="event-kind-label" style:color={config.color}>{config.label}</span>
                    {#if event.entity_type}
                      <span class="meta-sep">·</span>
                      <span>{ENTITY_LABELS[event.entity_type] ?? event.entity_type}</span>
                    {/if}
                    {#if event.subtitle}
                      <span class="meta-sep">·</span>
                      <span>{event.subtitle}</span>
                    {/if}
                    <span class="meta-sep">·</span>
                    <span>{relativeTime(event.created_at)}</span>
                  </div>
                </div>
              </div>
            {/each}
          </div>
        </div>
      {/each}

      <InfiniteScroll hasMore={nextCursor !== null} {loading} {onLoadMore} />
    {/if}
  {/if}
</div>

<style>
  .loading-card,
  .error-card {
    padding: 24px;
    text-align: center;
    border-radius: 16px;
    border: 1px solid var(--ring);
    background: var(--card);
  }

  .error-card {
    color: var(--danger);
    border-color: var(--danger);
  }

  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 20px;
  }

  .section-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .event-count {
    font-size: 12px;
    color: var(--muted);
  }

  .empty-state {
    padding: 48px 24px;
    border-radius: 16px;
    border: 1px dashed var(--ring);
    background: var(--card);
    text-align: center;
  }

  .empty-icon {
    font-size: 40px;
    color: var(--muted);
    opacity: 0.5;
    margin-bottom: 12px;
  }

  .empty-state h3 {
    margin: 0 0 8px;
    font-size: 16px;
  }

  .empty-state p {
    margin: 0;
    color: var(--muted);
    font-size: 13px;
  }

  /* Day groups */
  .day-group {
    margin-bottom: 24px;
  }

  .day-label {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted);
    margin-bottom: 8px;
    padding-left: 4px;
  }

  .day-events {
    display: grid;
    gap: 6px;
  }

  /* Event row */
  .event-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    border-radius: 12px;
    border: 1px solid var(--ring);
    background: var(--card);
    transition: border-color 0.15s;
  }

  .event-row:hover {
    border-color: color-mix(in srgb, var(--ring) 50%, var(--ink) 50%);
  }

  /* Icon */
  .event-icon {
    width: 32px;
    height: 32px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 8px;
    background: color-mix(in srgb, var(--icon-color) 15%, transparent);
  }

  .event-icon .material-icons-outlined {
    font-size: 18px;
    color: var(--icon-color);
  }

  /* Thumbnail */
  .event-thumb {
    width: 36px;
    height: 48px;
    flex-shrink: 0;
    background-size: cover;
    background-position: center;
    border-radius: 6px;
    border: 1px solid var(--ring);
  }

  .event-thumb.poster {
    width: 48px;
    height: 36px;
    border-radius: 6px;
  }

  /* Body */
  .event-body {
    flex: 1;
    min-width: 0;
  }

  .event-title-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 2px;
  }

  .event-title {
    font-weight: 600;
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .event-title.clickable {
    all: unset;
    font-weight: 600;
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    cursor: pointer;
    color: var(--ink);
    transition: color 0.15s;
  }

  .event-title.clickable:hover {
    color: var(--accent);
  }

  .event-meta {
    display: flex;
    align-items: center;
    gap: 5px;
    color: var(--muted);
    font-size: 11px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .event-kind-label {
    font-weight: 600;
  }

  .meta-sep {
    opacity: 0.4;
  }

  @media (max-width: 768px) {
    .event-row {
      gap: 8px;
      padding: 8px 10px;
    }

    .event-meta {
      white-space: normal;
      flex-wrap: wrap;
    }
  }
</style>
