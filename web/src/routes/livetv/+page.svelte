<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { page } from '$app/stores';
  import { SvelteSet } from 'svelte/reactivity';
  import {
    onReconnect,
    offReconnect,
    on as onLiveEvent,
    off as offLiveEvent
  } from '$lib/services/eventstream.svelte';
  import ErrorBanner from '$lib/components/shared/ErrorBanner.svelte';
  import LivetvTabs from '$lib/components/livetv/LivetvTabs.svelte';
  import DayStepper from '$lib/components/livetv/DayStepper.svelte';
  import FacetPills from '$lib/components/livetv/FacetPills.svelte';
  import FacetToggles from '$lib/components/livetv/FacetToggles.svelte';
  import EpgGrid from '$lib/components/livetv/EpgGrid.svelte';
  import ProgrammeDetail from '$lib/components/livetv/ProgrammeDetail.svelte';
  import { networkTint } from '$lib/components/livetv/tint';
  import { showMessage } from '$lib/context/app.svelte';
  import { getLivetvState, fetchLivetvTuners } from '$lib/stores/livetv.svelte';
  import { openLivetvChannel, preWarmLivetvChannel } from '$lib/stores/livetvPlayer.svelte';
  import { updateUrlParams } from '$lib/utils/url-state';
  import * as api from '$lib/api/client';
  import type {
    LivetvChannel,
    LivetvFacets,
    LivetvGuideChannel,
    LivetvGuideEntry
  } from '$lib/types';

  const livetv = getLivetvState();

  let channels = $state<LivetvChannel[]>([]);
  let facets = $state<LivetvFacets | null>(null);
  let guideRows = $state<LivetvGuideChannel[]>([]);
  let loading = $state(true);
  let error = $state('');

  let day = $state(startOfDay(new Date()));
  let now = $state(new Date());
  let network = $state<string | null>(null);
  const regions = new SvelteSet<string>();
  let hdOnly = $state(false);
  let includeRadio = $state(false);
  let searchQuery = $state('');
  let searchInput = $state('');

  let selected = $state<{ entry: LivetvGuideEntry; channel: LivetvChannel } | null>(null);
  let detailEl = $state<HTMLDivElement | undefined>();
  let recordBusy = $state(false);

  // The grid is as tall as the channel list, so the detail panel below it opens
  // off-screen and picking a programme reads as a dead tap.
  $effect(() => {
    if (selected && detailEl) {
      detailEl.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    }
  });

  async function scheduleSelected() {
    const chosen = selected;
    if (!chosen || recordBusy) return;
    recordBusy = true;
    try {
      await api.createLivetvRecording({ guide_id: chosen.entry.id });
      showMessage(`Recording ${chosen.entry.title}`);
      await loadGuide();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Could not schedule that recording');
    } finally {
      recordBusy = false;
    }
  }

  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let clockTimer: ReturnType<typeof setInterval> | undefined;
  let reconnectHandler: (() => void) | null = null;
  let guideSyncHandler: ((data: unknown) => void) | null = null;
  let channelGeneration = 0;
  let guideGeneration = 0;

  function startOfDay(d: Date): Date {
    const c = new Date(d);
    c.setHours(0, 0, 0, 0);
    return c;
  }

  function endOfDay(d: Date): Date {
    const c = startOfDay(d);
    c.setDate(c.getDate() + 1);
    return c;
  }

  const filteredChannels = $derived(
    channels.filter((c) => {
      if (network && (c.network ?? '') !== network) return false;
      if (regions.size > 0 && !regions.has(c.region ?? '')) return false;
      if (hdOnly && !c.hd) return false;
      if (!includeRadio && c.type_label === 'Radio') return false;
      return true;
    })
  );

  const networkPills = $derived(
    (facets?.network ?? []).map((f) => ({
      value: f.value,
      label: f.label,
      count: f.count,
      tint: networkTint(f.value)
    }))
  );

  const togglePills = $derived([
    ...(facets?.region ?? []).map((f) => ({
      value: `region:${f.value}`,
      label: f.label,
      active: regions.has(f.value)
    })),
    { value: 'hd', label: 'HD only', active: hdOnly },
    { value: 'radio', label: 'Radio', active: includeRadio }
  ]);

  const guideHorizon = $derived.by(() => {
    let latest = 0;
    for (const row of guideRows) {
      for (const entry of row.entries) {
        const t = new Date(entry.end_time).getTime();
        if (t > latest) latest = t;
      }
    }
    if (!latest) return null;
    return new Intl.DateTimeFormat(undefined, {
      weekday: 'short',
      hour: '2-digit',
      minute: '2-digit'
    }).format(new Date(latest));
  });

  const subtitle = $derived.by(() => {
    const tuner = livetv.primary;
    const parts: string[] = [];
    if (tuner) parts.push(tuner.friendly_name || tuner.model_number || tuner.device_id);
    parts.push(`${channels.length} channel${channels.length === 1 ? '' : 's'}`);
    if (guideHorizon) parts.push(`guide to ${guideHorizon}`);
    return parts.join(' · ');
  });

  async function loadChannels() {
    const gen = ++channelGeneration;
    try {
      const result = await api.getLivetvChannels({ enabled: true, limit: 500 });
      if (gen !== channelGeneration) return;
      channels = result.items ?? [];
      facets = result.facets ?? null;
      error = '';
    } catch (e) {
      if (gen !== channelGeneration) return;
      console.error('Failed to load Live TV channels:', e);
      error = 'Failed to load the channel lineup. Check the tuner and try again.';
      channels = [];
      facets = null;
    }
  }

  async function loadGuide() {
    const gen = ++guideGeneration;
    loading = true;
    try {
      const ids = filteredChannels.map((c) => c.id);
      if (ids.length === 0) {
        if (gen !== guideGeneration) return;
        guideRows = [];
        return;
      }
      const result = await api.getLivetvGuide({
        from: startOfDay(day).toISOString(),
        to: endOfDay(day).toISOString(),
        channel_ids: ids.join(','),
        search: searchQuery.trim() || undefined
      });
      if (gen !== guideGeneration) return;
      guideRows = result.channels ?? [];
      if (selected && !guideRows.some((r) => r.entries.some((e) => e.id === selected?.entry.id))) {
        selected = null;
      }
      error = '';
    } catch (e) {
      if (gen !== guideGeneration) return;
      console.error('Failed to load guide:', e);
      error = 'Failed to load the guide. Check the tuner and try again.';
      guideRows = [];
    } finally {
      if (gen === guideGeneration) loading = false;
    }
  }

  async function reload() {
    await loadChannels();
    await loadGuide();
  }

  function applyFilters() {
    updateUrlParams({
      date: startOfDay(day).toISOString().slice(0, 10),
      network,
      regions: regions.size > 0 ? Array.from(regions).join(',') : null,
      hd: hdOnly ? '1' : null,
      radio: includeRadio ? '1' : null,
      q: searchQuery.trim() || null
    });
    loadGuide();
  }

  function handleToggle(value: string) {
    if (value === 'hd') hdOnly = !hdOnly;
    else if (value === 'radio') includeRadio = !includeRadio;
    else {
      const region = value.slice('region:'.length);
      if (regions.has(region)) regions.delete(region);
      else regions.add(region);
    }
    applyFilters();
  }

  function handleSearchInput(value: string) {
    searchInput = value;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      searchQuery = value;
      applyFilters();
    }, 300);
  }

  function readUrlState() {
    const params = $page.url.searchParams;
    const date = params.get('date');
    if (date) {
      const parsed = new Date(`${date}T00:00:00`);
      if (!Number.isNaN(parsed.getTime())) day = startOfDay(parsed);
    }
    network = params.get('network');
    const regionParam = params.get('regions');
    regions.clear();
    if (regionParam) for (const r of regionParam.split(',')) regions.add(r);
    hdOnly = params.get('hd') === '1';
    includeRadio = params.get('radio') === '1';
    searchQuery = params.get('q') ?? '';
    searchInput = searchQuery;
  }

  onMount(() => {
    readUrlState();
    fetchLivetvTuners();
    reload();

    clockTimer = setInterval(() => {
      now = new Date();
    }, 30_000);

    reconnectHandler = onReconnect(() => {
      fetchLivetvTuners(true);
      reload();
    });

    guideSyncHandler = onLiveEvent('livetv_guide_synced', () => {
      reload();
    });
  });

  onDestroy(() => {
    if (reconnectHandler) offReconnect(reconnectHandler);
    if (guideSyncHandler) offLiveEvent('livetv_guide_synced', guideSyncHandler);
    clearTimeout(searchTimer);
    clearInterval(clockTimer);
  });
