<script lang="ts">
  import { showMessage } from '#lib/context/app.svelte.js';
  import { formatTimeAgo } from '#lib/stores/jobs.svelte.js';
  import * as api from '#lib/api/client.js';
  import MetaLine from '#lib/components/shared/MetaLine.svelte';
  import type { WatchHistoryItem, WatchHistorySource } from '#lib/api/client.js';

  const PAGE = 100;
  const SOURCES: [WatchHistorySource | null, string][] = [
    [null, 'All'],
    ['local', 'Library'],
    ['trakt', 'Trakt'],
    ['stremio', 'Stremio']
  ];
  const SOURCE_LABELS: Record<WatchHistorySource, string> = {
    local: 'Library',
    trakt: 'Trakt',
    stremio: 'Stremio'
  };

  let items = $state<WatchHistoryItem[]>([]);
  let total = $state(0);
  let status = $state<'loading' | 'ready' | 'error'>('loading');
  let error = $state('');
  let query = $state('');
  let source = $state<WatchHistorySource | null>(null);
  let picked = $state<Set<string>>(new Set());
  let busy = $state(false);
  let loadingMore = $state(false);

  const keyOf = (i: WatchHistoryItem) => `${i.media_type}:${i.tmdb_id ?? i.title}`;

  const fetchPage = (offset: number) =>
    api.getWatchHistory({
      source: source ?? undefined,
      search: query.trim() || undefined,
      limit: PAGE,
      offset
    });

  const load = async () => {
    try {
      const res = await fetchPage(0);
      items = res.items;
      total = res.total;
      picked = new Set();
      status = 'ready';
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load watch history';
      status = 'error';
    }
  };

  const loadMore = async () => {
    loadingMore = true;
    try {
      const res = await fetchPage(items.length);
      items = [...items, ...res.items];
      total = res.total;
    } catch {
      showMessage('Failed to load more');
    } finally {
      loadingMore = false;
    }
  };

  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    void source;
    void query;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(load, 250);
    return () => clearTimeout(searchTimer);
  });

  const selectable = $derived(items.filter((i) => i.tmdb_id != null));
  const chosen = $derived(selectable.filter((i) => picked.has(keyOf(i))));
  const allPicked = $derived(selectable.length > 0 && chosen.length === selectable.length);
  const unhide = $derived(chosen.length > 0 && chosen.every((i) => i.excluded));

  const titles = () => chosen.map((i) => ({ media_type: i.media_type, tmdb_id: i.tmdb_id! }));

  const toggle = (item: WatchHistoryItem) => {
    const next = new Set(picked);
    const k = keyOf(item);
    if (next.has(k)) next.delete(k);
    else next.add(k);
    picked = next;
  };

  const toggleAll = () => {
    picked = allPicked ? new Set() : new Set(selectable.map(keyOf));
  };

  const run = async (action: () => Promise<void>, done: string, failed: string) => {
    busy = true;
    try {
      await action();
      showMessage(done);
      await load();
    } catch {
      showMessage(failed);
    } finally {
      busy = false;
    }
  };

  const toggleExcluded = () => {
    const n = chosen.length;
    const exclude = !unhide;
    run(
      () => api.setWatchHistoryExcluded(titles(), exclude),
      exclude
        ? `${n} title${n === 1 ? '' : 's'} hidden from recommendations`
        : `${n} title${n === 1 ? '' : 's'} used for recommendations again`,
      'Update failed'
    );
  };

  const removePicked = () => {
    const n = chosen.length;
    if (
      !confirm(
        `Remove ${n} title${n === 1 ? '' : 's'} from your history? Watched marks and resume points are cleared. Stremio titles can't be deleted here, so they are hidden from recommendations instead.`
      )
    ) {
      return;
    }
    run(() => api.removeWatchHistory(titles()), `Removed ${n} from history`, 'Remove failed');
  };
</script>

<svelte:head>
  <title>Watch History — Kobbler</title>
</svelte:head>

