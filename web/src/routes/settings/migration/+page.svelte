<script lang="ts">
  import { onMount } from 'svelte';
  import { getAuthState } from '$lib/stores/auth.svelte';
  import { SearchSelect } from '@koblibs/ui';
  import type { MediaRoot } from '$lib/types';

  const auth = getAuthState();

  // ---- Types matching backend responses ----

  interface ConnectionState {
    url: string;
    apiKey: string;
    status: 'idle' | 'testing' | 'success' | 'error';
    version: string | null;
    error: string | null;
  }

  interface EpisodeStats {
    total: number;
    with_file: number;
    monitored: number;
  }

  interface PreviewItem {
    title: string;
    year: number | null;
    tmdb_id: string | null;
    tvdb_id: string | null;
    imdb_id: string | null;
    monitored: boolean;
    has_files: boolean;
    source_status: string | null;
    import_status: string;
    kobbler_id: string | null;
    source_id: number;
    episode_stats: EpisodeStats | null;
    will_want: boolean;
  }

  interface RootFolderMapping {
    path: string;
    suggested_media_root_id: string | null;
    suggested_media_root_name: string | null;
  }

  interface PreviewSummary {
    total: number;
    new: number;
    existing: number;
    no_tmdb_id: number;
  }

  interface PreviewResponse {
    service: string;
    summary: PreviewSummary;
    items: PreviewItem[];
    root_folders: RootFolderMapping[];
  }

  interface SkippedItem {
    title: string;
    reason: string;
  }

  interface ImportResult {
    imported: number;
    skipped: number;
    skipped_reasons: SkippedItem[];
    scans_triggered: string[];
    wants_enqueued: number;
  }

  type Service = 'sonarr' | 'radarr';

  // ---- API helpers ----

  async function apiGet<T>(path: string): Promise<T> {
    const resp = await fetch(path, {
      credentials: 'include'
    });
    if (!resp.ok) {
      const err = await resp.json().catch(() => ({ error: resp.statusText }));
      throw new Error(err.error || resp.statusText);
    }
    return resp.json();
  }

  async function apiPost<T>(path: string, body: unknown): Promise<T> {
    const resp = await fetch(path, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      credentials: 'include',
      body: JSON.stringify(body)
    });
    if (!resp.ok) {
      const err = await resp.json().catch(() => ({ error: resp.statusText }));
      throw new Error(err.error || resp.statusText);
    }
    return resp.json();
  }

  // ---- State ----

  let step = $state(1);

  // Step 1: Connection
  let sonarr = $state<ConnectionState>({
    url: '',
    apiKey: '',
    status: 'idle',
    version: null,
    error: null
  });

  let radarr = $state<ConnectionState>({
    url: '',
    apiKey: '',
    status: 'idle',
    version: null,
    error: null
  });

  let configLoading = $state(true);

  // Step 2: Preview
  let activeTab = $state<Service>('sonarr');
  let previewLoading = $state(false);
  let previewError = $state<string | null>(null);

  let sonarrItems = $state<PreviewItem[]>([]);
  let radarrItems = $state<PreviewItem[]>([]);
  let sonarrRootFolders = $state<RootFolderMapping[]>([]);
  let radarrRootFolders = $state<RootFolderMapping[]>([]);
  let sonarrFetched = $state(false);
  let radarrFetched = $state(false);

  let sonarrSelected = $state<Set<string>>(new Set());
  let radarrSelected = $state<Set<string>>(new Set());

  let searchFilter = $state('');

  // Sonarr filter chips
  let sonarrFilterAiring = $state(true);
  let sonarrFilterEnded = $state(false);
  let sonarrFilterMonitored = $state(false);
  let sonarrFilterMissing = $state(false);

  // Radarr filter chips
  let radarrFilterMissing = $state(true);
  let radarrFilterHasFiles = $state(false);
  let radarrFilterMonitored = $state(false);

  // Root folder mapping: source path -> media root UUID
  let rootMapping = $state<Record<string, string>>({});
  let mediaRoots = $state<MediaRoot[]>([]);

  // Want missing content toggle
  let wantMissing = $state(true);

  // Step 3: Import
  let importing = $state(false);
  let importResult = $state<ImportResult | null>(null);
  let importError = $state<string | null>(null);

  // ---- Derived ----

  let connectedServices = $derived(
    [
      sonarr.status === 'success' ? 'sonarr' : null,
      radarr.status === 'success' ? 'radarr' : null
    ].filter((s): s is Service => s !== null)
  );

  let canPreview = $derived(connectedServices.length > 0);

  let currentItems = $derived(activeTab === 'sonarr' ? sonarrItems : radarrItems);
  let currentSelected = $derived(activeTab === 'sonarr' ? sonarrSelected : radarrSelected);
  let currentRootFolders = $derived(activeTab === 'sonarr' ? sonarrRootFolders : radarrRootFolders);

  let filteredItems = $derived(
    currentItems.filter((item) => {
      if (!searchFilter) return true;
      const q = searchFilter.toLowerCase();
      return item.title.toLowerCase().includes(q) || (item.year && String(item.year).includes(q));
    })
  );

  let allRootFolderPaths = $derived(
    [...new Set([...sonarrRootFolders, ...radarrRootFolders].map((rf) => rf.path))].sort()
  );

  let allRootsMapped = $derived(allRootFolderPaths.every((path) => rootMapping[path]));

  let totalSelected = $derived(sonarrSelected.size + radarrSelected.size);

  let summaryStats = $derived.by(() => {
    const items = currentItems;
    const total = items.length;
    const newCount = items.filter((i) => i.import_status === 'new').length;
    const existsCount = items.filter((i) => i.import_status === 'exists').length;
    const noTmdb = items.filter((i) => !i.tmdb_id).length;
    const selected = currentSelected.size;
    const willWant = items.filter(
      (i) => i.will_want && i.tmdb_id && currentSelected.has(i.tmdb_id)
    ).length;
    return { total, newCount, existsCount, noTmdb, selected, willWant };
  });

  let canImport = $derived(totalSelected > 0 && allRootsMapped);

  // ---- Helpers ----

  function hasMissingEpisodes(item: PreviewItem): boolean {
    if (!item.episode_stats) return false;
    return item.episode_stats.with_file < item.episode_stats.total;
  }

  function statusLabel(item: PreviewItem): string {
    if (activeTab === 'sonarr') {
      return item.source_status ?? 'unknown';
    }
    return item.has_files ? 'Has File' : 'Missing';
  }

  function mediaRootsForType(type: string): MediaRoot[] {
    return mediaRoots.filter((mr) => mr.media_type === type);
  }

  // ---- Lifecycle ----

  onMount(async () => {
    try {
      const [config, roots] = await Promise.all([
        apiGet<Record<string, string | null>>('/api/v1/admin/config'),
        apiGet<MediaRoot[]>('/api/v1/admin/media-roots')
      ]);
      if (config.sonarr_url) sonarr.url = config.sonarr_url;
      if (config.sonarr_api_key) sonarr.apiKey = config.sonarr_api_key;
      if (config.radarr_url) radarr.url = config.radarr_url;
      if (config.radarr_api_key) radarr.apiKey = config.radarr_api_key;
      mediaRoots = roots;
    } catch {
      // Config might not have these fields yet, that's fine
    } finally {
      configLoading = false;
    }
  });

  // ---- Actions ----

  async function persistConnection(service: Service) {
    const conn = service === 'sonarr' ? sonarr : radarr;
    try {
      const updates: Record<string, string | null> = {};
      if (service === 'sonarr') {
        updates.sonarr_url = conn.url || null;
        updates.sonarr_api_key = conn.apiKey || null;
      } else {
        updates.radarr_url = conn.url || null;
        updates.radarr_api_key = conn.apiKey || null;
      }
      const resp = await fetch('/api/v1/admin/config', {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'include',
        body: JSON.stringify(updates)
      });
      if (!resp.ok) throw new Error('Failed to persist');
    } catch {
      // Non-critical: connection still works, just won't be remembered
    }
  }

  async function testConnection(service: Service) {
    const conn = service === 'sonarr' ? sonarr : radarr;
    conn.status = 'testing';
    conn.error = null;
    conn.version = null;
    try {
      const result = await apiPost<{ ok: boolean; version: string | null }>(
        '/api/v1/admin/migration/test',
        { service, url: conn.url, api_key: conn.apiKey }
      );
      if (result.ok) {
        conn.status = 'success';
        conn.version = result.version;
        await persistConnection(service);
      } else {
        conn.status = 'error';
        conn.error = 'Connection failed';
      }
    } catch (e) {
      conn.status = 'error';
      conn.error = e instanceof Error ? e.message : 'Connection failed';
    }
  }

  function goToPreview() {
    if (connectedServices.length > 0) {
      activeTab = connectedServices[0];
      step = 2;
    }
  }

  async function fetchPreview() {
    previewLoading = true;
    previewError = null;
    try {
      const result = await apiPost<PreviewResponse>('/api/v1/admin/migration/preview', {
        service: activeTab
      });
      if (activeTab === 'sonarr') {
        sonarrItems = result.items;
        sonarrRootFolders = result.root_folders;
        sonarrFetched = true;
        applyFilterPresets('sonarr');
      } else {
        radarrItems = result.items;
        radarrRootFolders = result.root_folders;
        radarrFetched = true;
        applyFilterPresets('radarr');
      }
      // Pre-fill root mapping with backend suggestions
      for (const rf of result.root_folders) {
        if (rf.suggested_media_root_id && !rootMapping[rf.path]) {
          rootMapping[rf.path] = rf.suggested_media_root_id;
        }
      }
      rootMapping = { ...rootMapping };
    } catch (e) {
      previewError = e instanceof Error ? e.message : 'Failed to fetch preview';
    } finally {
      previewLoading = false;
    }
  }

  function applyFilterPresets(service: Service) {
    const items = service === 'sonarr' ? sonarrItems : radarrItems;
    const selected = new Set<string>();

    for (const item of items) {
      if (!item.tmdb_id) continue;
      if (item.import_status === 'exists') continue;

      let match = false;
      if (service === 'sonarr') {
        if (
          sonarrFilterAiring &&
          (item.source_status === 'continuing' || item.source_status === 'upcoming')
        )
          match = true;
        if (sonarrFilterEnded && item.source_status === 'ended') match = true;
        if (sonarrFilterMonitored && item.monitored) match = true;
        if (sonarrFilterMissing && hasMissingEpisodes(item)) match = true;
      } else {
        if (radarrFilterMissing && !item.has_files && item.monitored) match = true;
        if (radarrFilterHasFiles && item.has_files) match = true;
        if (radarrFilterMonitored && item.monitored) match = true;
      }
      if (match) selected.add(item.tmdb_id);
    }

    if (service === 'sonarr') {
      sonarrSelected = selected;
    } else {
      radarrSelected = selected;
    }
  }

  function toggleItem(tmdbId: string) {
    const selected = activeTab === 'sonarr' ? sonarrSelected : radarrSelected;
    const next = new Set(selected);
    if (next.has(tmdbId)) {
      next.delete(tmdbId);
    } else {
      next.add(tmdbId);
    }
    if (activeTab === 'sonarr') {
      sonarrSelected = next;
    } else {
      radarrSelected = next;
    }
  }

  function selectAll() {
    const ids = new Set(filteredItems.filter((i) => i.tmdb_id).map((i) => i.tmdb_id!));
    if (activeTab === 'sonarr') {
      sonarrSelected = ids;
    } else {
      radarrSelected = ids;
    }
  }

  function deselectAll() {
    if (activeTab === 'sonarr') {
      sonarrSelected = new Set();
    } else {
      radarrSelected = new Set();
    }
  }

  async function startImport() {
    importing = true;
    importError = null;
    importResult = null;

    const batches: { service: Service; tmdb_ids: string[] }[] = [];
    if (sonarrSelected.size > 0) {
      batches.push({ service: 'sonarr', tmdb_ids: [...sonarrSelected] });
    }
    if (radarrSelected.size > 0) {
      batches.push({ service: 'radarr', tmdb_ids: [...radarrSelected] });
    }

    try {
      let totalImported = 0;
      let totalSkipped = 0;
      let totalWantsEnqueued = 0;
      let allSkippedReasons: SkippedItem[] = [];
      let allScansTriggered: string[] = [];

      for (const batch of batches) {
        const result = await apiPost<ImportResult>('/api/v1/admin/migration/import', {
          service: batch.service,
          tmdb_ids: batch.tmdb_ids,
          root_mapping: rootMapping,
          want_missing: wantMissing
        });
        totalImported += result.imported;
        totalSkipped += result.skipped;
        totalWantsEnqueued += result.wants_enqueued ?? 0;
        allSkippedReasons = [...allSkippedReasons, ...result.skipped_reasons];
        allScansTriggered = [...allScansTriggered, ...result.scans_triggered];
      }

      importResult = {
        imported: totalImported,
        skipped: totalSkipped,
        skipped_reasons: allSkippedReasons,
        scans_triggered: allScansTriggered,
        wants_enqueued: totalWantsEnqueued
      };
      step = 3;
    } catch (e) {
      importError = e instanceof Error ? e.message : 'Import failed';
    } finally {
      importing = false;
    }
  }