</script>

<svelte:head><title>Live TV · kobbler</title></svelte:head>

<div class="page-container livetv">
  <header class="head">
    <div class="titles">
      <h1>Live TV</h1>
      <span class="subtitle">{subtitle}</span>
    </div>
    <div class="head-actions">
      <label class="search">
        <span class="material-icons-outlined">search</span>
        <input
          type="search"
          placeholder="Search channels &amp; guide"
          value={searchInput}
          oninput={(e) => handleSearchInput(e.currentTarget.value)}
        />
      </label>
      <DayStepper
        value={day}
        onChange={(next) => {
          day = next;
          applyFilters();
        }}
      />
    </div>
  </header>

  <LivetvTabs active="guide" />

  {#if error}
    <ErrorBanner message={error} onRetry={reload} />
  {/if}

  <div class="filters">
    <FacetPills
      options={networkPills}
      value={network}
      onChange={(next) => {
        network = next;
        applyFilters();
      }}
    />
    {#if togglePills.length > 0}
      <span class="divider"></span>
      <FacetToggles options={togglePills} onToggle={handleToggle} />
    {/if}
  </div>

  {#if loading && guideRows.length === 0}
    <div class="loading-card">Loading guide…</div>
  {:else if guideRows.length === 0}
    <div class="empty-state">
      <h3>{channels.length === 0 ? 'No tuner channels yet' : 'Nothing in the guide'}</h3>
      <p class="muted">
        {#if channels.length === 0}
          Register an HDHomeRun and run a scan from
          <a href="/settings/livetv">Live TV settings</a>.
        {:else}
          No programmes match these filters for this day.
        {/if}
      </p>
    </div>
  {:else}
    <EpgGrid
      channels={guideRows}
      windowStart={startOfDay(day)}
      windowEnd={endOfDay(day)}
      {now}
      selectedEntryId={selected?.entry.id ?? null}
      onSelect={(entry, channel) => (selected = { entry, channel })}
    />

    {#if selected}
      {@const chosen = selected}
      <div bind:this={detailEl}>
        <ProgrammeDetail
          entry={chosen.entry}
          channel={chosen.channel}
          onRecord={scheduleSelected}
          {recordBusy}
          onWatchIntent={() => preWarmLivetvChannel(chosen.channel)}
          onWatch={() => openLivetvChannel(chosen.channel)}
        />
      </div>
    {/if}
  {/if}
</div>

<style>
  .livetv {
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
    gap: 2px;
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
    flex-wrap: wrap;
  }

  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 34px;
    padding: 0 12px;
    border: 1px solid var(--ring);
    border-radius: 9px;
    background: var(--card);
    min-width: 220px;
    color: var(--muted);
  }

  .search input {
    background: none;
    border: none;
    outline: none;
    color: var(--ink);
    font-size: 12.5px;
    width: 100%;
  }

  .search .material-icons-outlined {
    font-size: 16px;
  }

  .filters {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 14px;
    flex-wrap: wrap;
  }

  .divider {
    width: 1px;
    height: 22px;
    background: var(--ring);
    margin: 0 4px;
  }

  .empty-state a {
    color: var(--accent);
  }
</style>