<div class="history-section">
  <div class="section-header">
    <h2>Watch History</h2>
    {#if chosen.length > 0}
      <div class="actions">
        <button class="btn" type="button" onclick={toggleExcluded} disabled={busy}>
          {unhide ? 'Use for recommendations' : 'Hide from recommendations'}
        </button>
        <button class="btn alt danger" type="button" onclick={removePicked} disabled={busy}>
          Remove {chosen.length} from history
        </button>
      </div>
    {/if}
  </div>

  <p class="muted intro">
    Everything For You learns from. Imported Trakt or Stremio history can include what other people
    in the house watched: hide those titles so they stop shaping recommendations, or remove them to
    clear the watched marks too. Nothing is changed on Trakt or Stremio.
  </p>

  <div class="toolbar">
    <input class="search" type="search" placeholder="Filter titles" bind:value={query} />
    <div class="tabs" role="tablist">
      {#each SOURCES as [value, label] (label)}
        <button
          type="button"
          role="tab"
          class="tab"
          class:active={source === value}
          aria-selected={source === value}
          onclick={() => (source = value)}>{label}</button
        >
      {/each}
    </div>
    {#if selectable.length > 0}
      <button class="btn alt small" type="button" onclick={toggleAll}>
        {allPicked ? 'Clear selection' : 'Select all'}
      </button>
    {/if}
  </div>

  {#if status === 'loading'}
    <div class="muted">Loading history…</div>
  {:else if status === 'error'}
    <div class="error">{error}</div>
  {:else if items.length === 0}
    <div class="empty-state">
      <p class="muted">Nothing here.</p>
    </div>
  {:else}
    <div class="muted count">
      {chosen.length > 0 ? `${chosen.length} selected of ${total}` : `${total} titles`}
    </div>
    <div class="history-list">
      {#each items as item (keyOf(item))}
        <label class="history-row" class:picked={picked.has(keyOf(item))}>
          {#if item.tmdb_id == null}
            <span class="check-spacer"></span>
          {:else}
            <input
              type="checkbox"
              checked={picked.has(keyOf(item))}
              onchange={() => toggle(item)}
            />
          {/if}
          {#if item.poster_url}
            <img class="poster" src={item.poster_url} alt="" loading="lazy" />
          {:else}
            <span class="poster"></span>
          {/if}
          <span class="info">
            <span class="title" class:excluded={item.excluded}>
              {item.title}{#if item.year}<span class="muted"> ({item.year})</span>{/if}
              {#if item.excluded}<span class="pill">Hidden</span>{/if}
            </span>
            <span class="meta muted">
              <MetaLine
                items={[
                  item.media_type === 'tv' ? 'Series' : 'Movie',
                  item.sources.map((s) => SOURCE_LABELS[s] ?? s).join(', '),
                  item.last_watched_at ? formatTimeAgo(item.last_watched_at) : null
                ]}
              />
            </span>
          </span>
        </label>
      {/each}
    </div>
    {#if items.length < total}
      <button class="btn alt more" type="button" onclick={loadMore} disabled={loadingMore}>
        {loadingMore ? 'Loading…' : `Show more (${total - items.length} left)`}
      </button>
    {/if}
  {/if}
</div>

<style>
  .history-section {
    display: grid;
    gap: 16px;
  }

  .section-header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }

  .section-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .intro {
    margin: 0;
    font-size: 13px;
    max-width: 60ch;
  }

  .toolbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px;
  }

  .search {
    flex: 1;
    min-width: 160px;
    padding: 8px 12px;
    border-radius: 12px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
  }

  .tabs {
    display: flex;
    gap: 4px;
  }

  .tab {
    padding: 6px 12px;
    border-radius: 999px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    font-size: 12px;
    cursor: pointer;
  }

  .tab.active {
    background: var(--accent);
    border-color: var(--accent);
    color: white;
  }

  .history-list {
    display: grid;
    gap: 8px;
  }

  .history-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 12px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    cursor: pointer;
  }

  .history-row.picked {
    border-color: var(--accent);
  }

  .check-spacer {
    width: 13px;
    flex-shrink: 0;
  }

  .poster {
    width: 36px;
    height: 54px;
    border-radius: 6px;
    object-fit: cover;
    background: var(--ring);
    flex-shrink: 0;
  }

  .info {
    display: grid;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }

  .title {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    font-weight: 600;
    font-size: 14px;
  }

  .meta {
    font-size: 11px;
  }

  .pill {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 2px 8px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    color: var(--accent);
  }

  .btn {
    padding: 8px 16px;
    border-radius: 12px;
    border: none;
    background: var(--accent);
    color: white;
    font-weight: 600;
    font-size: 13px;
    cursor: pointer;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn.alt {
    background: var(--card);
    border: 1px solid var(--ring);
    color: var(--ink);
  }

  .btn.small {
    padding: 6px 10px;
    font-size: 12px;
  }

  .empty-state {
    padding: 32px;
    text-align: center;
    background: var(--card);
    border: 1px dashed var(--ring);
    border-radius: 16px;
  }

  .empty-state p {
    margin: 0;
  }

  .error {
    padding: 16px;
    text-align: center;
    border-radius: 12px;
    border: 1px solid var(--danger);
    background: var(--card);
    color: var(--danger);
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .btn.danger {
    color: var(--danger);
  }

  .title.excluded {
    opacity: 0.6;
  }

  .count {
    font-size: 12px;
  }

  .more {
    justify-self: center;
  }
</style>
