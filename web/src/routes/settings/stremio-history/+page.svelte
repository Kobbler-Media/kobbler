<script lang="ts">
  import { onMount } from 'svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import { formatTimeAgo } from '$lib/stores/jobs.svelte';
  import * as api from '$lib/api/client';
  import type { StremioHistoryItem } from '$lib/api/client';

  type Filter = 'available' | 'imported' | 'all';

  let items = $state<StremioHistoryItem[]>([]);
  let linked = $state(true);
  let status = $state<'loading' | 'ready' | 'error'>('loading');
  let error = $state('');
  let query = $state('');
  let filter = $state<Filter>('available');
  let picked = $state<Set<string>>(new Set());
  let busy = $state(false);

  const load = async () => {
    try {
      const res = await api.getStremioHistory();
      linked = res.linked;
      items = res.items;
      status = 'ready';
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load Stremio history';
      status = 'error';
    }
  };

  onMount(load);

  const visible = $derived(
    items.filter((i) => {
      if (filter === 'available' && i.imported) return false;
      if (filter === 'imported' && !i.imported) return false;
      return !query.trim() || i.title.toLowerCase().includes(query.trim().toLowerCase());
    })
  );

  const selectable = $derived(visible.filter((i) => !i.imported));
  const allPicked = $derived(
    selectable.length > 0 && selectable.every((i) => picked.has(i.stremio_id))
  );

  const toggle = (id: string) => {
    const next = new Set(picked);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    picked = next;
  };

  const toggleAll = () => {
    const next = new Set(picked);
    for (const i of selectable) {
      if (allPicked) next.delete(i.stremio_id);
      else next.add(i.stremio_id);
    }
    picked = next;
  };

  const importPicked = async () => {
    busy = true;
    try {
      const { selected } = await api.importStremioHistory([...picked]);
      showMessage(
        `Importing ${selected} title${selected === 1 ? '' : 's'} — they'll reach What to Watch shortly`
      );
      picked = new Set();
      await load();
    } catch {
      showMessage('Import failed');
    } finally {
      busy = false;
    }
  };

  const stop = async (item: StremioHistoryItem) => {
    busy = true;
    try {
      await api.stopStremioImport(item.stremio_id);
      showMessage(`Stopped following ${item.title}`);
      await load();
    } catch {
      showMessage('Failed to stop following');
    } finally {
      busy = false;
    }
  };
</script>

<svelte:head>
  <title>Stremio History — Kobbler</title>
</svelte:head>

<div class="history-section">
  <div class="section-header">
    <h2>Stremio History</h2>
    {#if picked.size > 0}
      <button class="btn" type="button" onclick={importPicked} disabled={busy}>
        Import {picked.size} selected
      </button>
    {/if}
  </div>

  <p class="muted intro">
    Pick the shows and movies you watched on the shared Stremio account. Kobbler marks them watched
    for your profile only, so What to Watch picks up the next episode or a new season. Imported
    titles keep following Stremio on every sync until you stop them; nothing is ever un-watched.
  </p>

  {#if status === 'loading'}
    <div class="muted">Loading history…</div>
  {:else if status === 'error'}
    <div class="error">{error}</div>
  {:else if !linked}
    <div class="empty-state">
      <h3>No Stremio account linked</h3>
      <p class="muted">An admin can link one under Settings → Streaming.</p>
    </div>
  {:else}
    <div class="toolbar">
      <input class="search" type="search" placeholder="Filter titles" bind:value={query} />
      <div class="tabs" role="tablist">
        {#each [['available', 'Not imported'], ['imported', 'Imported'], ['all', 'All']] as [value, label] (value)}
          <button
            type="button"
            role="tab"
            class="tab"
            class:active={filter === value}
            aria-selected={filter === value}
            onclick={() => (filter = value as Filter)}>{label}</button
          >
        {/each}
      </div>
      {#if selectable.length > 0}
        <button class="btn alt small" type="button" onclick={toggleAll}>
          {allPicked ? 'Clear selection' : 'Select all'}
        </button>
      {/if}
    </div>

    {#if visible.length === 0}
      <div class="empty-state">
        <p class="muted">Nothing here.</p>
      </div>
    {:else}
      <div class="history-list">
        {#each visible as item (item.stremio_id)}
          <label class="history-row" class:picked={picked.has(item.stremio_id)}>
            {#if item.imported}
              <span class="check-spacer"></span>
            {:else}
              <input
                type="checkbox"
                checked={picked.has(item.stremio_id)}
                onchange={() => toggle(item.stremio_id)}
              />
            {/if}
            {#if item.poster_url}
              <img class="poster" src={item.poster_url} alt="" loading="lazy" />
            {:else}
              <span class="poster"></span>
            {/if}
            <span class="info">
              <span class="title">
                {item.title}{#if item.year}<span class="muted"> ({item.year})</span>{/if}
                {#if item.imported}<span class="pill">Imported</span>{/if}
              </span>
              <span class="meta muted">
                {item.media_type === 'tv' ? 'Series' : 'Movie'} · {item.progress} · {formatTimeAgo(
                  item.last_watched
                )}
              </span>
              {#if item.import_error}
                <span class="meta import-error">{item.import_error}</span>
              {/if}
            </span>
            {#if item.imported}
              <button
                class="btn alt small"
                type="button"
                onclick={() => stop(item)}
                disabled={busy}
              >
                Stop following
              </button>
            {/if}
          </label>
        {/each}
      </div>
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

  .import-error {
    color: var(--danger);
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

  .empty-state h3 {
    margin: 0 0 8px;
    font-size: 16px;
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
</style>
