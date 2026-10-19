<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { page } from '$app/stores';
  import { SvelteSet } from 'svelte/reactivity';
  import { formatAge } from '@koblibs/ui';
  import {
    onReconnect,
    offReconnect,
    onDebounced,
    on as onLiveEvent,
    off as offLiveEvent
  } from '$lib/services/eventstream.svelte';
  import ErrorBanner from '$lib/components/shared/ErrorBanner.svelte';
  import LivetvTabs from '$lib/components/livetv/LivetvTabs.svelte';
  import ChannelFacetRail from '$lib/components/livetv/ChannelFacetRail.svelte';
  import type { FacetGroupId } from '$lib/components/livetv/types';
  import ChannelTable from '$lib/components/livetv/ChannelTable.svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import { getLivetvState, fetchLivetvTuners } from '$lib/stores/livetv.svelte';
  import { openLivetvChannel, preWarmLivetvChannel } from '$lib/stores/livetvPlayer.svelte';
  import { updateUrlParams } from '$lib/utils/url-state';
  import MetaLine from '$lib/components/shared/MetaLine.svelte';
  import * as api from '$lib/api/client';
  import type { LivetvBulkAction, LivetvChannel, LivetvFacet, LivetvFacets } from '$lib/types';

  const livetv = getLivetvState();

  let channels = $state<LivetvChannel[]>([]);
  let total = $state(0);
  let baselineTotal = $state(0);
  let baselineFacets = $state<LivetvFacets | null>(null);
  let facets = $state<LivetvFacets | null>(null);
  let loading = $state(true);
  let error = $state('');
  let bulkBusy = $state(false);

  let searchQuery = $state('');
  let searchInput = $state('');
  let selectedFacets = $state<Record<FacetGroupId, string | null>>({
    network: null,
    region: null,
    type: null,
    guide: null,
    status: null
  });

  const selectedIds = new SvelteSet<string>();
  const busyIds = new SvelteSet<string>();

  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let reconnectHandler: (() => void) | null = null;
  let scanHandler: ((data: unknown) => void) | null = null;
  let guideHandler: ((data: unknown) => void) | null = null;
  let loadGeneration = 0;
  let lastClickedId: string | null = null;

  const PAGE_LIMIT = 500;

  function facetCount(list: LivetvFacet[] | undefined, ...keys: string[]): number {
    if (!list) return 0;
    const wanted = keys.map((k) => k.toLowerCase());
    return list
      .filter(
        (f) => wanted.includes(f.value.toLowerCase()) || wanted.includes(f.label.toLowerCase())
      )
      .reduce((n, f) => n + f.count, 0);
  }

  const stats = $derived.by(() => {
    const f = baselineFacets;
    const enabled = facetCount(f?.status, 'enabled', 'true');
    const hd = facetCount(f?.type, 'hd');
    const radio = facetCount(f?.type, 'radio');
    const dead = facetCount(f?.type, 'unnamed', 'dead', 'unnamed / dead');
    const withGuide = facetCount(f?.guide, 'true', 'has_guide', 'has guide data');
    const muxes = f?.mux?.length ?? 0;
    return [
      { value: baselineTotal, label: 'services found', tone: 'ink', sub: null },
      { value: enabled, label: 'enabled', tone: 'ink', sub: `of ${baselineTotal}` },
      { value: hd, label: 'HD', tone: 'hd', sub: null },
      { value: radio, label: 'radio', tone: 'radio', sub: null },
      { value: dead, label: 'unnamed / dead', tone: 'dim', sub: 'auto-hidden' },
      { value: withGuide, label: 'with guide data', tone: 'guide', sub: null },
      {
        value: muxes,
        label: `mux${muxes === 1 ? '' : 'es'}`,
        tone: 'ink',
        sub: livetv.tunerCount > 0 ? `${livetv.tunerCount} tuners` : null
      }
    ];
  });

  const headerLine = $derived.by(() => {
    const tuner = livetv.primary;
    if (!tuner) return ['No tuner registered yet'];
    const parts = [tuner.friendly_name || tuner.model_number, tuner.device_id];
    try {
      parts.push(new URL(tuner.base_url).hostname);
    } catch {
      // base_url may be a bare host during manual registration
    }
    if (tuner.last_scan_at) parts.push(`scanned ${formatAge(tuner.last_scan_at)}`);
    return parts;
  });

  function queryParams() {
    const params: api.LivetvChannelQuery = { limit: PAGE_LIMIT };
    if (searchQuery.trim()) params.search = searchQuery.trim();
    if (selectedFacets.network) params.network = selectedFacets.network;
    if (selectedFacets.region) params.region = selectedFacets.region;
    if (selectedFacets.type) params.kind = selectedFacets.type;
    if (selectedFacets.guide) params.has_guide = selectedFacets.guide === 'true';
    const status = selectedFacets.status;
    if (status === 'enabled') params.enabled = true;
    else if (status === 'hidden') params.enabled = false;
    else if (status === 'favourite') params.favourite = true;
    return params;
  }

  async function loadChannels() {
    const gen = ++loadGeneration;
    loading = true;
    try {
      const result = await api.getLivetvChannels(queryParams());
      if (gen !== loadGeneration) return;
      channels = result.items ?? [];
      total = result.total ?? channels.length;
      facets = result.facets ?? null;
      error = '';
    } catch (e) {
      if (gen !== loadGeneration) return;
      console.error('Failed to load Live TV channels:', e);
      error = 'Failed to load the scanned lineup. Check the tuner and try again.';
      channels = [];
      total = 0;
      facets = null;
    } finally {
      if (gen === loadGeneration) loading = false;
    }
  }

  async function loadBaseline() {
    try {
      const result = await api.getLivetvChannels({ limit: 1 });
      baselineTotal = result.total ?? 0;
      baselineFacets = result.facets ?? null;
    } catch {
      baselineTotal = 0;
      baselineFacets = null;
    }
  }

  async function reload() {
    await Promise.all([loadBaseline(), loadChannels()]);
  }

  function persist() {
    updateUrlParams({
      search: searchQuery.trim() || null,
      network: selectedFacets.network,
      region: selectedFacets.region,
      kind: selectedFacets.type,
      guide: selectedFacets.guide,
      status: selectedFacets.status
    });
  }

  function handleFacetSelect(group: FacetGroupId, value: string | null) {
    selectedFacets = { ...selectedFacets, [group]: value };
    persist();
    loadChannels();
  }

  function clearFacets() {
    selectedFacets = { network: null, region: null, type: null, guide: null, status: null };
    persist();
    loadChannels();
  }

  function handleSearchInput(value: string) {
    searchInput = value;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      searchQuery = value;
      persist();
      loadChannels();
    }, 300);
  }

  function toggleSelect(id: string, range: boolean) {
    if (range && lastClickedId) {
      const ids = channels.map((c) => c.id);
      const from = ids.indexOf(lastClickedId);
      const to = ids.indexOf(id);
      if (from >= 0 && to >= 0) {
        const [lo, hi] = from < to ? [from, to] : [to, from];
        for (let i = lo; i <= hi; i++) selectedIds.add(ids[i]);
        lastClickedId = id;
        return;
      }
    }
    if (selectedIds.has(id)) selectedIds.delete(id);
    else selectedIds.add(id);
    lastClickedId = id;
  }

  function selectAllShown() {
    for (const c of channels) selectedIds.add(c.id);
  }

  function clearSelection() {
    selectedIds.clear();
    lastClickedId = null;
  }

  async function toggleEnabled(channel: LivetvChannel) {
    busyIds.add(channel.id);
    try {
      const updated = await api.updateLivetvChannel(channel.id, { enabled: !channel.enabled });
      channels = channels.map((c) => (c.id === channel.id ? updated : c));
      loadBaseline();
    } catch (e) {
      console.error('Failed to update channel:', e);
      showMessage('Failed to update the channel');
    } finally {
      busyIds.delete(channel.id);
    }
  }

  async function bulk(action: LivetvBulkAction) {
    if (selectedIds.size === 0) return;
    bulkBusy = true;
    try {
      const result = await api.bulkUpdateLivetvChannels(Array.from(selectedIds), action);
      showMessage(`${result.updated} channel${result.updated === 1 ? '' : 's'} updated`);
      clearSelection();
      await reload();
    } catch (e) {
      console.error('Bulk update failed:', e);
      showMessage('Bulk update failed');
    } finally {
      bulkBusy = false;
    }
  }

  async function rescan() {
    const tuner = livetv.primary;
    if (!tuner) return;
    try {
      await api.scanLivetvTuner(tuner.id);
      showMessage('Channel scan queued');
    } catch (e) {
      console.error('Failed to queue scan:', e);
      showMessage('Failed to queue the scan');
    }
  }

  function readUrlState() {
    const params = $page.url.searchParams;
    searchQuery = params.get('search') ?? '';
    searchInput = searchQuery;
    selectedFacets = {
      network: params.get('network'),
      region: params.get('region'),
      type: params.get('kind'),
      guide: params.get('guide'),
      status: params.get('status')
    };
  }

  onMount(() => {
    readUrlState();
    fetchLivetvTuners();
    reload();

    reconnectHandler = onReconnect(() => {
      fetchLivetvTuners(true);
      reload();
    });

    scanHandler = onDebounced('livetv_scan_progress', () => {
      fetchLivetvTuners(true);
      reload();
    });
    guideHandler = onLiveEvent('livetv_guide_synced', () => {
      reload();
    });
  });

  onDestroy(() => {
    if (reconnectHandler) offReconnect(reconnectHandler);
    if (scanHandler) offLiveEvent('livetv_scan_progress', scanHandler);
    if (guideHandler) offLiveEvent('livetv_guide_synced', guideHandler);
    clearTimeout(searchTimer);
  });
