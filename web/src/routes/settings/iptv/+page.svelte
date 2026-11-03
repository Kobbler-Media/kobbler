<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { ExpandableSidebar } from '@koblibs/ui';
  import ErrorBanner from '#lib/components/shared/ErrorBanner.svelte';
  import ProviderForm from '#lib/components/iptv/ProviderForm.svelte';
  import UnmatchedReview from '#lib/components/iptv/UnmatchedReview.svelte';
  import { onReconnect, offReconnect } from '#lib/services/eventstream.svelte.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import * as api from '#lib/api/client.js';
  import type { SystemConfig, SystemConfigUpdate } from '#lib/api/client.js';
  import type {
    IptvProvider,
    IptvChannel,
    IptvUnmatchedSource,
    IptvProgrammeWatch,
    IptvWatchMatch
  } from '#lib/types.js';

  type Tab = 'providers' | 'channels' | 'unmatched' | 'watches' | 'recording';

  let activeTab = $state<Tab>('providers');
  let loading = $state(true);
  let error = $state('');

  // Recording tab
  type IptvSettingsData = {
    recording_path: string | null;
    epg_retention_days: number | null;
    default_format: string | null;
  };
  let settings = $state<IptvSettingsData>({
    recording_path: null,
    epg_retention_days: 7,
    default_format: null
  });
  let settingsSaving = $state(false);

  // System config (for Defaults card)
  let systemConfig = $state<SystemConfig | null>(null);
  let systemConfigLoading = $state(true);
  let systemConfigSaving = $state(false);
  let iptvLanguageFilterText = $state('');

  // Providers tab
  let providers = $state<IptvProvider[]>([]);
  let showAddProvider = $state(false);
  let addingSaving = $state(false);
  let syncingIds = $state(new Set<string>());
  let testResult = $state<{ ok: boolean; message: string } | null>(null);

  // Provider sidebar
  let sidebarOpen = $state(false);
  let selectedProvider = $state<IptvProvider | null>(null);
  let confirmDeleteProviderId = $state<string | null>(null);
  let editingProvider = $state(false);
  let editSaving = $state(false);

  // Channels tab
  let channels = $state<IptvChannel[]>([]);
  let channelSearch = $state('');
  let channelTotal = $state(0);
  let channelOffset = $state(0);
  let togglingIds = $state(new Set<string>());
  let selectedChannelIds = $state(new Set<string>());
  let showMergeModal = $state(false);
  let mergeCanonicalName = $state('');
  let confirmDisableChannelId = $state<string | null>(null);

  // Unmatched tab
  let unmatched = $state<IptvUnmatchedSource[]>([]);
  let unmatchedTotal = $state(0);

  // Watches tab
  let watches = $state<IptvProgrammeWatch[]>([]);
  let watchMatches = $state<IptvWatchMatch[]>([]);
  let showNewWatch = $state(false);
  let newWatchPattern = $state('');
  let newWatchMode = $state('contains');
  let watchSaving = $state(false);
  let confirmDeleteWatchId = $state<string | null>(null);

  // Reconnect handler
  let reconnectHandler: (() => void) | null = null;

  // Watch match count for tab badge
  const watchMatchCount = $derived(watchMatches.length);

  async function loadProviders() {
    loading = true;
    error = '';
    try {
      providers = await api.getIptvProviders();
    } catch (e) {
      console.error('Failed to load providers:', e);
      error = 'Failed to load providers. Please check your connection and try again.';
    } finally {
      loading = false;
    }
  }

  async function loadChannels() {
    loading = true;
    error = '';
    try {
      const params: Record<string, string | number | undefined> = {
        limit: 50,
        offset: channelOffset
      };
      if (channelSearch.trim()) params.search = channelSearch.trim();
      const result = await api.getIptvChannels(params);
      channels = result.items;
      channelTotal = result.total;
    } catch (e) {
      console.error('Failed to load channels:', e);
      error = 'Failed to load channels.';
    } finally {
      loading = false;
    }
  }

  async function loadUnmatched() {
    loading = true;
    error = '';
    try {
      const result = await api.getIptvUnmatched(50, 0);
      unmatched = result.items;
      unmatchedTotal = result.total;
    } catch (e) {
      console.error('Failed to load unmatched:', e);
      error = 'Failed to load unmatched sources.';
    } finally {
      loading = false;
    }
  }

  async function loadWatches() {
    loading = true;
    error = '';
    try {
      const [w, m] = await Promise.all([api.getIptvWatches(), api.getIptvWatchMatches()]);
      watches = w;
      watchMatches = m;
    } catch (e) {
      console.error('Failed to load watches:', e);
      error = 'Failed to load programme watches.';
    } finally {
      loading = false;
    }
  }

  // Also load watch match count on mount for badge
  async function loadWatchMatchCount() {
    try {
      watchMatches = await api.getIptvWatchMatches();
    } catch {
      // Silently fail for badge count
    }
  }

  async function handleAddProvider(data: {
    name: string;
    url: string;
    username: string;
    password: string;
    preferred_format: string;
    sync_interval_secs: number;
  }) {
    addingSaving = true;
    try {
      await api.createIptvProvider(data);
      showMessage('Provider added');
      showAddProvider = false;
      testResult = null;
      loadProviders();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to add provider');
    } finally {
      addingSaving = false;
    }
  }

  async function handleConfirmDeleteProvider() {
    if (!confirmDeleteProviderId) return;
    const id = confirmDeleteProviderId;
    confirmDeleteProviderId = null;
    try {
      await api.deleteIptvProvider(id);
      providers = providers.filter((p) => p.id !== id);
      if (selectedProvider?.id === id) {
        sidebarOpen = false;
        selectedProvider = null;
      }
      showMessage('Provider deleted');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete provider');
    }
  }

  async function handleSync(id: string) {
    syncingIds = new Set([...syncingIds, id]);
    try {
      await api.syncIptvProvider(id);
      showMessage('Sync started');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to start sync');
    } finally {
      const next = new Set(syncingIds);
      next.delete(id);
      syncingIds = next;
    }
  }

  function handleSelectProvider(provider: IptvProvider) {
    selectedProvider = provider;
    sidebarOpen = true;
  }

  function handleCloseSidebar() {
    sidebarOpen = false;
    selectedProvider = null;
    editingProvider = false;
  }

  async function handleUpdateProvider(data: {
    name: string;
    url: string;
    username: string;
    password: string;
    preferred_format: string;
    sync_interval_secs: number;
    enabled?: boolean;
  }) {
    if (!selectedProvider) return;
    editSaving = true;
    try {
      const input: Record<string, string | number | boolean> = {
        name: data.name,
        url: data.url,
        username: data.username,
        preferred_format: data.preferred_format,
        sync_interval_secs: data.sync_interval_secs
      };
      if (data.password) input.password = data.password;
      if (data.enabled !== undefined) input.enabled = data.enabled;
      await api.updateIptvProvider(selectedProvider.id, input);
      showMessage('Provider updated');
      editingProvider = false;
      loadProviders();
      // Update the sidebar view with new values
      selectedProvider = {
        ...selectedProvider,
        name: data.name,
        url: data.url,
        username: data.username,
        preferred_format: data.preferred_format,
        sync_interval_secs: data.sync_interval_secs,
        enabled: data.enabled ?? selectedProvider.enabled
      };
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update provider');
    } finally {
      editSaving = false;
    }
  }

  async function handleToggleChannel(id: string, enabled: boolean) {
    if (enabled) {
      // Disabling requires confirmation
      confirmDisableChannelId = id;
      return;
    }
    doToggleChannel(id, enabled);
  }

  async function handleConfirmDisableChannel() {
    if (!confirmDisableChannelId) return;
    const id = confirmDisableChannelId;
    confirmDisableChannelId = null;
    doToggleChannel(id, true);
  }

  async function doToggleChannel(id: string, currentEnabled: boolean) {
    togglingIds = new Set([...togglingIds, id]);
    try {
      await api.toggleIptvChannel(id, !currentEnabled);
      channels = channels.map((c) => (c.id === id ? { ...c, enabled: !currentEnabled } : c));
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to toggle channel');
    } finally {
      const next = new Set(togglingIds);
      next.delete(id);
      togglingIds = next;
    }
  }

  function toggleChannelSelection(id: string) {
    const next = new Set(selectedChannelIds);
    if (next.has(id)) {
      next.delete(id);
    } else {
      next.add(id);
    }
    selectedChannelIds = next;
  }

  function startMerge() {
    if (selectedChannelIds.size < 2) return;
    const selectedChannels = channels.filter((c) => selectedChannelIds.has(c.id));
    mergeCanonicalName = selectedChannels[0]?.name ?? '';
    showMergeModal = true;
  }

  function cancelMerge() {
    showMergeModal = false;
    mergeCanonicalName = '';
  }

  async function handleMerge() {
    if (!mergeCanonicalName) return;
    const selected = channels.filter((c) => selectedChannelIds.has(c.id));
    // Find the target channel (the one whose name matches the chosen canonical name)
    const target = selected.find((c) => c.name === mergeCanonicalName);
    if (!target) return;
    const sources = selected.filter((c) => c.id !== target.id);
    try {
      for (const source of sources) {
        await api.mergeIptvChannels(source.id, target.id);
      }
      showMessage(`Merged ${sources.length} channel(s) into "${mergeCanonicalName}"`);
      showMergeModal = false;
      mergeCanonicalName = '';
      selectedChannelIds = new Set();
      await loadChannels();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Merge failed');
    }
  }

  async function handleCreateWatch() {
    if (!newWatchPattern.trim()) return;
    watchSaving = true;
    try {
      await api.createIptvWatch({
        title_pattern: newWatchPattern.trim(),
        match_mode: newWatchMode
      });
      showMessage('Watch created');
      showNewWatch = false;
      newWatchPattern = '';
      newWatchMode = 'contains';
      loadWatches();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to create watch');
    } finally {
      watchSaving = false;
    }
  }

  async function handleConfirmDeleteWatch() {
    if (!confirmDeleteWatchId) return;
    const id = confirmDeleteWatchId;
    confirmDeleteWatchId = null;
    try {
      await api.deleteIptvWatch(id);
      watches = watches.filter((w) => w.id !== id);
      showMessage('Watch deleted');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete watch');
    }
  }

  async function loadSettings() {
    try {
      const s = await api.getIptvSettings();
      settings = {
        recording_path: (s.recording_path as string) ?? null,
        epg_retention_days: (s.epg_retention_days as number) ?? 7,
        default_format: (s.default_format as string) ?? null
      };
    } catch {
      showMessage('Failed to load settings');
    }
  }

  async function saveSettings() {
    settingsSaving = true;
    try {
      await api.updateIptvSettings({
        recording_path: settings.recording_path,
        epg_retention_days: settings.epg_retention_days,
        default_format: settings.default_format
      });
      showMessage('Settings saved');
    } catch {
      showMessage('Failed to save settings');
    } finally {
      settingsSaving = false;
    }
  }

  async function loadSystemConfig() {
    systemConfigLoading = true;
    try {
      systemConfig = await api.getSystemConfig();
      iptvLanguageFilterText = systemConfig.iptv_language_filter.join(', ');
    } catch {
      showMessage('Failed to load system config');
    } finally {
      systemConfigLoading = false;
    }
  }

  async function saveSystemConfig() {
    if (!systemConfig) return;
    systemConfigSaving = true;
    try {
      const languages = iptvLanguageFilterText
        .split(',')
        .map((s) => s.trim())
        .filter((s) => s.length > 0);
      const updates: SystemConfigUpdate = {
        iptv_language_filter: languages,
        iptv_auto_create_channels: systemConfig.iptv_auto_create_channels,
        iptv_health_check_enabled: systemConfig.iptv_health_check_enabled
      };
      systemConfig = await api.updateSystemConfig(updates);
      iptvLanguageFilterText = systemConfig.iptv_language_filter.join(', ');
      showMessage('Defaults saved');
    } catch {
      showMessage('Failed to save defaults');
    } finally {
      systemConfigSaving = false;
    }
  }

  function switchTab(tab: Tab) {
    activeTab = tab;
    error = '';
    if (tab === 'providers') loadProviders();
    else if (tab === 'channels') loadChannels();
    else if (tab === 'watches') loadWatches();
    else if (tab === 'recording') loadSettings();
    else loadUnmatched();
  }

  function formatDate(dateStr: string | null): string {
    if (!dateStr) return 'Never';
    return new Date(dateStr).toLocaleString(undefined, {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit'
    });
  }

  let searchTimeout: ReturnType<typeof setTimeout> | undefined;
  function handleChannelSearch() {
    clearTimeout(searchTimeout);
    searchTimeout = setTimeout(() => {
      channelOffset = 0;
      selectedChannelIds = new Set();
      loadChannels();
    }, 300);
  }

  onMount(() => {
    loadProviders();
    loadWatchMatchCount();
    loadSystemConfig();

    reconnectHandler = onReconnect(() => {
      switchTab(activeTab);
    });

    return () => clearTimeout(searchTimeout);
  });

  onDestroy(() => {
    if (reconnectHandler) offReconnect(reconnectHandler);
  });
