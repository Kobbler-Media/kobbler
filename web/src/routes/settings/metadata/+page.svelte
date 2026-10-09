<script lang="ts">
  import { onMount } from 'svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import { getAuthState } from '$lib/stores/auth.svelte';
  import {
    getMetadataProviders,
    updateMetadataProvider,
    getVideoMetadataProviders,
    updateVideoMetadataProvider,
    clearMetadataCache,
    triggerLibraryCleanup,
    triggerMetadataWriteAll,
    triggerSidecarRewrite,
    getSystemConfig,
    updateSystemConfig,
    type MetadataProvider,
    type MetadataProviderUpdate,
    type SystemConfig,
    type SystemConfigUpdate
  } from '$lib/api/client';
  import type { VideoMetadataProvider } from '$lib/types';

  const auth = getAuthState();

  let metadataProviders = $state<MetadataProvider[]>([]);
  let videoProviders = $state<VideoMetadataProvider[]>([]);
  let loading = $state(true);
  let videoLoading = $state(true);
  let error = $state<string | null>(null);
  let videoError = $state<string | null>(null);
  let clearingCache = $state(false);
  let cleaningLibrary = $state(false);
  let writingMetadata = $state(false);
  let rewritingSidecars = $state(false);
  let editingProviderId = $state<string | null>(null);
  let editingEnabled = $state(false);
  let editingPriority = $state(1);
  let editingConfig = $state<Record<string, unknown>>({});

  // System config state for global settings
  let systemConfig = $state<SystemConfig | null>(null);
  let systemConfigLoading = $state(true);
  let systemConfigSaving = $state(false);

  let editingVideoProviderId = $state<string | null>(null);
  let editingVideoEnabled = $state(false);
  let editingVideoPriority = $state(1);
  let editingVideoConfig = $state<Record<string, unknown>>({});

  const sortedMetadataProviders = $derived(
    [...metadataProviders].sort((a, b) => a.priority - b.priority)
  );

  const sortedVideoProviders = $derived(
    [...videoProviders].sort((a, b) => a.priority - b.priority)
  );

  onMount(async () => {
    await Promise.all([loadMetadataProviders(), loadVideoProviders(), loadSystemConfig()]);
  });

  const loadSystemConfig = async () => {
    systemConfigLoading = true;
    try {
      systemConfig = await getSystemConfig();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load global settings');
    } finally {
      systemConfigLoading = false;
    }
  };

  const saveGlobalSettings = async () => {
    if (!systemConfig) return;
    systemConfigSaving = true;
    try {
      const updates: SystemConfigUpdate = {
        metadata_providers_enabled: systemConfig.metadata_providers_enabled,
        metadata_cache_days: systemConfig.metadata_cache_days
      };
      systemConfig = await updateSystemConfig(updates);
      showMessage('Global settings saved');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to save global settings');
    } finally {
      systemConfigSaving = false;
    }
  };

  const loadMetadataProviders = async () => {
    loading = true;
    error = null;
    try {
      metadataProviders = await getMetadataProviders();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load providers';
    } finally {
      loading = false;
    }
  };

  const loadVideoProviders = async () => {
    videoLoading = true;
    videoError = null;
    try {
      videoProviders = await getVideoMetadataProviders();
    } catch (e) {
      videoError = e instanceof Error ? e.message : 'Failed to load video providers';
    } finally {
      videoLoading = false;
    }
  };

  const startEditVideoProvider = (provider: VideoMetadataProvider) => {
    editingVideoProviderId = provider.id;
    editingVideoEnabled = provider.enabled;
    editingVideoPriority = provider.priority;
    editingVideoConfig = {};
    for (const field of provider.config_fields) {
      if (field.current_value !== null && field.current_value !== undefined) {
        editingVideoConfig[field.key] = field.current_value;
      } else {
        editingVideoConfig[field.key] = '';
      }
    }
  };

  const cancelEditVideoProvider = () => {
    editingVideoProviderId = null;
  };

  const saveVideoProvider = async (providerId: string) => {
    try {
      const configUpdate: Record<string, unknown> = {};
      const provider = videoProviders.find((p) => p.id === providerId);
      for (const [key, value] of Object.entries(editingVideoConfig)) {
        const fieldDef = provider?.config_fields.find((f) => f.key === key);
        if (value !== '') {
          configUpdate[key] = value;
        } else if (fieldDef?.field_type === 'secret' && fieldDef?.current_value) {
          configUpdate[key] = null;
        }
      }

      await updateVideoMetadataProvider(providerId, {
        enabled: editingVideoEnabled,
        priority: editingVideoPriority,
        config: Object.keys(configUpdate).length > 0 ? configUpdate : undefined
      });
      await loadVideoProviders();
      editingVideoProviderId = null;
      editingVideoConfig = {};
      showMessage('Video provider updated');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update video provider');
    }
  };

  const toggleVideoProviderEnabled = async (provider: VideoMetadataProvider) => {
    try {
      await updateVideoMetadataProvider(provider.id, { enabled: !provider.enabled });
      await loadVideoProviders();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to toggle video provider');
    }
  };

  const formatCapability = (cap: string): string => {
    return cap
      .split('_')
      .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
      .join(' ');
  };

  const startEditProvider = (provider: MetadataProvider) => {
    editingProviderId = provider.id;
    editingEnabled = provider.enabled;
    editingPriority = provider.priority;
    editingConfig = {};
    for (const field of provider.config_fields) {
      if (field.current_value !== null && field.current_value !== undefined) {
        editingConfig[field.key] = field.current_value;
      } else {
        editingConfig[field.key] = '';
      }
    }
  };

  const cancelEditProvider = () => {
    editingProviderId = null;
    editingConfig = {};
  };

  const saveProvider = async (providerId: string) => {
    try {
      const configUpdate: Record<string, unknown> = {};
      const provider = metadataProviders.find((p) => p.id === providerId);
      for (const [key, value] of Object.entries(editingConfig)) {
        const fieldDef = provider?.config_fields.find((f) => f.key === key);
        if (value !== '') {
          configUpdate[key] = value;
        } else if (fieldDef?.field_type === 'secret' && fieldDef?.current_value) {
          configUpdate[key] = null;
        }
      }

      const update: MetadataProviderUpdate = {
        enabled: editingEnabled,
        priority: editingPriority,
        config: Object.keys(configUpdate).length > 0 ? configUpdate : undefined
      };

      await updateMetadataProvider(providerId, update);
      await loadMetadataProviders();
      editingProviderId = null;
      editingConfig = {};
      showMessage('Provider updated');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update provider');
    }
  };

  const toggleProviderEnabled = async (provider: MetadataProvider) => {
    try {
      await updateMetadataProvider(provider.id, { enabled: !provider.enabled });
      await loadMetadataProviders();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to toggle provider');
    }
  };

  const handleClearCache = async () => {
    if (clearingCache) return;
    clearingCache = true;
    try {
      const result = await clearMetadataCache();
      showMessage(`Cleared ${result.deleted} cached entries`);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to clear cache');
    } finally {
      clearingCache = false;
    }
  };

  const handleLibraryCleanup = async () => {
    if (cleaningLibrary) return;
    cleaningLibrary = true;
    try {
      const result = await triggerLibraryCleanup();
      showMessage(`Library cleanup started (job ${result.job_id.slice(0, 8)}…)`);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to start library cleanup');
    } finally {
      cleaningLibrary = false;
    }
  };

  const handleWriteMetadata = async () => {
    if (writingMetadata) return;
    writingMetadata = true;
    try {
      const result = await triggerMetadataWriteAll();
      showMessage(result.message);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to enqueue metadata writes');
    } finally {
      writingMetadata = false;
    }
  };

  const handleSidecarRewrite = async () => {
    if (rewritingSidecars) return;
    rewritingSidecars = true;
    try {
      const result = await triggerSidecarRewrite();
      showMessage(`Sidecar rewrite started (job ${result.job_id.slice(0, 8)}…)`);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to start sidecar rewrite');
    } finally {
      rewritingSidecars = false;
    }
  };
