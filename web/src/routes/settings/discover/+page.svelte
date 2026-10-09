<script lang="ts">
  import { onMount } from 'svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import { getAuthState } from '$lib/stores/auth.svelte';
  import {
    getSystemConfig,
    updateSystemConfig,
    getDiscoverWatchProviders,
    refreshDiscover,
    type SystemConfig,
    type WatchProviderInfo
  } from '$lib/api/client';

  const auth = getAuthState();

  let loading = $state(true);
  let error = $state<string | null>(null);
  let saving = $state(false);
  let refreshing = $state(false);
  let loadingProviders = $state(false);

  let systemConfig = $state<SystemConfig | null>(null);
  let region = $state('US');
  let providers = $state<WatchProviderInfo[]>([]);
  let selectedIds = $state<Set<number>>(new Set());
  let providerSearch = $state('');

  const commonRegions = [
    { code: 'US', label: 'United States' },
    { code: 'GB', label: 'United Kingdom' },
    { code: 'CA', label: 'Canada' },
    { code: 'AU', label: 'Australia' },
    { code: 'DE', label: 'Germany' },
    { code: 'FR', label: 'France' },
    { code: 'NL', label: 'Netherlands' },
    { code: 'SE', label: 'Sweden' },
    { code: 'NO', label: 'Norway' },
    { code: 'DK', label: 'Denmark' },
    { code: 'NZ', label: 'New Zealand' },
    { code: 'IN', label: 'India' },
    { code: 'JP', label: 'Japan' },
    { code: 'BR', label: 'Brazil' }
  ];

  const filteredProviders = $derived(
    providerSearch.trim()
      ? providers.filter((p) =>
          p.provider_name.toLowerCase().includes(providerSearch.toLowerCase())
        )
      : providers
  );

  const selectedCount = $derived(selectedIds.size);

  async function loadConfig() {
    loading = true;
    error = null;
    try {
      systemConfig = await getSystemConfig();
      region = systemConfig.discover_region ?? 'US';
      const current = systemConfig.discover_streaming_services ?? [];
      selectedIds = new Set(current.map((s) => s.id));
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load config';
    } finally {
      loading = false;
    }
  }

  async function loadProviders(regionCode: string) {
    loadingProviders = true;
    try {
      const result = await getDiscoverWatchProviders(regionCode);
      providers = result.providers;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load streaming services');
    } finally {
      loadingProviders = false;
    }
  }

  function toggleProvider(id: number) {
    const next = new Set(selectedIds);
    if (next.has(id)) {
      next.delete(id);
    } else {
      next.add(id);
    }
    selectedIds = next;
  }

  function selectAll() {
    selectedIds = new Set(filteredProviders.map((p) => p.provider_id));
  }

  function deselectAll() {
    selectedIds = new Set();
  }

  async function handleRegionChange(newRegion: string) {
    region = newRegion.toUpperCase().trim();
    if (region.length >= 2) {
      await loadProviders(region);
    }
  }

  async function handleSave() {
    saving = true;
    try {
      const services = providers
        .filter((p) => selectedIds.has(p.provider_id))
        .map((p) => ({ id: p.provider_id, name: p.provider_name }));

      await updateSystemConfig({
        discover_region: region,
        discover_streaming_services: services
      });

      showMessage('Discover settings saved');

      // Trigger discover refresh in the background
      refreshing = true;
      try {
        await refreshDiscover();
        showMessage('Discover content refreshing');
      } catch {
        showMessage('Settings saved but discover refresh failed. Try again later.');
      } finally {
        refreshing = false;
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to save settings');
    } finally {
      saving = false;
    }
  }

  onMount(async () => {
    await loadConfig();
    await loadProviders(region);
  });
</script>

<svelte:head>
  <title>Discover Settings — Kobbler</title>
</svelte:head>

{#if !auth.canManageContent}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <div class="discover-settings">
    <div class="section-header">
      <h2>Discover</h2>
      <p class="section-description">
        Configure your region and streaming services to power the discover page with personalised
        recommendations.
      </p>
    </div>

    {#if loading}
      <div class="loading-card">Loading configuration...</div>
    {:else if error}
      <div class="error">{error}</div>
    {:else}
      <div class="config-form">
        <!-- Region card -->
        <div class="config-card" data-field="discover_region">
          <h3>Region</h3>
          <p class="card-description muted">
            Streaming service availability varies by region. Select your country to see the right
            providers.
          </p>

          <div class="form-group">
            <label for="region-input">Region code</label>
            <input
              id="region-input"
              type="text"
              class="text-input region-input"
              placeholder="e.g. US, GB, AU"
              maxlength="2"
              bind:value={region}
              oninput={(e) => handleRegionChange(e.currentTarget.value)}
            />
          </div>

          <div class="region-chips">
            {#each commonRegions as r (r.code)}
              <button
                class="region-chip"
                class:active={region === r.code}
                type="button"
                onclick={() => handleRegionChange(r.code)}
                title={r.label}
              >
                {r.code}
              </button>
            {/each}
          </div>
        </div>

        <!-- Streaming services card -->
        <div class="config-card" data-field="discover_streaming_services">
          <div class="services-header">
            <div>
              <h3>Streaming Services</h3>
              <p class="card-description muted">
                Select the services you subscribe to. The discover page will show trending content
                from these providers.
                {#if selectedCount > 0}
                  <span class="selected-count">{selectedCount} selected</span>
                {/if}
              </p>
            </div>
            <div class="services-actions">
              <button class="btn-text" type="button" onclick={selectAll}>Select all</button>
              <button class="btn-text" type="button" onclick={deselectAll}>Clear</button>
            </div>
          </div>

          {#if providers.length > 8}
            <div class="search-bar">
              <svg
                viewBox="0 0 24 24"
                width="16"
                height="16"
                fill="currentColor"
                class="search-icon"
              >
                <path
                  d="M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z"
                ></path>
              </svg>
              <input
                type="text"
                class="text-input search-input"
                placeholder="Filter services..."
                bind:value={providerSearch}
              />
            </div>
          {/if}

          {#if loadingProviders}
            <div class="providers-loading">Loading available services...</div>
          {:else if providers.length === 0}
            <div class="providers-empty">
              No streaming services found for region <strong>{region}</strong>. Try a different
              region code.
            </div>
          {:else}
            <div class="providers-grid">
              {#each filteredProviders as provider (provider.provider_id)}
                {@const isSelected = selectedIds.has(provider.provider_id)}
                <button
                  class="provider-card"
                  class:selected={isSelected}
                  type="button"
                  onclick={() => toggleProvider(provider.provider_id)}
                >
                  <div class="provider-logo-wrap">
                    {#if provider.logo_url}
                      <img
                        src={provider.logo_url}
                        alt={provider.provider_name}
                        class="provider-logo"
                        loading="lazy"
                      />
                    {:else}
                      <div class="provider-logo-placeholder">
                        {provider.provider_name.charAt(0)}
                      </div>
                    {/if}
                  </div>
                  <span class="provider-name">{provider.provider_name}</span>
                  <div class="check-indicator" class:visible={isSelected}>
                    <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor">
                      <path d="M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z"></path>
                    </svg>
                  </div>
                </button>
              {/each}
            </div>

            {#if providerSearch && filteredProviders.length === 0}
              <div class="providers-empty">
                No services matching "<strong>{providerSearch}</strong>"
              </div>
            {/if}
          {/if}
        </div>

        <!-- Save -->
        <div class="save-row">
          <button class="save-button" onclick={handleSave} disabled={saving || refreshing}>
            {#if saving}
              Saving...
            {:else if refreshing}
              Refreshing...
            {:else}
              Save & Refresh Discover
            {/if}
          </button>
          {#if selectedCount === 0}
            <span class="save-hint muted">
              No services selected. The discover page will show general trending content.
            </span>
          {/if}
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

  .discover-settings {
    display: grid;
    gap: 16px;
  }

  .section-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .section-description {
    font-size: 13px;
    color: var(--muted);
    margin: 4px 0 0;
  }

  .config-form {
    display: grid;
    gap: 16px;
  }

  .config-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 16px;
    display: grid;
    gap: 12px;
  }

  .config-card h3 {
    margin: 0 0 4px;
    font-size: 15px;
    font-weight: 600;
  }

  .card-description {
    font-size: 13px;
    margin: 0;
  }

  .muted {
    color: var(--muted);
  }

  .selected-count {
    display: inline-block;
    margin-left: 4px;
    padding: 1px 8px;
    border-radius: 10px;
    background: var(--accent);
    color: white;
    font-size: 11px;
    font-weight: 600;
  }

  .form-group {
    display: grid;
    gap: 4px;
    margin-bottom: 4px;
  }

  .form-group label {
    font-size: 12px;
    font-weight: 500;
    color: var(--muted);
  }

  .text-input {
    padding: 6px 10px;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: var(--field);
    color: var(--ink);
    font-size: 13px;
    width: 100%;
    box-sizing: border-box;
  }

  .text-input:focus {
    outline: none;
    border-color: var(--accent);
  }

  .region-input {
    max-width: 120px;
    text-transform: uppercase;
    font-weight: 600;
    letter-spacing: 0.05em;
  }

  .region-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .region-chip {
    padding: 4px 12px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--muted);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
    letter-spacing: 0.03em;
  }

  .region-chip:hover {
    border-color: var(--accent);
    color: var(--ink);
  }

  .region-chip.active {
    background: var(--accent);
    border-color: var(--accent);
    color: white;
  }

  .services-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }

  .services-actions {
    display: flex;
    gap: 8px;
    flex-shrink: 0;
    padding-top: 2px;
  }

  .btn-text {
    background: none;
    border: none;
    color: var(--accent);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    padding: 2px 4px;
  }

  .btn-text:hover {
    text-decoration: underline;
  }

  .search-bar {
    position: relative;
  }

  .search-icon {
    position: absolute;
    left: 10px;
    top: 50%;
    transform: translateY(-50%);
    color: var(--muted);
    pointer-events: none;
  }

  .search-input {
    padding-left: 32px;
  }

  .providers-loading,
  .providers-empty {
    padding: 24px;
    text-align: center;
    font-size: 13px;
    color: var(--muted);
    border: 1px dashed var(--ring);
    border-radius: 12px;
  }

  .providers-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    gap: 8px;
  }

  @media (max-width: 480px) {
    .providers-grid {
      grid-template-columns: repeat(auto-fill, minmax(110px, 1fr));
    }
  }

  .provider-card {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 12px 8px;
    border-radius: 12px;
    border: 1.5px solid var(--ring);
    background: var(--card);
    cursor: pointer;
    transition: all 0.15s;
    text-align: center;
  }

  .provider-card:hover {
    border-color: color-mix(in srgb, var(--accent) 50%, transparent);
    background: color-mix(in srgb, var(--accent) 4%, var(--card));
  }

  .provider-card.selected {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 8%, var(--card));
  }

  .provider-logo-wrap {
    width: 48px;
    height: 48px;
    border-radius: 10px;
    overflow: hidden;
    flex-shrink: 0;
  }

  .provider-logo {
    width: 100%;
    height: 100%;
    object-fit: cover;
    border-radius: 10px;
  }

  .provider-logo-placeholder {
    width: 100%;
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--ring);
    color: var(--muted);
    font-size: 20px;
    font-weight: 700;
    border-radius: 10px;
  }

  .provider-name {
    font-size: 11px;
    font-weight: 500;
    color: var(--ink);
    line-height: 1.3;
    overflow: hidden;
    text-overflow: ellipsis;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    max-width: 100%;
  }

  .check-indicator {
    position: absolute;
    top: 6px;
    right: 6px;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: var(--accent);
    color: white;
    display: flex;
    align-items: center;
    justify-content: center;
    opacity: 0;
    transform: scale(0.5);
    transition: all 0.15s;
  }

  .check-indicator.visible {
    opacity: 1;
    transform: scale(1);
  }

  .save-row {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }

  .save-button {
    padding: 8px 20px;
    border: 1px solid var(--accent);
    border-radius: 10px;
    background: var(--accent);
    color: white;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: opacity 0.15s;
    white-space: nowrap;
  }

  .save-button:hover {
    opacity: 0.9;
  }

  .save-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .save-hint {
    font-size: 12px;
  }

  .loading-card {
    padding: 24px;
    text-align: center;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
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