</script>

<svelte:head>
  <title>Migration — Kobbler</title>
</svelte:head>

{#if !auth.isAdmin}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <div class="migration-page">
    <div class="page-header">
      <h2>Sonarr / Radarr Migration</h2>
      <p class="subtitle">Import your existing library from Sonarr and Radarr into Kobbler.</p>
    </div>

    <!-- Step indicator -->
    <div class="steps-bar">
      <button
        class="step-dot"
        class:active={step === 1}
        class:done={step > 1}
        onclick={() => {
          if (step > 1) step = 1;
        }}
      >
        <span class="material-icons-outlined">
          {step > 1 ? 'check_circle' : 'link'}
        </span>
        <span class="step-label">Connect</span>
      </button>
      <div class="step-line" class:active={step >= 2}></div>
      <button
        class="step-dot"
        class:active={step === 2}
        class:done={step > 2}
        disabled={!canPreview}
        onclick={() => {
          if (step > 2 && canPreview) step = 2;
        }}
      >
        <span class="material-icons-outlined">
          {step > 2 ? 'check_circle' : 'preview'}
        </span>
        <span class="step-label">Preview</span>
      </button>
      <div class="step-line" class:active={step >= 3}></div>
      <button class="step-dot" class:active={step === 3} disabled>
        <span class="material-icons-outlined">download</span>
        <span class="step-label">Import</span>
      </button>
    </div>

    <!-- Step 1: Connect -->
    {#if step === 1}
      <div class="connect-grid">
        {#each [{ key: 'sonarr' as Service, label: 'Sonarr', icon: 'live_tv', placeholder: 'http://sonarr:8989', conn: sonarr }, { key: 'radarr' as Service, label: 'Radarr', icon: 'movie', placeholder: 'http://radarr:7878', conn: radarr }] as svc (svc.key)}
          <div class="connect-card">
            <div class="connect-card-header">
              <span class="material-icons-outlined service-icon">{svc.icon}</span>
              <h3>{svc.label}</h3>
              {#if svc.conn.status === 'success'}
                <span class="status-badge success">
                  <span class="material-icons-outlined">check_circle</span>
                  Connected
                </span>
              {/if}
            </div>

            {#if configLoading}
              <div class="loading-text">Loading config...</div>
            {:else}
              <label class="field-label">
                URL
                <input
                  type="text"
                  data-field="{svc.key}_url"
                  bind:value={svc.conn.url}
                  placeholder={svc.placeholder}
                />
              </label>

              <label class="field-label">
                API Key
                <input
                  type="password"
                  data-field="{svc.key}_api_key"
                  bind:value={svc.conn.apiKey}
                  placeholder="Paste your API key"
                />
              </label>

              <div class="connect-card-footer">
                <button
                  class="btn"
                  onclick={() => testConnection(svc.key)}
                  disabled={svc.conn.status === 'testing' || !svc.conn.url || !svc.conn.apiKey}
                >
                  {#if svc.conn.status === 'testing'}
                    <span class="spinner"></span> Testing...
                  {:else}
                    Test Connection
                  {/if}
                </button>

                {#if svc.conn.status === 'success' && svc.conn.version}
                  <span class="version-tag">v{svc.conn.version}</span>
                {/if}

                {#if svc.conn.status === 'error' && svc.conn.error}
                  <span class="error-inline">
                    <span class="material-icons-outlined">error</span>
                    {svc.conn.error}
                  </span>
                {/if}
              </div>
            {/if}
          </div>
        {/each}
      </div>

      {#if canPreview}
        <div class="step-actions">
          <button class="btn primary" onclick={goToPreview}>
            Continue to Preview
            <span class="material-icons-outlined">arrow_forward</span>
          </button>
        </div>
      {/if}
    {/if}

    <!-- Step 2: Preview -->
    {#if step === 2}
      <div class="preview-section">
        <!-- Tabs -->
        {#if connectedServices.length > 1}
          <div class="tab-bar">
            {#each connectedServices as svc (svc)}
              <button
                class="tab"
                class:active={activeTab === svc}
                onclick={() => {
                  activeTab = svc;
                }}
              >
                <span class="material-icons-outlined">
                  {svc === 'sonarr' ? 'live_tv' : 'movie'}
                </span>
                {svc === 'sonarr' ? 'Sonarr' : 'Radarr'}
                {#if (svc === 'sonarr' && sonarrSelected.size > 0) || (svc === 'radarr' && radarrSelected.size > 0)}
                  <span class="tab-count">
                    {svc === 'sonarr' ? sonarrSelected.size : radarrSelected.size}
                  </span>
                {/if}
              </button>
            {/each}
          </div>
        {/if}

        <!-- Fetch button -->
        {#if (activeTab === 'sonarr' && !sonarrFetched) || (activeTab === 'radarr' && !radarrFetched)}
          <div class="fetch-prompt">
            <p>
              Fetch library data from {activeTab === 'sonarr' ? 'Sonarr' : 'Radarr'} to preview what can
              be imported.
            </p>
            <button class="btn primary" onclick={fetchPreview} disabled={previewLoading}>
              {#if previewLoading}
                <span class="spinner"></span> Fetching...
              {:else}
                <span class="material-icons-outlined">cloud_download</span>
                Fetch Library
              {/if}
            </button>
            {#if previewError}
              <p class="error-text">{previewError}</p>
            {/if}
          </div>
        {:else}
          <!-- Summary bar -->
          {@const stats = summaryStats}
          <div class="summary-bar">
            <span class="summary-stat">{stats.total} total</span>
            <span class="summary-divider"></span>
            <span class="summary-stat new">{stats.newCount} new</span>
            <span class="summary-stat exists">{stats.existsCount} already exist</span>
            {#if stats.noTmdb > 0}
              <span class="summary-stat warn">{stats.noTmdb} no TMDB ID</span>
            {/if}
            <span class="summary-divider"></span>
            <span class="summary-stat accent">{stats.selected} selected</span>
            {#if wantMissing && stats.willWant > 0}
              <span class="summary-stat want">{stats.willWant} to search</span>
            {/if}
          </div>

          <!-- Filter chips -->
          <div class="filter-row">
            <span class="filter-label">Auto-select:</span>
            {#if activeTab === 'sonarr'}
              <button
                class="chip"
                class:active={sonarrFilterAiring}
                onclick={() => {
                  sonarrFilterAiring = !sonarrFilterAiring;
                  applyFilterPresets('sonarr');
                }}>Still Airing</button
              >
              <button
                class="chip"
                class:active={sonarrFilterEnded}
                onclick={() => {
                  sonarrFilterEnded = !sonarrFilterEnded;
                  applyFilterPresets('sonarr');
                }}>Ended</button
              >
              <button
                class="chip"
                class:active={sonarrFilterMonitored}
                onclick={() => {
                  sonarrFilterMonitored = !sonarrFilterMonitored;
                  applyFilterPresets('sonarr');
                }}>Monitored</button
              >
              <button
                class="chip"
                class:active={sonarrFilterMissing}
                onclick={() => {
                  sonarrFilterMissing = !sonarrFilterMissing;
                  applyFilterPresets('sonarr');
                }}>Has Missing Episodes</button
              >
            {:else}
              <button
                class="chip"
                class:active={radarrFilterMissing}
                onclick={() => {
                  radarrFilterMissing = !radarrFilterMissing;
                  applyFilterPresets('radarr');
                }}>Missing Files</button
              >
              <button
                class="chip"
                class:active={radarrFilterHasFiles}
                onclick={() => {
                  radarrFilterHasFiles = !radarrFilterHasFiles;
                  applyFilterPresets('radarr');
                }}>Has Files</button
              >
              <button
                class="chip"
                class:active={radarrFilterMonitored}
                onclick={() => {
                  radarrFilterMonitored = !radarrFilterMonitored;
                  applyFilterPresets('radarr');
                }}>Monitored</button
              >
            {/if}
          </div>

          <!-- Root folder mapping -->
          {#if currentRootFolders.length > 0}
            <div class="root-mapping-card">
              <h4>
                <span class="material-icons-outlined">folder</span>
                Root Folder Mapping
              </h4>
              <div class="root-mapping-grid">
                {#each currentRootFolders as folder (folder.path)}
                  <div class="root-mapping-row">
                    <span class="root-source">{folder.path}</span>
                    <span class="material-icons-outlined arrow-icon">arrow_forward</span>
                    <SearchSelect
                      id="root-mapping-{folder.path}"
                      items={mediaRootsForType(activeTab === 'sonarr' ? 'tv' : 'movie').map(
                        (root) => ({ value: root.id, label: `${root.name} (${root.path})` })
                      )}
                      value={rootMapping[folder.path] ?? ''}
                      onchange={(v) => {
                        rootMapping[folder.path] = v;
                        rootMapping = { ...rootMapping };
                      }}
                      placeholder="Select media root..."
                    />
                  </div>
                {/each}
              </div>
            </div>
          {/if}

          <!-- Want missing toggle -->
          <div class="want-toggle-card">
            <label class="want-toggle">
              <input type="checkbox" bind:checked={wantMissing} />
              <div class="want-toggle-text">
                <span class="want-toggle-label">
                  <span class="material-icons-outlined">manage_search</span>
                  Search for missing content after import
                </span>
                <span class="want-toggle-desc">
                  {#if wantMissing && summaryStats.willWant > 0}
                    {summaryStats.willWant}
                    {activeTab === 'sonarr' ? 'show' : 'movie'}{summaryStats.willWant !== 1
                      ? 's'
                      : ''} will be searched
                  {:else if wantMissing}
                    No selected items need searching
                  {:else}
                    Disabled — missing content will not be searched
                  {/if}
                </span>
              </div>
            </label>
          </div>

          <!-- Search + select controls -->
          <div class="table-controls">
            <div class="search-box">
              <span class="material-icons-outlined">search</span>
              <input type="text" placeholder="Filter by title..." bind:value={searchFilter} />
            </div>
            <div class="select-actions">
              <button class="btn small" onclick={selectAll}>Select All</button>
              <button class="btn small alt" onclick={deselectAll}>Deselect All</button>
            </div>
          </div>

          <!-- Results table -->
          <div class="table-wrapper">
            <table>
              <thead>
                <tr>
                  <th class="col-check"></th>
                  <th class="col-title">Title</th>
                  <th class="col-year">Year</th>
                  <th class="col-status">Status</th>
                  <th class="col-monitored">Monitored</th>
                  <th class="col-import">Import</th>
                </tr>
              </thead>
              <tbody>
                {#each filteredItems as item (item.source_id)}
                  <tr
                    class:disabled-row={!item.tmdb_id}
                    onclick={() => {
                      if (item.tmdb_id) toggleItem(item.tmdb_id);
                    }}
                  >
                    <td class="col-check">
                      <input
                        type="checkbox"
                        checked={item.tmdb_id ? currentSelected.has(item.tmdb_id) : false}
                        disabled={!item.tmdb_id}
                        onclick={(e: MouseEvent) => e.stopPropagation()}
                        onchange={() => {
                          if (item.tmdb_id) toggleItem(item.tmdb_id);
                        }}
                      />
                    </td>
                    <td class="col-title">
                      {item.title}
                      {#if item.episode_stats}
                        <span class="episode-info">
                          {item.episode_stats.with_file}/{item.episode_stats.total} episodes
                        </span>
                      {/if}
                    </td>
                    <td class="col-year">{item.year ?? '—'}</td>
                    <td class="col-status">
                      {#if item.has_files || (activeTab === 'sonarr' && !hasMissingEpisodes(item))}
                        <span class="badge file">
                          <span class="material-icons-outlined">folder</span>
                          {statusLabel(item)}
                        </span>
                      {:else}
                        <span class="badge missing">
                          <span class="material-icons-outlined">cloud_off</span>
                          {statusLabel(item)}
                        </span>
                      {/if}
                    </td>
                    <td class="col-monitored">
                      {#if item.monitored}
                        <span class="badge monitored">
                          <span class="material-icons-outlined">visibility</span>
                        </span>
                      {:else}
                        <span class="badge unmonitored">
                          <span class="material-icons-outlined">visibility_off</span>
                        </span>
                      {/if}
                    </td>
                    <td class="col-import">
                      {#if !item.tmdb_id}
                        <span class="badge warn">No TMDB</span>
                      {:else if item.import_status === 'new'}
                        <span class="badge new">New</span>
                      {:else}
                        <span class="badge exists">Exists</span>
                      {/if}
                    </td>
                  </tr>
                {/each}
                {#if filteredItems.length === 0}
                  <tr>
                    <td colspan="6" class="empty-row">
                      {searchFilter ? 'No items match your search.' : 'No items found.'}
                    </td>
                  </tr>
                {/if}
              </tbody>
            </table>
          </div>

          <!-- Import action -->
          <div class="step-actions">
            <button
              class="btn alt"
              onclick={() => {
                step = 1;
              }}
            >
              <span class="material-icons-outlined">arrow_back</span>
              Back
            </button>
            <button class="btn primary" onclick={startImport} disabled={!canImport || importing}>
              {#if importing}
                <span class="spinner"></span> Importing...
              {:else}
                Import {totalSelected} Item{totalSelected !== 1 ? 's' : ''}
              {/if}
            </button>
            {#if importError}
              <span class="error-inline">
                <span class="material-icons-outlined">error</span>
                {importError}
              </span>
            {/if}
            {#if !allRootsMapped && totalSelected > 0}
              <span class="warn-inline">
                <span class="material-icons-outlined">warning</span>
                Map all root folders before importing
              </span>
            {/if}
          </div>
        {/if}
      </div>
    {/if}

    <!-- Step 3: Results -->
    {#if step === 3 && importResult}
      <div class="results-section">
        <div class="results-hero">
          <span class="material-icons-outlined hero-icon">check_circle</span>
          <h3>Import Complete</h3>
          <p class="results-count">
            {importResult.imported} item{importResult.imported !== 1 ? 's' : ''} imported
          </p>
        </div>

        <div class="info-banner">
          <span class="material-icons-outlined">auto_awesome</span>
          <div>
            <strong>Enrichment jobs queued</strong>
            <p>Metadata will be fetched and matched automatically in the background.</p>
          </div>
        </div>

        {#if importResult.wants_enqueued > 0}
          <div class="info-banner want">
            <span class="material-icons-outlined">manage_search</span>
            <div>
              <strong
                >{importResult.wants_enqueued} search{importResult.wants_enqueued !== 1 ? 'es' : ''} enqueued</strong
              >
              <p>Missing content will be searched for automatically via Kobload.</p>
            </div>
          </div>
        {/if}

        {#if importResult.scans_triggered.length > 0}
          <div class="info-banner">
            <span class="material-icons-outlined">folder_open</span>
            <div>
              <strong>Media root scans triggered</strong>
              <p>Mapped media roots are being scanned for existing files.</p>
            </div>
          </div>
        {/if}

        {#if importResult.skipped_reasons.length > 0}
          <div class="skipped-card">
            <h4>Skipped ({importResult.skipped_reasons.length})</h4>
            <div class="skipped-list">
              {#each importResult.skipped_reasons as skip (skip.title)}
                <div class="skipped-item">
                  <span class="skipped-title">{skip.title}</span>
                  <span class="skipped-reason">{skip.reason}</span>
                </div>
              {/each}
            </div>
          </div>
        {/if}

        <div class="step-actions">
          <a href="/tv" class="btn primary">
            <span class="material-icons-outlined">live_tv</span>
            TV Shows
          </a>
          <a href="/movies" class="btn primary">
            <span class="material-icons-outlined">movie</span>
            Movies
          </a>
          <button
            class="btn alt"
            onclick={() => {
              step = 1;
              importResult = null;
            }}
          >
            Start Over
          </button>
        </div>
      </div>
    {/if}
  </div>
{/if}

<style>
  .permission-denied {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 4rem 2rem;
    color: var(--muted);
    text-align: center;
  }

  .permission-denied h2 {
    font-size: 1.25rem;
    margin-bottom: 0.5rem;
    color: var(--ink);
  }

  .migration-page {
    display: grid;
    gap: 20px;
    max-width: 960px;
  }

  .page-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .subtitle {
    margin: 4px 0 0;
    font-size: 13px;
    color: var(--muted);
  }

  /* ---- Steps bar ---- */

  .steps-bar {
    display: flex;
    align-items: center;
    gap: 0;
    padding: 12px 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
  }

  .step-dot {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    border: none;
    background: none;
    color: var(--muted);
    font-size: 13px;
    font-weight: 500;
    cursor: default;
    border-radius: 8px;
    transition: all 0.15s;
  }

  .step-dot:not(:disabled) {
    cursor: pointer;
  }

  .step-dot:not(:disabled):hover {
    background: var(--ring);
  }

  .step-dot .material-icons-outlined {
    font-size: 20px;
  }

  .step-dot.active {
    color: var(--accent);
  }

  .step-dot.done {
    color: #4ade80;
  }

  .step-line {
    flex: 1;
    height: 2px;
    background: var(--ring);
    margin: 0 4px;
    border-radius: 1px;
    transition: background 0.2s;
  }

  .step-line.active {
    background: var(--accent);
  }

  .step-label {
    font-size: 13px;
  }

  /* ---- Connect cards ---- */

  .connect-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 16px;
  }

  .connect-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 20px;
    display: grid;
    gap: 14px;
  }

  .connect-card-header {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .connect-card-header h3 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
    flex: 1;
  }

  .service-icon {
    font-size: 24px;
    color: var(--accent);
  }

  .field-label {
    display: grid;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .field-label input {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .field-label input:focus {
    outline: none;
    border-color: var(--accent);
  }

  .connect-card-footer {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }

  .loading-text {
    color: var(--muted);
    font-size: 13px;
    padding: 8px 0;
  }

  .status-badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    font-weight: 600;
    padding: 3px 8px;
    border-radius: 8px;
  }

  .status-badge .material-icons-outlined {
    font-size: 16px;
  }

  .status-badge.success {
    color: #4ade80;
    background: rgba(74, 222, 128, 0.1);
  }

  .version-tag {
    font-size: 12px;
    color: var(--muted);
    background: var(--ring);
    padding: 2px 8px;
    border-radius: 6px;
  }

  .error-inline {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    color: var(--danger);
  }

  .error-inline .material-icons-outlined {
    font-size: 16px;
  }

  .warn-inline {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    color: #f59e0b;
  }

  .warn-inline .material-icons-outlined {
    font-size: 16px;
  }

  /* ---- Buttons ---- */

  .btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    border-radius: 12px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    font-weight: 600;
    font-size: 13px;
    cursor: pointer;
    transition: all 0.15s;
    text-decoration: none;
  }

  .btn:hover:not(:disabled) {
    background: var(--ring);
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: white;
  }

  .btn.primary:hover:not(:disabled) {
    filter: brightness(1.1);
  }

  .btn.alt {
    background: transparent;
    border-color: var(--ring);
    color: var(--muted);
  }

  .btn.small {
    padding: 5px 10px;
    font-size: 12px;
    border-radius: 8px;
  }

  .btn .material-icons-outlined {
    font-size: 18px;
  }

  /* ---- Spinner ---- */

  .spinner {
    display: inline-block;
    width: 14px;
    height: 14px;
    border: 2px solid var(--ring);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.6s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  /* ---- Preview section ---- */

  .preview-section {
    display: grid;
    gap: 16px;
  }

  .tab-bar {
    display: flex;
    gap: 4px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    padding: 4px;
  }

  .tab {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: 1;
    justify-content: center;
    padding: 8px 16px;
    border: none;
    background: none;
    color: var(--muted);
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    border-radius: 8px;
    transition: all 0.15s;
  }

  .tab:hover {
    background: var(--ring);
  }

  .tab.active {
    background: var(--accent);
    color: white;
  }

  .tab .material-icons-outlined {
    font-size: 18px;
  }

  .tab-count {
    background: rgba(255, 255, 255, 0.2);
    padding: 1px 6px;
    border-radius: 6px;
    font-size: 11px;
  }

  .fetch-prompt {
    text-align: center;
    padding: 40px 20px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    display: grid;
    gap: 16px;
    justify-items: center;
  }

  .fetch-prompt p {
    margin: 0;
    color: var(--muted);
    font-size: 13px;
  }

  .error-text {
    color: var(--danger);
    font-size: 13px;
  }

  /* ---- Summary bar ---- */

  .summary-bar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    font-size: 13px;
    flex-wrap: wrap;
  }

  .summary-stat {
    color: var(--muted);
  }

  .summary-stat.new {
    color: #4ade80;
  }

  .summary-stat.exists {
    color: var(--muted);
  }

  .summary-stat.warn {
    color: #f59e0b;
  }

  .summary-stat.accent {
    color: var(--accent);
    font-weight: 600;
  }

  .summary-stat.want {
    color: #a78bfa;
  }

  .summary-divider {
    width: 1px;
    height: 16px;
    background: var(--ring);
  }

  /* ---- Filter chips ---- */

  .filter-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .filter-label {
    font-size: 12px;
    color: var(--muted);
    font-weight: 500;
  }

  .chip {
    padding: 5px 12px;
    border-radius: 20px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    font-size: 12px;
    cursor: pointer;
    transition: all 0.15s;
  }

  .chip:hover {
    border-color: var(--accent);
    color: var(--ink);
  }

  .chip.active {
    background: var(--accent);
    border-color: var(--accent);
    color: white;
  }

  /* ---- Root mapping ---- */

  .root-mapping-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 16px;
  }

  .root-mapping-card h4 {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0 0 12px;
    font-size: 14px;
    font-weight: 600;
  }

  .root-mapping-card h4 .material-icons-outlined {
    font-size: 20px;
    color: var(--accent);
  }

  .root-mapping-grid {
    display: grid;
    gap: 8px;
  }

  .root-mapping-row {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .root-source {
    flex: 1;
    font-size: 13px;
    color: var(--muted);
    font-family: monospace;
    padding: 6px 10px;
    background: rgba(255, 255, 255, 0.03);
    border-radius: 8px;
    border: 1px solid var(--ring);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .arrow-icon {
    font-size: 18px;
    color: var(--muted);
    flex-shrink: 0;
  }

  .root-mapping-row :global(.search-select) {
    flex: 1;
  }

  /* ---- Want toggle ---- */

  .want-toggle-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    padding: 14px 16px;
  }

  .want-toggle {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    cursor: pointer;
  }

  .want-toggle input[type='checkbox'] {
    margin-top: 2px;
    flex-shrink: 0;
  }

  .want-toggle-text {
    display: grid;
    gap: 2px;
  }

  .want-toggle-label {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    font-weight: 600;
  }

  .want-toggle-label .material-icons-outlined {
    font-size: 18px;
    color: var(--accent);
  }

  .want-toggle-desc {
    font-size: 12px;
    color: var(--muted);
  }

  /* ---- Table controls ---- */

  .table-controls {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }

  .search-box {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: 1;
    min-width: 200px;
    padding: 6px 12px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
  }

  .search-box .material-icons-outlined {
    font-size: 18px;
    color: var(--muted);
  }

  .search-box input {
    flex: 1;
    border: none;
    background: none;
    color: var(--ink);
    font-size: 13px;
    outline: none;
  }

  .select-actions {
    display: flex;
    gap: 6px;
  }

  /* ---- Results table ---- */

  .table-wrapper {
    overflow-x: auto;
    border-radius: 16px;
    border: 1px solid var(--ring);
    background: var(--card);
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }

  thead {
    border-bottom: 1px solid var(--ring);
  }

  th {
    text-align: left;
    padding: 10px 12px;
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  td {
    padding: 10px 12px;
    border-top: 1px solid var(--ring);
  }

  tbody tr {
    cursor: pointer;
    transition: background 0.1s;
  }

  tbody tr:first-child td {
    border-top: none;
  }

  tbody tr:hover {
    background: rgba(255, 255, 255, 0.03);
  }

  tr.disabled-row {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .col-check {
    width: 40px;
    text-align: center;
  }

  .col-title {
    min-width: 200px;
  }

  .episode-info {
    display: block;
    font-size: 11px;
    color: var(--muted);
    margin-top: 2px;
  }

  .col-year {
    width: 60px;
    color: var(--muted);
  }

  .col-status {
    width: 140px;
  }

  .col-monitored {
    width: 80px;
    text-align: center;
  }

  .col-import {
    width: 80px;
  }

  .empty-row {
    text-align: center;
    padding: 24px;
    color: var(--muted);
  }

  input[type='checkbox'] {
    accent-color: var(--accent);
    width: 16px;
    height: 16px;
    cursor: pointer;
  }

  /* ---- Badges ---- */

  .badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 8px;
    border-radius: 6px;
    font-size: 11px;
    font-weight: 600;
    white-space: nowrap;
  }

  .badge .material-icons-outlined {
    font-size: 14px;
  }

  .badge.file {
    color: #4ade80;
    background: rgba(74, 222, 128, 0.1);
  }

  .badge.missing {
    color: #f59e0b;
    background: rgba(245, 158, 11, 0.1);
  }

  .badge.monitored {
    color: var(--accent);
    background: rgba(99, 102, 241, 0.1);
  }

  .badge.unmonitored {
    color: var(--muted);
    background: rgba(255, 255, 255, 0.05);
  }

  .badge.new {
    color: #4ade80;
    background: rgba(74, 222, 128, 0.1);
  }

  .badge.exists {
    color: var(--muted);
    background: rgba(255, 255, 255, 0.05);
  }

  .badge.warn {
    color: #f59e0b;
    background: rgba(245, 158, 11, 0.1);
  }

  /* ---- Step actions ---- */

  .step-actions {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }

  /* ---- Results section ---- */

  .results-section {
    display: grid;
    gap: 16px;
  }

  .results-hero {
    text-align: center;
    padding: 32px 20px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
  }

  .hero-icon {
    font-size: 48px;
    color: #4ade80;
  }

  .results-hero h3 {
    margin: 8px 0 4px;
    font-size: 18px;
  }

  .results-count {
    margin: 0;
    color: var(--muted);
    font-size: 14px;
  }

  .info-banner {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 14px 16px;
    background: rgba(99, 102, 241, 0.08);
    border: 1px solid rgba(99, 102, 241, 0.2);
    border-radius: 12px;
  }

  .info-banner .material-icons-outlined {
    font-size: 22px;
    color: var(--accent);
    flex-shrink: 0;
    margin-top: 1px;
  }

  .info-banner strong {
    display: block;
    font-size: 13px;
    margin-bottom: 2px;
  }

  .info-banner p {
    margin: 0;
    font-size: 12px;
    color: var(--muted);
  }

  .info-banner.want {
    background: rgba(167, 139, 250, 0.08);
    border-color: rgba(167, 139, 250, 0.2);
  }

  .info-banner.want .material-icons-outlined {
    color: #a78bfa;
  }

  .skipped-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 16px;
  }

  .skipped-card h4 {
    margin: 0 0 12px;
    font-size: 14px;
    font-weight: 600;
  }

  .skipped-list {
    display: grid;
    gap: 6px;
    max-height: 200px;
    overflow-y: auto;
  }

  .skipped-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 10px;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.03);
    font-size: 13px;
  }

  .skipped-title {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .skipped-reason {
    color: var(--muted);
    font-size: 12px;
    flex-shrink: 0;
  }

  /* ---- Responsive ---- */

  @media (max-width: 768px) {
    .connect-grid {
      grid-template-columns: 1fr;
    }

    .steps-bar {
      padding: 8px 10px;
    }

    .step-label {
      display: none;
    }

    .root-mapping-row {
      flex-direction: column;
      align-items: stretch;
    }

    .arrow-icon {
      display: none;
    }

    .table-controls {
      flex-direction: column;
      align-items: stretch;
    }

    .search-box {
      min-width: unset;
    }

    .col-status,
    .col-monitored {
      display: none;
    }
  }
</style>