</script>

<svelte:head>
  <title>Metadata Settings — Kobbler</title>
</svelte:head>

{#if !auth.isAdmin}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <div class="metadata-page">
    <h2>Metadata</h2>

    <div class="section">
      <h3>Global Settings</h3>
      {#if systemConfigLoading}
        <div class="loading-card">Loading global settings...</div>
      {:else if systemConfig}
        <div class="provider-row">
          <div class="provider-edit-form">
            <div class="provider-edit-fields">
              <label class="checkbox-label">
                <input
                  type="checkbox"
                  data-field="metadata_providers_enabled"
                  checked={systemConfig.metadata_providers_enabled}
                  onchange={(e) => {
                    if (systemConfig)
                      systemConfig.metadata_providers_enabled = (
                        e.target as HTMLInputElement
                      ).checked;
                  }}
                />
                Enable automatic metadata lookups
              </label>
              <label>
                Cache Duration (days)
                <input
                  type="number"
                  data-field="metadata_cache_days"
                  min="0"
                  value={systemConfig.metadata_cache_days}
                  oninput={(e) => {
                    if (systemConfig)
                      systemConfig.metadata_cache_days =
                        parseInt((e.target as HTMLInputElement).value) || 0;
                  }}
                />
                <span class="field-hint">How long to cache metadata lookups</span>
              </label>
            </div>
            <div class="provider-edit-actions">
              <button
                class="btn small"
                type="button"
                onclick={saveGlobalSettings}
                disabled={systemConfigSaving}
              >
                {systemConfigSaving ? 'Saving...' : 'Save'}
              </button>
            </div>
          </div>
        </div>
      {/if}
    </div>

    <div class="section">
      <h3>Providers</h3>

      {#if loading}
        <div class="loading-card">Loading providers…</div>
      {:else if error}
        <div class="error">{error}</div>
      {:else if metadataProviders.length === 0}
        <div class="empty-state">
          <h4>No metadata providers configured</h4>
          <p class="muted">
            Metadata providers are used to fetch book information during ingestion.
          </p>
        </div>
      {:else}
        <p class="muted" style="margin-bottom: 16px;">
          Configure metadata providers for fetching book information. Lower priority numbers are
          searched first.
        </p>
        <div class="provider-list">
          {#each sortedMetadataProviders as provider (provider.id)}
            <div class="provider-row" class:disabled={!provider.enabled}>
              {#if editingProviderId === provider.id}
                <div class="provider-edit-form">
                  <div class="provider-edit-header">
                    <h4>{provider.display_name}</h4>
                  </div>
                  <div class="provider-edit-fields">
                    <label class="checkbox-label">
                      <input type="checkbox" bind:checked={editingEnabled} />
                      Enabled
                    </label>
                    <label>
                      Priority
                      <input type="number" bind:value={editingPriority} min="1" max="1000" />
                      <span class="field-hint">Lower numbers are searched first</span>
                    </label>
                    {#each provider.config_fields as field (field.key)}
                      <label>
                        {field.label}
                        {#if field.field_type === 'secret'}
                          <input
                            type="password"
                            value={editingConfig[field.key] ?? ''}
                            oninput={(e) =>
                              (editingConfig[field.key] = (e.target as HTMLInputElement).value)}
                            placeholder={field.current_value
                              ? '(configured)'
                              : (field.default ?? 'Not set')}
                          />
                        {:else if field.field_type === 'boolean'}
                          <input
                            type="checkbox"
                            checked={!!editingConfig[field.key]}
                            onchange={(e) =>
                              (editingConfig[field.key] = (e.target as HTMLInputElement).checked)}
                          />
                        {:else if field.field_type === 'integer'}
                          <input
                            type="number"
                            value={editingConfig[field.key] ?? ''}
                            oninput={(e) =>
                              (editingConfig[field.key] =
                                parseInt((e.target as HTMLInputElement).value) || '')}
                          />
                        {:else}
                          <input
                            type="text"
                            value={editingConfig[field.key] ?? ''}
                            oninput={(e) =>
                              (editingConfig[field.key] = (e.target as HTMLInputElement).value)}
                            placeholder={field.default ?? ''}
                          />
                        {/if}
                        {#if field.description}
                          <span class="field-hint">{field.description}</span>
                        {/if}
                        {#if field.help_url}
                          <a href={field.help_url} target="_blank" class="field-help-link"
                            >Get value</a
                          >
                        {/if}
                      </label>
                    {/each}
                  </div>
                  <div class="provider-edit-actions">
                    <button
                      class="btn small"
                      type="button"
                      onclick={() => saveProvider(provider.id)}
                    >
                      Save
                    </button>
                    <button class="btn alt small" type="button" onclick={cancelEditProvider}>
                      Cancel
                    </button>
                  </div>
                </div>
              {:else}
                <div class="provider-info">
                  <div class="provider-header">
                    <span class="provider-name">{provider.display_name}</span>
                    <span class="provider-priority muted">Priority: {provider.priority}</span>
                  </div>
                  <div class="provider-description muted">
                    {provider.description}
                  </div>
                  <div class="provider-capabilities">
                    {#each provider.capabilities as cap (cap)}
                      <span class="capability-badge">{formatCapability(cap)}</span>
                    {/each}
                  </div>
                  {#each provider.config_fields as field (field.key)}
                    {#if field.current_value}
                      <div class="provider-config-status muted">
                        {field.label} configured
                      </div>
                    {/if}
                  {/each}
                  <a href={provider.website} target="_blank" class="provider-website muted">
                    {provider.website}
                  </a>
                </div>
                <div class="provider-actions">
                  <button
                    class="btn alt small"
                    type="button"
                    onclick={() => toggleProviderEnabled(provider)}
                  >
                    {provider.enabled ? 'Disable' : 'Enable'}
                  </button>
                  <button
                    class="btn alt small"
                    type="button"
                    onclick={() => startEditProvider(provider)}
                  >
                    Edit
                  </button>
                </div>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    </div>

    <div class="section">
      <h3>Video Metadata</h3>

      {#if videoLoading}
        <div class="loading-card">Loading video providers...</div>
      {:else if videoError}
        <div class="error">{videoError}</div>
      {:else if videoProviders.length === 0}
        <div class="empty-state">
          <h4>No video metadata providers configured</h4>
          <p class="muted">
            Video metadata providers are used to fetch movie and TV show information.
          </p>
        </div>
      {:else}
        <p class="muted" style="margin-bottom: 16px;">
          Configure video metadata providers for fetching movie and TV show information. Lower
          priority numbers are searched first.
        </p>
        <div class="provider-list">
          {#each sortedVideoProviders as provider (provider.id)}
            <div class="provider-row" class:disabled={!provider.enabled}>
              {#if editingVideoProviderId === provider.id}
                <div class="provider-edit-form">
                  <div class="provider-edit-header">
                    <h4>{provider.display_name}</h4>
                  </div>
                  <div class="provider-edit-fields">
                    <label class="checkbox-label">
                      <input type="checkbox" bind:checked={editingVideoEnabled} />
                      Enabled
                    </label>
                    <label>
                      Priority
                      <input type="number" bind:value={editingVideoPriority} min="1" max="1000" />
                      <span class="field-hint">Lower numbers are searched first</span>
                    </label>
                    {#each provider.config_fields as field (field.key)}
                      <label>
                        {field.label}
                        {#if field.field_type === 'secret'}
                          <input
                            type="password"
                            value={editingVideoConfig[field.key] ?? ''}
                            oninput={(e) =>
                              (editingVideoConfig[field.key] = (
                                e.target as HTMLInputElement
                              ).value)}
                            placeholder={field.current_value
                              ? '(configured)'
                              : (field.default ?? 'Not set')}
                          />
                        {:else if field.field_type === 'boolean'}
                          <input
                            type="checkbox"
                            checked={!!editingVideoConfig[field.key]}
                            onchange={(e) =>
                              (editingVideoConfig[field.key] = (
                                e.target as HTMLInputElement
                              ).checked)}
                          />
                        {:else if field.field_type === 'integer'}
                          <input
                            type="number"
                            value={editingVideoConfig[field.key] ?? ''}
                            oninput={(e) =>
                              (editingVideoConfig[field.key] =
                                parseInt((e.target as HTMLInputElement).value) || '')}
                          />
                        {:else}
                          <input
                            type="text"
                            value={editingVideoConfig[field.key] ?? ''}
                            oninput={(e) =>
                              (editingVideoConfig[field.key] = (
                                e.target as HTMLInputElement
                              ).value)}
                            placeholder={field.default ?? ''}
                          />
                        {/if}
                        {#if field.description}
                          <span class="field-hint">{field.description}</span>
                        {/if}
                        {#if field.help_url}
                          <a href={field.help_url} target="_blank" class="field-help-link"
                            >Get value</a
                          >
                        {/if}
                      </label>
                    {/each}
                  </div>
                  <div class="provider-edit-actions">
                    <button
                      class="btn small"
                      type="button"
                      onclick={() => saveVideoProvider(provider.id)}
                    >
                      Save
                    </button>
                    <button class="btn alt small" type="button" onclick={cancelEditVideoProvider}>
                      Cancel
                    </button>
                  </div>
                </div>
              {:else}
                <div class="provider-info">
                  <div class="provider-header">
                    <span class="provider-name">{provider.display_name}</span>
                    <span class="provider-priority muted">Priority: {provider.priority}</span>
                  </div>
                  <div class="provider-description muted">
                    {provider.description}
                  </div>
                  <div class="provider-capabilities">
                    {#each provider.capabilities as cap (cap)}
                      <span class="capability-badge">{formatCapability(cap)}</span>
                    {/each}
                  </div>
                  {#each provider.config_fields as field (field.key)}
                    {#if field.current_value}
                      <div class="provider-config-status muted">
                        {field.label} configured
                      </div>
                    {/if}
                  {/each}
                  <a href={provider.website} target="_blank" class="provider-website muted">
                    {provider.website}
                  </a>
                </div>
                <div class="provider-actions">
                  <button
                    class="btn alt small"
                    type="button"
                    onclick={() => toggleVideoProviderEnabled(provider)}
                  >
                    {provider.enabled ? 'Disable' : 'Enable'}
                  </button>
                  <button
                    class="btn alt small"
                    type="button"
                    onclick={() => startEditVideoProvider(provider)}
                  >
                    Edit
                  </button>
                </div>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    </div>

    <div class="section">
      <h3>Tools</h3>
      <div class="tools-list">
        <div class="tool-item">
          <div class="tool-info">
            <span class="tool-name">Clear Cache</span>
            <span class="tool-description muted"
              >Clears the metadata cache to purge stale or incorrect cached metadata from providers.</span
            >
          </div>
          <button class="btn alt small" onclick={handleClearCache} disabled={clearingCache}>
            {clearingCache ? 'Clearing...' : 'Clear Cache'}
          </button>
        </div>
        <div class="tool-item">
          <div class="tool-info">
            <span class="tool-name">Clean Up Library</span>
            <span class="tool-description muted"
              >Removes orphaned records (books, movies, episodes without files or active wants),
              their orphaned parents (authors, series, genres, people), and cleans up orphaned files
              from storage. Runs as a background job.</span
            >
          </div>
          <button class="btn alt small" onclick={handleLibraryCleanup} disabled={cleaningLibrary}>
            {cleaningLibrary ? 'Starting...' : 'Clean Up Library'}
          </button>
        </div>
        <div class="tool-item">
          <div class="tool-info">
            <span class="tool-name">Write Metadata to Files</span>
            <span class="tool-description muted"
              >Writes current library metadata (title, authors, series, etc.) to all ebook files.
              Use after fixing metadata issues to ensure files match the library database.</span
            >
          </div>
          <button class="btn alt small" onclick={handleWriteMetadata} disabled={writingMetadata}>
            {writingMetadata ? 'Enqueuing...' : 'Write to All Files'}
          </button>
        </div>
        <div class="tool-item">
          <div class="tool-info">
            <span class="tool-name">Rewrite All Sidecars</span>
            <span class="tool-description muted"
              >Regenerates all sidecar files (NFO, metadata.json, metadata.opf) across the library.
              TV shows are re-enriched first to populate episode-level identifiers. Use after fixing
              sidecar bugs or changing sidecar settings.</span
            >
          </div>
          <button class="btn alt small" onclick={handleSidecarRewrite} disabled={rewritingSidecars}>
            {rewritingSidecars ? 'Starting...' : 'Rewrite Sidecars'}
          </button>
        </div>
      </div>
    </div>
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

  .metadata-page {
    display: grid;
    gap: 24px;
  }

  .metadata-page > h2 {
    margin: 0;
    font-size: 20px;
  }

  .section {
    display: grid;
    gap: 16px;
  }

  .section h3 {
    margin: 0;
    font-size: 16px;
  }

  .tools-list {
    display: grid;
    gap: 12px;
  }

  .tool-item {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 14px;
    padding: 16px;
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 16px;
  }

  .tool-info {
    display: grid;
    gap: 4px;
    flex: 1;
  }

  .tool-name {
    font-weight: 600;
    font-size: 14px;
  }

  .tool-description {
    font-size: 13px;
  }

  .provider-list {
    display: grid;
    gap: 12px;
  }

  .provider-row {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 14px;
    padding: 16px;
    display: flex;
    justify-content: space-between;
    gap: 16px;
    transition: opacity 0.15s;
  }

  .provider-row.disabled {
    opacity: 0.6;
  }

  .provider-info {
    flex: 1;
    display: grid;
    gap: 6px;
  }

  .provider-header {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .provider-name {
    font-weight: 600;
    font-size: 15px;
  }

  .provider-priority {
    font-size: 12px;
  }

  .provider-description {
    font-size: 13px;
  }

  .provider-capabilities {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 4px;
  }

  .capability-badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 6px;
    background: var(--ring);
  }

  .provider-config-status {
    font-size: 11px;
    color: var(--accent);
  }

  .provider-website {
    font-size: 11px;
    text-decoration: none;
  }

  .provider-website:hover {
    text-decoration: underline;
  }

  .provider-actions {
    display: flex;
    flex-direction: column;
    gap: 6px;
    flex-shrink: 0;
  }

  .provider-edit-form {
    width: 100%;
    display: grid;
    gap: 12px;
  }

  .provider-edit-header h4 {
    margin: 0;
    font-size: 16px;
  }

  .provider-edit-fields {
    display: grid;
    gap: 12px;
  }

  .provider-edit-fields label {
    display: grid;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .provider-edit-fields input[type='text'],
  .provider-edit-fields input[type='password'],
  .provider-edit-fields input[type='number'] {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .provider-edit-actions {
    display: flex;
    gap: 8px;
  }

  .field-hint {
    font-size: 11px;
    color: var(--muted);
  }

  .field-help-link {
    font-size: 11px;
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

  .btn.alt {
    background: var(--card);
    border: 1px solid var(--ring);
    color: var(--ink);
  }

  .btn.small {
    padding: 6px 10px;
    font-size: 12px;
  }

  .loading-card {
    padding: 24px;
    text-align: center;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
  }

  .empty-state {
    padding: 32px;
    text-align: center;
    background: var(--card);
    border: 1px dashed var(--ring);
    border-radius: 16px;
  }

  .empty-state h4 {
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

  @media (max-width: 768px) {
    .provider-row {
      flex-direction: column;
    }

    .provider-actions {
      flex-direction: row;
    }

    .tool-item {
      flex-direction: column;
      align-items: stretch;
    }
  }
</style>