</script>

<svelte:head><title>Live TV channels · kobbler</title></svelte:head>

<div class="page-container livetv-channels">
  <header class="head">
    <div class="titles">
      <h1>Channels</h1>
      <span class="subtitle"><MetaLine items={headerLine} /></span>
    </div>
    <div class="head-actions">
      <button type="button" class="btn-sec" disabled={!livetv.primary} onclick={rescan}>
        <span class="material-icons-outlined">refresh</span>
        Rescan
      </button>
    </div>
  </header>

  <LivetvTabs active="channels" />

  {#if error}
    <ErrorBanner message={error} onRetry={reload} />
  {/if}

  <div class="stats">
    {#each stats as stat (stat.label)}
      <div class="stat">
        <span class="stat-value" data-tone={stat.tone}>{stat.value}</span>
        <span class="stat-label">{stat.label}</span>
        {#if stat.sub}<span class="stat-sub">{stat.sub}</span>{/if}
      </div>
    {/each}
  </div>

  <div class="layout">
    <ChannelFacetRail
      facets={facets ?? baselineFacets}
      selected={selectedFacets}
      onSelect={handleFacetSelect}
      onClear={clearFacets}
    />

    <section class="panel">
      <div class="toolbar">
        <label class="search">
          <span class="material-icons-outlined">search</span>
          <input
            type="search"
            placeholder={baselineTotal
              ? `Search ${baselineTotal} scanned services`
              : 'Search scanned services'}
            value={searchInput}
            oninput={(e) => handleSearchInput(e.currentTarget.value)}
          />
        </label>

        <span class="counts">
          <strong>{total}</strong> shown · {selectedIds.size} selected
        </span>

        <button
          type="button"
          class="link-btn"
          disabled={channels.length === 0}
          onclick={selectAllShown}
        >
          Select all
        </button>
        <button
          type="button"
          class="link-btn"
          disabled={selectedIds.size === 0}
          onclick={clearSelection}
        >
          Clear
        </button>

        <span class="spacer"></span>

        <div class="bulk">
          <button
            type="button"
            class="bulk-btn"
            disabled={selectedIds.size === 0 || bulkBusy}
            onclick={() => bulk('enable')}
          >
            Enable
          </button>
          <button
            type="button"
            class="bulk-btn"
            disabled={selectedIds.size === 0 || bulkBusy}
            onclick={() => bulk('hide')}
          >
            Hide
          </button>
          <button
            type="button"
            class="bulk-btn"
            disabled={selectedIds.size === 0 || bulkBusy}
            onclick={() => bulk('favourite')}
          >
            Favourite
          </button>
        </div>
      </div>

      <div class="table-scroll">
        {#if loading && channels.length === 0}
          <div class="loading-card">Loading lineup…</div>
        {:else if channels.length === 0}
          <div class="empty-state">
            <h3>{baselineTotal === 0 ? 'Nothing scanned yet' : 'No services match'}</h3>
            <p class="muted">
              {#if baselineTotal === 0}
                Register an HDHomeRun and run a scan from
                <a href="/settings/livetv">Live TV settings</a>.
              {:else}
                Clear a filter or widen the search.
              {/if}
            </p>
          </div>
        {:else}
          <ChannelTable
            {channels}
            {selectedIds}
            {busyIds}
            onToggleSelect={toggleSelect}
            onToggleEnabled={toggleEnabled}
            onWatch={openLivetvChannel}
            onWatchIntent={preWarmLivetvChannel}
          />
        {/if}
      </div>

      <footer class="note">
        <span class="dot"></span>
        Curation is keyed on <strong>mux frequency + program number</strong>, not LCN — renames,
        favourites and hidden flags survive a rescan even if the broadcaster renumbers.
      </footer>
    </section>
  </div>
</div>

<style>
  .livetv-channels {
    padding-bottom: 40px;
  }

  .head {
    display: flex;
    align-items: flex-start;
    gap: 14px;
    margin-bottom: 16px;
    flex-wrap: wrap;
  }

  .titles {
    display: flex;
    flex-direction: column;
    gap: 3px;
    flex-grow: 1;
    min-width: 0;
  }

  h1 {
    margin: 0;
    font-size: 24px;
    font-weight: 600;
    letter-spacing: -0.02em;
  }

  .subtitle {
    font-size: 12px;
    color: var(--muted);
  }

  .head-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .head-actions .material-icons-outlined {
    font-size: 16px;
  }

  .stats {
    display: flex;
    gap: 10px;
    margin-bottom: 16px;
    flex-wrap: wrap;
  }

  .stat {
    flex-grow: 1;
    min-width: 120px;
    box-sizing: border-box;
    border: 1px solid var(--ring);
    border-radius: 11px;
    background: var(--card);
    padding: 11px 14px;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .stat-value {
    font-size: 20px;
    font-weight: 700;
    letter-spacing: -0.02em;
    font-variant-numeric: tabular-nums;
    color: var(--ink);
  }

  .stat-value[data-tone='hd'] {
    color: #5fcf8a;
  }
  .stat-value[data-tone='radio'] {
    color: #a98ce5;
  }
  .stat-value[data-tone='guide'] {
    color: #6dc3d6;
  }
  .stat-value[data-tone='dim'] {
    color: var(--muted);
  }

  .stat-label {
    font-size: 11.5px;
    color: var(--muted);
  }

  .stat-sub {
    font-size: 10px;
    color: var(--muted);
    opacity: 0.75;
  }

  .layout {
    display: flex;
    gap: 16px;
    align-items: flex-start;
  }

  .panel {
    flex-grow: 1;
    min-width: 0;
    box-sizing: border-box;
    border: 1px solid var(--ring);
    border-radius: 12px;
    background: var(--card);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 14px;
    border-bottom: 1px solid var(--ring);
    flex-wrap: wrap;
  }

  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 32px;
    padding: 0 11px;
    border: 1px solid var(--ring);
    border-radius: 9px;
    background: var(--bg);
    flex-grow: 1;
    max-width: 280px;
    color: var(--muted);
  }

  .search input {
    background: none;
    border: none;
    outline: none;
    color: var(--ink);
    font-size: 12px;
    width: 100%;
  }

  .search .material-icons-outlined {
    font-size: 15px;
  }

  .counts {
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
  }

  .counts strong {
    color: var(--ink);
  }

  .link-btn {
    background: none;
    border: none;
    padding: 0;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--accent);
    cursor: pointer;
  }

  .link-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .spacer {
    flex-grow: 1;
  }

  .bulk {
    display: flex;
    align-items: center;
    gap: 7px;
  }

  .bulk-btn {
    height: 30px;
    padding: 0 12px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: transparent;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--muted);
    cursor: pointer;
  }

  .bulk-btn:hover:not(:disabled) {
    color: var(--ink);
    border-color: var(--accent);
  }

  .bulk-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .table-scroll {
    overflow-x: auto;
  }

  .note {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 11px 14px;
    border-top: 1px solid var(--ring);
    font-size: 11.5px;
    color: var(--muted);
  }

  .note strong {
    color: var(--ink);
    font-weight: 600;
  }

  .note .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
    flex-shrink: 0;
  }

  .empty-state a {
    color: var(--accent);
  }

  @media (max-width: 900px) {
    .layout {
      flex-direction: column;
    }
  }
</style>