</script>

<svelte:head>
  <title>IPTV Settings — Kobbler</title>
</svelte:head>

{#if error}
  <div class="error-wrapper">
    <ErrorBanner
      message={error}
      onRetry={() => switchTab(activeTab)}
      onDismiss={() => (error = '')}
    />
  </div>
{/if}

<div class="settings-page">
  <h1>IPTV</h1>

  <!-- Defaults card -->
  <div class="defaults-card">
    <h3>Defaults</h3>
    {#if systemConfigLoading}
      <p class="muted">Loading defaults...</p>
    {:else if systemConfig}
      <div class="defaults-fields">
        <label>
          Language Filter
          <input
            type="text"
            data-field="iptv_language_filter"
            value={iptvLanguageFilterText}
            oninput={(e) => {
              iptvLanguageFilterText = (e.target as HTMLInputElement).value;
            }}
            placeholder="en, fr, de"
          />
          <span class="field-hint">Comma-separated language codes (e.g. en,fr,de)</span>
        </label>
        <label class="toggle-label">
          <input
            type="checkbox"
            data-field="iptv_auto_create_channels"
            checked={systemConfig.iptv_auto_create_channels}
            onchange={(e) => {
              if (systemConfig)
                systemConfig.iptv_auto_create_channels = (e.target as HTMLInputElement).checked;
            }}
          />
          Auto-create channels for unmatched streams
        </label>
        <label class="toggle-label">
          <input
            type="checkbox"
            data-field="iptv_health_check_enabled"
            checked={systemConfig.iptv_health_check_enabled}
            onchange={(e) => {
              if (systemConfig)
                systemConfig.iptv_health_check_enabled = (e.target as HTMLInputElement).checked;
            }}
          />
          Enable periodic source health checks
        </label>
        <button
          class="action-btn"
          type="button"
          onclick={saveSystemConfig}
          disabled={systemConfigSaving}
        >
          {systemConfigSaving ? 'Saving...' : 'Save Defaults'}
        </button>
      </div>
    {/if}
  </div>

  <!-- Tab bar -->
  <div class="tab-bar">
    <button
      class="tab-btn"
      class:active={activeTab === 'providers'}
      type="button"
      onclick={() => switchTab('providers')}
    >
      Providers
    </button>
    <button
      class="tab-btn"
      class:active={activeTab === 'channels'}
      type="button"
      onclick={() => switchTab('channels')}
    >
      Channels
    </button>
    <button
      class="tab-btn"
      class:active={activeTab === 'unmatched'}
      type="button"
      onclick={() => switchTab('unmatched')}
    >
      Unmatched
      {#if unmatchedTotal > 0}
        <span class="tab-badge">{unmatchedTotal}</span>
      {/if}
    </button>
    <button
      class="tab-btn"
      class:active={activeTab === 'watches'}
      type="button"
      onclick={() => switchTab('watches')}
    >
      Watches
      {#if watchMatchCount > 0}
        <span class="tab-badge accent">{watchMatchCount}</span>
      {/if}
    </button>
    <button
      class="tab-btn"
      class:active={activeTab === 'recording'}
      type="button"
      onclick={() => switchTab('recording')}
    >
      Recording
    </button>
  </div>

  {#if loading}
    <div class="skeleton-list">
      {#each Array(4) as _, i (i)}
        <div class="skeleton-item">
          <div class="skeleton-line wide"></div>
          <div class="skeleton-line narrow"></div>
        </div>
      {/each}
    </div>
  {:else if activeTab === 'providers'}
    <!-- Providers tab -->
    <div class="tab-content">
      <div class="tab-actions">
        <button class="add-btn" type="button" onclick={() => (showAddProvider = !showAddProvider)}>
          <span class="material-icons-outlined" style="font-size: 14px;">add</span>
          Add Provider
        </button>
      </div>

      {#if showAddProvider}
        <div class="form-wrapper">
          <ProviderForm
            onsubmit={handleAddProvider}
            oncancel={() => {
              showAddProvider = false;
              testResult = null;
            }}
            saving={addingSaving}
          />
          {#if testResult}
            <div class="test-result" class:success={testResult.ok} class:fail={!testResult.ok}>
              <span class="material-icons-outlined" style="font-size: 14px;">
                {testResult.ok ? 'check_circle' : 'error'}
              </span>
              {testResult.message}
            </div>
          {/if}
        </div>
      {/if}

      {#if providers.length === 0}
        <div class="empty-state">
          <p>No IPTV providers configured</p>
          <p class="muted">Add a provider to start streaming live TV</p>
        </div>
      {:else}
        <div class="provider-list">
          {#each providers as provider (provider.id)}
            <div
              class="provider-card"
              role="button"
              tabindex="0"
              onclick={() => handleSelectProvider(provider)}
              onkeydown={(e) => {
                if (e.key === 'Enter') handleSelectProvider(provider);
              }}
            >
              <div class="provider-header">
                <div class="provider-info">
                  <span class="provider-name">{provider.name}</span>
                  <span class="provider-url">{provider.url}</span>
                </div>
                <span class="enabled-badge" class:disabled={!provider.enabled}>
                  {provider.enabled ? 'Enabled' : 'Disabled'}
                </span>
              </div>
              <div class="provider-meta">
                <span>User: {provider.username}</span>
                <span>Format: {provider.preferred_format}</span>
                <span>Last sync: {formatDate(provider.last_sync_at)}</span>
              </div>
              {#if provider.last_sync_error}
                <div class="sync-error">{provider.last_sync_error}</div>
              {/if}
              <div class="provider-card-actions">
                <button
                  class="action-btn"
                  type="button"
                  disabled={syncingIds.has(provider.id)}
                  onclick={(e) => {
                    e.stopPropagation();
                    handleSync(provider.id);
                  }}
                >
                  {syncingIds.has(provider.id) ? 'Syncing...' : 'Sync Now'}
                </button>
                <button
                  class="action-btn danger"
                  type="button"
                  onclick={(e) => {
                    e.stopPropagation();
                    confirmDeleteProviderId = provider.id;
                  }}
                >
                  Delete
                </button>
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {:else if activeTab === 'channels'}
    <!-- Channels tab -->
    <div class="tab-content">
      <div class="channels-toolbar">
        <div class="search-bar">
          <span class="material-icons-outlined search-icon">search</span>
          <input
            type="text"
            placeholder="Search channels..."
            bind:value={channelSearch}
            oninput={handleChannelSearch}
          />
          {#if channelSearch}
            <button
              type="button"
              class="clear-btn"
              aria-label="Clear search"
              onclick={() => {
                channelSearch = '';
                handleChannelSearch();
              }}
            >
              <span class="material-icons-outlined" style="font-size: 14px;">close</span>
            </button>
          {/if}
        </div>

        {#if selectedChannelIds.size >= 2}
          <button class="merge-btn" type="button" onclick={startMerge}>
            <span class="material-icons-outlined" style="font-size: 14px;">merge_type</span>
            Merge ({selectedChannelIds.size})
          </button>
        {/if}
      </div>

      <p class="result-count">{channelTotal} channels</p>

      <div class="channel-list">
        {#each channels as channel (channel.id)}
          <div class="channel-row" class:disabled-channel={!channel.enabled}>
            <label class="channel-checkbox">
              <input
                type="checkbox"
                checked={selectedChannelIds.has(channel.id)}
                onchange={() => toggleChannelSelection(channel.id)}
              />
            </label>
            <div class="channel-info">
              {#if channel.icon_url}
                <img src={channel.icon_url} alt="" class="ch-icon" />
              {/if}
              <div class="ch-text">
                <span class="ch-name">{channel.name}</span>
                <span class="ch-meta">
                  {channel.source_count} sources
                  {#if channel.category}
                    | {channel.category}
                  {/if}
                </span>
              </div>
            </div>
            <button
              class="toggle-btn"
              class:active={channel.enabled}
              type="button"
              disabled={togglingIds.has(channel.id)}
              onclick={() => handleToggleChannel(channel.id, channel.enabled)}
              aria-label={channel.enabled ? 'Disable' : 'Enable'}
            >
              <div class="toggle-knob"></div>
            </button>
          </div>
        {/each}
      </div>

      {#if channelTotal > 50}
        <div class="pagination">
          <button
            class="page-btn"
            type="button"
            disabled={channelOffset === 0}
            onclick={() => {
              channelOffset = Math.max(0, channelOffset - 50);
              loadChannels();
            }}
          >
            Previous
          </button>
          <span class="page-info">
            {channelOffset + 1}-{Math.min(channelOffset + 50, channelTotal)} of {channelTotal}
          </span>
          <button
            class="page-btn"
            type="button"
            disabled={channelOffset + 50 >= channelTotal}
            onclick={() => {
              channelOffset += 50;
              loadChannels();
            }}
          >
            Next
          </button>
        </div>
      {/if}
    </div>
  {:else if activeTab === 'watches'}
    <!-- Watches tab -->
    <div class="tab-content">
      <div class="tab-actions">
        <button class="add-btn" type="button" onclick={() => (showNewWatch = !showNewWatch)}>
          <span class="material-icons-outlined" style="font-size: 14px;">add</span>
          Add Watch
        </button>
      </div>

      {#if showNewWatch}
        <div class="form-wrapper">
          <form
            onsubmit={(e) => {
              e.preventDefault();
              handleCreateWatch();
            }}
          >
            <div class="form-row">
              <div class="form-field">
                <label for="watch-pattern">Title Pattern</label>
                <input
                  id="watch-pattern"
                  type="text"
                  bind:value={newWatchPattern}
                  placeholder="e.g. Premier League"
                  required
                />
              </div>
              <div class="form-field">
                <label for="watch-mode">Match Mode</label>
                <select id="watch-mode" bind:value={newWatchMode}>
                  <option value="contains">Contains</option>
                  <option value="exact">Exact</option>
                  <option value="regex">Regex</option>
                </select>
              </div>
            </div>
            <div class="form-actions">
              <button type="button" class="btn-sec" onclick={() => (showNewWatch = false)}
                >Cancel</button
              >
              <button
                type="submit"
                class="btn-primary"
                disabled={watchSaving || !newWatchPattern.trim()}
              >
                {watchSaving ? 'Creating...' : 'Create Watch'}
              </button>
            </div>
          </form>
        </div>
      {/if}

      {#if watches.length === 0}
        <div class="empty-state">
          <p>No programme watches configured</p>
          <p class="muted">Create watches to get notified about matching EPG programmes</p>
        </div>
      {:else}
        <div class="watches-list">
          {#each watches as watch (watch.id)}
            {@const matchCount = watchMatches.filter((m) => m.watch_id === watch.id).length}
            <div class="watch-item">
              <div class="watch-info">
                <div class="watch-title-row">
                  <span class="watch-pattern">{watch.title_pattern}</span>
                  <span class="watch-mode-badge">{watch.match_mode}</span>
                  {#if matchCount > 0}
                    <span class="watch-match-count">{matchCount} matches</span>
                  {/if}
                </div>
                <div class="watch-meta">
                  <span class="watch-status" class:disabled={!watch.enabled}>
                    {watch.enabled ? 'Enabled' : 'Disabled'}
                  </span>
                  <span>Created {formatDate(watch.created_at)}</span>
                </div>
              </div>
              <button
                class="icon-btn-sm danger"
                type="button"
                onclick={() => (confirmDeleteWatchId = watch.id)}
                aria-label="Delete watch"
              >
                <span class="material-icons-outlined" style="font-size: 14px;">delete</span>
              </button>
            </div>
          {/each}
        </div>

        {#if watchMatches.length > 0}
          <div class="matches-section">
            <h3>Current Matches</h3>
            <div class="matches-list">
              {#each watchMatches as match (match.watch_id + match.channel_id + match.start_time)}
                <div class="match-item">
                  <div class="match-info">
                    <span class="match-title">{match.epg_title}</span>
                    <span class="match-meta">
                      {match.channel_name} --
                      {new Date(match.start_time).toLocaleString(undefined, {
                        month: 'short',
                        day: 'numeric',
                        hour: '2-digit',
                        minute: '2-digit'
                      })}
                    </span>
                  </div>
                  {#if match.is_live}
                    <span class="live-badge">LIVE</span>
                  {/if}
                </div>
              {/each}
            </div>
          </div>
        {/if}
      {/if}
    </div>
  {:else}
    <!-- Unmatched tab -->
    <div class="tab-content">
      <p class="result-count">{unmatchedTotal} unmatched sources</p>
      <UnmatchedReview sources={unmatched} onrefresh={loadUnmatched} />
    </div>
  {/if}
</div>

<!-- Provider detail sidebar -->
<ExpandableSidebar
  open={sidebarOpen}
  title={selectedProvider?.name ?? 'Provider Details'}
  onClose={handleCloseSidebar}
>
  {#if selectedProvider}
    {#if editingProvider}
      <div class="sidebar-content">
        <ProviderForm
          provider={selectedProvider}
          onsubmit={handleUpdateProvider}
          oncancel={() => (editingProvider = false)}
          saving={editSaving}
        />
      </div>
    {:else}
      <div class="sidebar-content">
        <div class="sidebar-field">
          <span class="sidebar-label">Status</span>
          <span class="enabled-badge" class:disabled={!selectedProvider.enabled}>
            {selectedProvider.enabled ? 'Enabled' : 'Disabled'}
          </span>
        </div>

        <div class="sidebar-field">
          <span class="sidebar-label">Server URL</span>
          <span class="sidebar-value url">{selectedProvider.url}</span>
        </div>

        <div class="sidebar-field">
          <span class="sidebar-label">Username</span>
          <span class="sidebar-value">{selectedProvider.username}</span>
        </div>

        <div class="sidebar-field">
          <span class="sidebar-label">Preferred Format</span>
          <span class="sidebar-value">{selectedProvider.preferred_format}</span>
        </div>

        <div class="sidebar-field">
          <span class="sidebar-label">Sync Interval</span>
          <span class="sidebar-value"
            >{Math.round(selectedProvider.sync_interval_secs / 3600)}h</span
          >
        </div>

        {#if selectedProvider.account_info && Object.keys(selectedProvider.account_info).length > 0}
          <div class="sidebar-section">
            <h4>Account Info</h4>
            <div class="account-info-grid">
              {#each Object.entries(selectedProvider.account_info) as [key, value] (key)}
                <div class="account-info-item">
                  <span class="sidebar-label">{key}</span>
                  <span class="sidebar-value">{String(value)}</span>
                </div>
              {/each}
            </div>
          </div>
        {/if}

        <div class="sidebar-field">
          <span class="sidebar-label">Last Sync</span>
          <span class="sidebar-value">{formatDate(selectedProvider.last_sync_at)}</span>
        </div>

        {#if selectedProvider.last_sync_error}
          <div class="sidebar-error">
            <span class="material-icons-outlined" style="font-size: 14px;">error</span>
            {selectedProvider.last_sync_error}
          </div>
        {/if}

        <div class="sidebar-actions">
          <button class="action-btn-full" type="button" onclick={() => (editingProvider = true)}>
            <span class="material-icons-outlined" style="font-size: 16px;">edit</span>
            Edit Provider
          </button>
          <button
            class="action-btn-full"
            type="button"
            disabled={syncingIds.has(selectedProvider.id)}
            onclick={() => handleSync(selectedProvider!.id)}
          >
            <span class="material-icons-outlined" style="font-size: 16px;">sync</span>
            {syncingIds.has(selectedProvider.id) ? 'Syncing...' : 'Sync Now'}
          </button>
          <button
            class="action-btn-full danger"
            type="button"
            onclick={() => (confirmDeleteProviderId = selectedProvider!.id)}
          >
            <span class="material-icons-outlined" style="font-size: 16px;">delete</span>
            Delete Provider
          </button>
        </div>
      </div>
    {/if}
  {/if}
</ExpandableSidebar>

<!-- Confirm delete provider dialog -->
{#if confirmDeleteProviderId}
  <div class="modal-backdrop" role="presentation" onclick={() => (confirmDeleteProviderId = null)}>
    <div class="modal-card" role="presentation" onclick={(e) => e.stopPropagation()}>
      <h3>Delete Provider</h3>
      <p>
        Are you sure you want to delete this provider? All associated channels and sources will be
        removed.
      </p>
      <div class="modal-actions">
        <button type="button" class="btn-sec" onclick={() => (confirmDeleteProviderId = null)}
          >Cancel</button
        >
        <button type="button" class="btn-danger" onclick={handleConfirmDeleteProvider}
          >Delete</button
        >
      </div>
    </div>
  </div>
{/if}

<!-- Confirm disable channel dialog -->
{#if confirmDisableChannelId}
  <div class="modal-backdrop" role="presentation" onclick={() => (confirmDisableChannelId = null)}>
    <div class="modal-card" role="presentation" onclick={(e) => e.stopPropagation()}>
      <h3>Disable Channel</h3>
      <p>Disable this channel? It will no longer appear in the guide or channel list.</p>
      <div class="modal-actions">
        <button type="button" class="btn-sec" onclick={() => (confirmDisableChannelId = null)}
          >Cancel</button
        >
        <button type="button" class="btn-danger" onclick={handleConfirmDisableChannel}
          >Disable</button
        >
      </div>
    </div>
  </div>
{/if}

<!-- Confirm delete watch dialog -->
{#if confirmDeleteWatchId}
  <div class="modal-backdrop" role="presentation" onclick={() => (confirmDeleteWatchId = null)}>
    <div class="modal-card" role="presentation" onclick={(e) => e.stopPropagation()}>
      <h3>Delete Watch</h3>
      <p>Are you sure you want to delete this programme watch?</p>
      <div class="modal-actions">
        <button type="button" class="btn-sec" onclick={() => (confirmDeleteWatchId = null)}
          >Cancel</button
        >
        <button type="button" class="btn-danger" onclick={handleConfirmDeleteWatch}>Delete</button>
      </div>
    </div>
  </div>
{/if}

<!-- Recording tab -->
{#if activeTab === 'recording'}
  <div class="recording-settings">
    <div class="setting-group">
      <label class="setting-label" for="recording-path">Recording Path</label>
      <input
        id="recording-path"
        type="text"
        class="setting-input"
        placeholder="/path/to/recordings"
        bind:value={settings.recording_path}
      />
      <span class="setting-hint">Directory where IPTV recordings are saved</span>
    </div>

    <div class="setting-group">
      <label class="setting-label" for="epg-retention">EPG Retention (days)</label>
      <div class="setting-row">
        <input
          id="epg-retention"
          type="range"
          min="1"
          max="14"
          bind:value={settings.epg_retention_days}
          class="setting-slider"
        />
        <span class="setting-value">{settings.epg_retention_days ?? 7} days</span>
      </div>
      <span class="setting-hint"
        >How long to keep EPG history for favourite channels (for catchup)</span
      >
    </div>

    <div class="setting-group">
      <label class="setting-label" for="default-format">Default Stream Format</label>
      <select id="default-format" class="setting-input" bind:value={settings.default_format}>
        <option value="m3u8">HLS (m3u8)</option>
        <option value="ts">MPEG-TS</option>
        <option value="mkv">MKV</option>
      </select>
      <span class="setting-hint">Preferred format for live streams</span>
    </div>

    <button
      class="btn-primary save-btn"
      type="button"
      disabled={settingsSaving}
      onclick={saveSettings}
    >
      {settingsSaving ? 'Saving...' : 'Save Settings'}
    </button>
  </div>
{/if}

<!-- Merge modal -->
{#if showMergeModal}
  <div class="modal-backdrop" role="presentation" onclick={cancelMerge}>
    <div class="modal-card" role="presentation" onclick={(e) => e.stopPropagation()}>
      <h3>Merge Channels</h3>
      <p>Select the canonical name for the merged channel:</p>
      <div class="merge-options">
        {#each channels.filter((c) => selectedChannelIds.has(c.id)) as ch (ch.id)}
          <label class="merge-option">
            <input type="radio" name="merge-name" bind:group={mergeCanonicalName} value={ch.name} />
            <span>{ch.name}</span>
          </label>
        {/each}
      </div>
      <div class="modal-actions">
        <button type="button" class="btn-sec" onclick={cancelMerge}>Cancel</button>
        <button
          type="button"
          class="btn-primary"
          disabled={!mergeCanonicalName}
          onclick={handleMerge}>Merge</button
        >
      </div>
    </div>
  </div>
{/if}

<style>
  .error-wrapper {
    padding: 12px 24px 0;
  }

  .settings-page {
    max-width: 900px;
    margin: 0 auto;
    padding: 24px;
  }

  h1 {
    font-size: 24px;
    font-weight: 700;
    margin: 0 0 20px;
  }

  /* Defaults card */
  .defaults-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 14px;
    padding: 16px;
    margin-bottom: 20px;
    display: grid;
    gap: 12px;
  }

  .defaults-card h3 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }

  .defaults-fields {
    display: grid;
    gap: 12px;
  }

  .defaults-fields label {
    display: grid;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .defaults-fields input[type='text'] {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .defaults-fields .toggle-label {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--ink);
    cursor: pointer;
  }

  .defaults-fields .toggle-label input[type='checkbox'] {
    width: 16px;
    height: 16px;
    accent-color: var(--accent);
  }

  .field-hint {
    font-size: 11px;
    color: var(--muted);
  }

  /* Tabs */
  .tab-bar {
    display: flex;
    gap: 2px;
    border-bottom: 1px solid var(--ring);
    margin-bottom: 20px;
    overflow-x: auto;
  }

  .tab-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 10px 20px;
    border: none;
    background: transparent;
    color: var(--muted);
    font-size: 14px;
    font-weight: 500;
    cursor: pointer;
    border-bottom: 2px solid transparent;
    margin-bottom: -1px;
    transition: all 0.15s;
    white-space: nowrap;
  }

  .tab-btn:hover {
    color: var(--ink);
  }

  .tab-btn.active {
    color: var(--accent);
    border-bottom-color: var(--accent);
  }

  .tab-badge {
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 10px;
    background: var(--accent);
    color: var(--bg);
    font-weight: 700;
  }

  .tab-badge.accent {
    background: var(--accent-dim);
    color: var(--accent);
  }

  .tab-content {
    min-height: 200px;
  }

  .tab-actions {
    display: flex;
    justify-content: flex-end;
    margin-bottom: 16px;
  }

  /* Skeleton */
  .skeleton-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-top: 8px;
  }

  .skeleton-item {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
  }

  .skeleton-line {
    height: 12px;
    border-radius: 4px;
    background: var(--ring);
    animation: skeleton-pulse 1.5s ease-in-out infinite;
  }

  .skeleton-line.wide {
    width: 55%;
  }
  .skeleton-line.narrow {
    width: 30%;
  }

  @keyframes skeleton-pulse {
    0%,
    100% {
      opacity: 0.4;
    }
    50% {
      opacity: 0.7;
    }
  }

  /* Buttons */
  .add-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 7px 14px;
    border-radius: 8px;
    border: 1px solid var(--accent);
    background: var(--accent);
    color: var(--bg);
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
  }

  .add-btn:hover {
    opacity: 0.9;
  }

  .btn-primary {
    padding: 8px 20px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    border: 1px solid var(--accent);
    background: var(--accent);
    color: var(--bg);
    transition: all 0.15s;
  }

  .btn-primary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-sec {
    padding: 8px 20px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    transition: all 0.15s;
  }

  .btn-danger {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    border: 1px solid var(--danger, #ef4444);
    background: transparent;
    color: var(--danger, #ef4444);
    transition: all 0.15s;
  }

  .btn-danger:hover {
    background: rgba(239, 68, 68, 0.1);
  }

  /* Form */
  .form-wrapper {
    margin-bottom: 20px;
    padding: 16px;
    border: 1px solid var(--ring);
    border-radius: 12px;
  }

  .form-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    flex: 1;
    margin-bottom: 12px;
  }

  .form-field label {
    font-size: 12px;
    font-weight: 600;
    color: var(--muted);
  }

  .form-field input,
  .form-field select {
    padding: 8px 12px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--bg);
    color: var(--ink);
    font-size: 14px;
    outline: none;
  }

  .form-field input:focus,
  .form-field select:focus {
    border-color: var(--accent);
  }

  .form-row {
    display: flex;
    gap: 12px;
  }

  .form-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .test-result {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 12px;
    padding: 8px 12px;
    border-radius: 8px;
    font-size: 13px;
  }

  .test-result.success {
    background: rgba(34, 197, 94, 0.1);
    color: #22c55e;
  }

  .test-result.fail {
    background: rgba(239, 68, 68, 0.1);
    color: #ef4444;
  }

  /* Providers */
  .provider-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .provider-card {
    padding: 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    cursor: pointer;
    text-align: left;
    font: inherit;
    color: inherit;
    width: 100%;
    transition: border-color 0.15s;
  }

  .provider-card:hover {
    border-color: var(--accent);
  }

  .provider-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }

  .provider-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .provider-name {
    font-size: 15px;
    font-weight: 600;
  }

  .provider-url {
    font-size: 12px;
    color: var(--muted);
    word-break: break-all;
  }

  .enabled-badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 10px;
    background: rgba(34, 197, 94, 0.15);
    color: #22c55e;
    font-weight: 600;
    flex-shrink: 0;
  }

  .enabled-badge.disabled {
    background: rgba(239, 68, 68, 0.15);
    color: #ef4444;
  }

  .provider-meta {
    display: flex;
    gap: 16px;
    font-size: 12px;
    color: var(--muted);
    flex-wrap: wrap;
  }

  .sync-error {
    font-size: 12px;
    color: var(--danger, #ef4444);
    padding: 6px 10px;
    background: rgba(239, 68, 68, 0.08);
    border-radius: 6px;
  }

  .provider-card-actions {
    display: flex;
    gap: 8px;
  }

  .action-btn {
    padding: 5px 14px;
    border-radius: 6px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
  }

  .action-btn:hover:not(:disabled) {
    border-color: var(--accent);
  }

  .action-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .action-btn.danger:hover {
    color: var(--danger, #ef4444);
    border-color: var(--danger, #ef4444);
  }

  /* Channels tab */
  .channels-toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 12px;
  }

  .search-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 14px;
    border: 1px solid var(--ring);
    border-radius: 10px;
    background: var(--card);
    flex: 1;
  }

  .search-bar:focus-within {
    border-color: var(--accent);
  }

  .search-icon {
    color: var(--muted);
    font-size: 16px;
    flex-shrink: 0;
  }

  .search-bar input {
    flex: 1;
    border: none;
    background: transparent;
    color: var(--ink);
    font-size: 14px;
    outline: none;
  }

  .search-bar input::placeholder {
    color: var(--muted);
  }

  .clear-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    color: var(--muted);
    cursor: pointer;
    padding: 2px;
    border-radius: 4px;
  }

  .clear-btn:hover {
    color: var(--ink);
    background: var(--bg-2);
  }

  .merge-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 14px;
    border-radius: 8px;
    border: 1px solid var(--accent);
    background: var(--accent-dim);
    color: var(--accent);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
    transition: all 0.15s;
  }

  .merge-btn:hover {
    background: var(--accent);
    color: var(--bg);
  }

  .result-count {
    font-size: 12px;
    color: var(--muted);
    margin: 0 0 12px;
  }

  .channel-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .channel-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 12px;
    border-radius: 8px;
    transition: background 0.15s;
  }

  .channel-row:hover {
    background: rgba(255, 255, 255, 0.03);
  }

  :global([data-theme='light']) .channel-row:hover {
    background: rgba(0, 0, 0, 0.02);
  }

  .channel-row.disabled-channel {
    opacity: 0.5;
  }

  .channel-checkbox {
    display: flex;
    align-items: center;
    cursor: pointer;
    flex-shrink: 0;
  }

  .channel-checkbox input {
    width: 14px;
    height: 14px;
    cursor: pointer;
  }

  .channel-info {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    flex: 1;
  }

  .ch-icon {
    width: 28px;
    height: 28px;
    border-radius: 4px;
    object-fit: contain;
    flex-shrink: 0;
  }

  .ch-text {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }

  .ch-name {
    font-size: 13px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .ch-meta {
    font-size: 11px;
    color: var(--muted);
  }

  .toggle-btn {
    width: 36px;
    height: 20px;
    border-radius: 10px;
    border: none;
    background: var(--ring);
    cursor: pointer;
    position: relative;
    transition: background 0.2s;
    flex-shrink: 0;
    padding: 0;
  }

  .toggle-btn.active {
    background: var(--accent);
  }

  .toggle-knob {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: white;
    position: absolute;
    top: 2px;
    left: 2px;
    transition: transform 0.2s;
  }

  .toggle-btn.active .toggle-knob {
    transform: translateX(16px);
  }

  .toggle-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .pagination {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 12px;
    margin-top: 16px;
    padding-top: 16px;
    border-top: 1px solid var(--ring);
  }

  .page-btn {
    padding: 5px 14px;
    border-radius: 6px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
  }

  .page-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .page-info {
    font-size: 12px;
    color: var(--muted);
  }

  /* Watches tab */
  .watches-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .watch-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 14px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 8px;
  }

  .watch-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .watch-title-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .watch-pattern {
    font-size: 14px;
    font-weight: 600;
  }

  .watch-mode-badge {
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.08);
    color: var(--muted);
    text-transform: uppercase;
    font-weight: 600;
  }

  :global([data-theme='light']) .watch-mode-badge {
    background: rgba(0, 0, 0, 0.06);
  }

  .watch-match-count {
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 10px;
    background: var(--accent-dim);
    color: var(--accent);
    font-weight: 600;
  }

  .watch-meta {
    display: flex;
    gap: 8px;
    font-size: 12px;
    color: var(--muted);
  }

  .watch-status.disabled {
    color: var(--danger, #ef4444);
  }

  .icon-btn-sm {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
  }

  .icon-btn-sm.danger:hover {
    color: var(--danger, #ef4444);
    background: rgba(239, 68, 68, 0.1);
  }

  /* Matches section */
  .matches-section {
    margin-top: 24px;
    padding-top: 20px;
    border-top: 1px solid var(--ring);
  }

  .matches-section h3 {
    font-size: 15px;
    font-weight: 600;
    margin: 0 0 12px;
  }

  .matches-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .match-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 12px;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.03);
  }

  :global([data-theme='light']) .match-item {
    background: rgba(0, 0, 0, 0.02);
  }

  .match-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .match-title {
    font-size: 13px;
    font-weight: 500;
  }

  .match-meta {
    font-size: 11px;
    color: var(--muted);
  }

  .live-badge {
    font-size: 9px;
    font-weight: 800;
    padding: 2px 6px;
    border-radius: 4px;
    background: var(--accent);
    color: var(--bg);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    flex-shrink: 0;
  }

  /* Sidebar */
  .sidebar-content {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 4px 0;
  }

  .sidebar-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .sidebar-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .sidebar-value {
    font-size: 14px;
    color: var(--ink);
  }

  .sidebar-value.url {
    font-size: 12px;
    word-break: break-all;
    color: var(--muted);
  }

  .sidebar-section {
    padding-top: 8px;
    border-top: 1px solid var(--ring);
  }

  .sidebar-section h4 {
    font-size: 13px;
    font-weight: 600;
    margin: 0 0 8px;
  }

  .account-info-grid {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .account-info-item {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .sidebar-error {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    padding: 8px 12px;
    background: rgba(239, 68, 68, 0.08);
    border-radius: 6px;
    font-size: 13px;
    color: var(--danger, #ef4444);
  }

  .sidebar-actions {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 8px;
    padding-top: 12px;
    border-top: 1px solid var(--ring);
  }

  .action-btn-full {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 8px 16px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    transition: all 0.15s;
    width: 100%;
  }

  .action-btn-full:hover:not(:disabled) {
    border-color: var(--accent);
  }

  .action-btn-full:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .action-btn-full.danger {
    border-color: var(--danger, #ef4444);
    color: var(--danger, #ef4444);
    background: transparent;
  }

  .action-btn-full.danger:hover {
    background: rgba(239, 68, 68, 0.1);
  }

  /* Recording settings */
  .recording-settings {
    display: flex;
    flex-direction: column;
    gap: 24px;
    max-width: 500px;
    padding: 16px 0;
  }

  .setting-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .setting-label {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
  }

  .setting-input {
    padding: 8px 12px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-1);
    color: var(--ink);
    font-size: 13px;
  }

  .setting-input:focus {
    outline: none;
    border-color: var(--accent);
  }

  .setting-row {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .setting-slider {
    flex: 1;
    accent-color: var(--accent);
  }

  .setting-value {
    font-size: 13px;
    font-weight: 500;
    color: var(--ink);
    min-width: 60px;
  }

  .setting-hint {
    font-size: 11px;
    color: var(--ink-2);
  }

  .save-btn {
    align-self: flex-start;
    padding: 8px 20px;
  }

  /* Modals */
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    z-index: 100;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px;
  }

  .modal-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 24px;
    max-width: 400px;
    width: 100%;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.3);
  }

  .modal-card h3 {
    font-size: 16px;
    font-weight: 600;
    margin: 0 0 8px;
  }

  .modal-card p {
    font-size: 14px;
    color: var(--muted);
    margin: 0 0 20px;
    line-height: 1.5;
  }

  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .merge-options {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-bottom: 16px;
  }

  .merge-option {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 14px;
    cursor: pointer;
    padding: 6px 8px;
    border-radius: 6px;
    transition: background 0.15s;
  }

  .merge-option:hover {
    background: var(--bg-2);
  }

  /* Empty state */
  .empty-state {
    text-align: center;
    padding: 48px;
    color: var(--muted);
  }

  .empty-state p {
    margin: 0;
  }

  .empty-state .muted {
    font-size: 13px;
    margin-top: 4px;
  }

  @media (max-width: 480px) {
    .form-row {
      flex-direction: column;
    }

    .channels-toolbar {
      flex-direction: column;
      align-items: stretch;
    }
  }
</style>
