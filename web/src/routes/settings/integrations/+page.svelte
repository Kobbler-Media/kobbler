<script lang="ts">
  import { onMount } from 'svelte';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { getAuthState } from '#lib/stores/auth.svelte.js';
  import {
    getSyncProviders,
    updateSyncProvider,
    type SyncProviderSettings
  } from '#lib/api/client.js';
  import TraktSection from '#lib/components/TraktSection.svelte';

  const PROVIDER_INFO: Record<string, { name: string; website: string; capabilities: string[] }> = {
    hardcover: {
      name: 'Hardcover',
      website: 'hardcover.app',
      capabilities: ['Mark Reading', 'Mark Finished', 'Progress', 'Reading Time', 'Ratings']
    }
  };

  function getProviderInfo(providerType: string) {
    return (
      PROVIDER_INFO[providerType] ?? {
        name: providerType.charAt(0).toUpperCase() + providerType.slice(1),
        website: '',
        capabilities: []
      }
    );
  }

  const auth = getAuthState();

  let providers = $state<SyncProviderSettings[]>([]);
  let loading = $state(true);
  let saving = $state(false);
  let error = $state<string | null>(null);

  // Per-provider edit state
  let editingProvider = $state<string | null>(null);
  let editEnabled = $state(false);
  let editApiKey = $state('');

  onMount(async () => {
    await loadSettings();
  });

  const loadSettings = async () => {
    loading = true;
    error = null;
    try {
      providers = await getSyncProviders();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load settings';
    } finally {
      loading = false;
    }
  };

  const startEditing = (providerType: string) => {
    const provider = providers.find((p) => p.provider_type === providerType);
    if (!provider) return;
    editEnabled = provider.enabled;
    editApiKey = '';
    editingProvider = providerType;
  };

  const cancelEditing = () => {
    editingProvider = null;
    editApiKey = '';
  };

  const saveSettings = async (providerType: string) => {
    saving = true;
    try {
      await updateSyncProvider(providerType, {
        enabled: editEnabled,
        api_key: editApiKey || undefined
      });
      await loadSettings();
      editingProvider = null;
      editApiKey = '';
      const info = getProviderInfo(providerType);
      showMessage(`${info.name} settings updated`);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to save settings');
    } finally {
      saving = false;
    }
  };

  const toggleEnabled = async (providerType: string) => {
    const provider = providers.find((p) => p.provider_type === providerType);
    if (!provider) return;
    saving = true;
    try {
      await updateSyncProvider(providerType, { enabled: !provider.enabled });
      await loadSettings();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update settings');
    } finally {
      saving = false;
    }
  };

  const clearApiKey = async (providerType: string) => {
    const info = getProviderInfo(providerType);
    if (!confirm(`Remove your ${info.name} API key? This will disable syncing.`)) return;
    saving = true;
    try {
      await updateSyncProvider(providerType, { api_key: '' });
      await loadSettings();
      showMessage('API key removed');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to remove API key');
    } finally {
      saving = false;
    }
  };
</script>

<svelte:head>
  <title>Integrations — Kobbler</title>
</svelte:head>

{#if !auth.isAdmin}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <div class="integrations-section">
    <div class="section-header">
      <h2>Reading Trackers</h2>
    </div>

    {#if loading}
      <div class="loading-card">Loading settings...</div>
    {:else if error}
      <div class="error">{error}</div>
    {:else if providers.length > 0}
      <p class="muted description">
        Connect your reading tracker accounts to sync reading status, progress, and ratings from
        your devices. When enabled, your reading activity will be automatically pushed to the
        provider.
      </p>

      {#each providers as provider (provider.provider_type)}
        {@const info = getProviderInfo(provider.provider_type)}
        <div class="provider-card" class:disabled={!provider.enabled}>
          {#if editingProvider === provider.provider_type}
            <div class="provider-edit-form">
              <div class="provider-edit-header">
                <div class="provider-logo">
                  <svg viewBox="0 0 24 24" width="24" height="24" fill="currentColor">
                    <path
                      d="M18 2H6c-1.1 0-2 .9-2 2v16c0 1.1.9 2 2 2h12c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zM6 4h5v8l-2.5-1.5L6 12V4z"
                    ></path>
                  </svg>
                </div>
                <h4>{info.name}</h4>
              </div>

              <div class="provider-edit-fields">
                <label class="checkbox-label">
                  <input type="checkbox" bind:checked={editEnabled} />
                  Enable {info.name} sync
                </label>

                <label>
                  API Key
                  <input
                    type="password"
                    bind:value={editApiKey}
                    placeholder={provider.has_api_key
                      ? '(configured - enter new to change)'
                      : 'Paste your API key'}
                  />
                  {#if info.website}
                    <span class="field-hint">
                      Get your API key from
                      <a href="https://{info.website}/account/api" target="_blank" rel="noopener"
                        >{info.website}/account/api</a
                      >
                    </span>
                  {/if}
                </label>
              </div>

              <div class="provider-edit-actions">
                <button
                  class="btn small"
                  type="button"
                  onclick={() => saveSettings(provider.provider_type)}
                  disabled={saving}
                >
                  {saving ? 'Saving...' : 'Save'}
                </button>
                <button
                  class="btn alt small"
                  type="button"
                  onclick={cancelEditing}
                  disabled={saving}
                >
                  Cancel
                </button>
              </div>
            </div>
          {:else}
            <div class="provider-row">
              <div class="provider-info">
                <div class="provider-header">
                  <div class="provider-logo">
                    <svg viewBox="0 0 24 24" width="24" height="24" fill="currentColor">
                      <path
                        d="M18 2H6c-1.1 0-2 .9-2 2v16c0 1.1.9 2 2 2h12c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zM6 4h5v8l-2.5-1.5L6 12V4z"
                      ></path>
                    </svg>
                  </div>
                  <span class="provider-name">{info.name}</span>
                  {#if provider.has_api_key}
                    <span class="config-badge">API Key Set</span>
                  {/if}
                  <span class="status-badge" class:enabled={provider.enabled}>
                    {provider.enabled ? 'Enabled' : 'Disabled'}
                  </span>
                </div>

                <p class="provider-description muted">
                  Track your reading on {info.name}. Syncs status, progress, and ratings.
                </p>

                {#if info.capabilities.length > 0}
                  <div class="provider-capabilities">
                    {#each info.capabilities as capability (capability)}
                      <span class="capability-badge">{capability}</span>
                    {/each}
                  </div>
                {/if}

                {#if info.website}
                  <a href="https://{info.website}" target="_blank" class="provider-website muted">
                    {info.website}
                  </a>
                {/if}
              </div>

              <div class="provider-actions">
                <button
                  class="btn alt small"
                  type="button"
                  onclick={() => toggleEnabled(provider.provider_type)}
                  disabled={saving || !provider.has_api_key}
                  title={!provider.has_api_key ? 'Add API key to enable' : ''}
                >
                  {provider.enabled ? 'Disable' : 'Enable'}
                </button>
                <button
                  class="btn alt small"
                  type="button"
                  onclick={() => startEditing(provider.provider_type)}
                >
                  {provider.has_api_key ? 'Edit' : 'Configure'}
                </button>
                {#if provider.has_api_key}
                  <button
                    class="btn alt small danger"
                    type="button"
                    onclick={() => clearApiKey(provider.provider_type)}
                    disabled={saving}
                  >
                    Remove Key
                  </button>
                {/if}
              </div>
            </div>
          {/if}
        </div>

        {#if provider.has_api_key && provider.enabled}
          <div class="sync-info">
            <span class="material-icons-outlined">check_circle</span>
            <span>Reading activity will sync to {info.name} automatically.</span>
          </div>
        {:else if provider.has_api_key && !provider.enabled}
          <div class="sync-info warning">
            <span class="material-icons-outlined">pause_circle</span>
            <span>Sync paused. Enable to resume syncing to {info.name}.</span>
          </div>
        {:else}
          <div class="sync-info muted">
            <span class="material-icons-outlined">info</span>
            <span>Add your {info.name} API key to start syncing reading activity.</span>
          </div>
        {/if}
      {/each}
    {:else}
      <p class="muted description">No reading tracker providers available.</p>
    {/if}

    <TraktSection />
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

  .integrations-section {
    display: grid;
    gap: 16px;
  }

  .section-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .description {
    margin: 0;
    font-size: 13px;
    line-height: 1.5;
  }

  .provider-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 14px;
    padding: 16px;
    transition: opacity 0.15s;
  }

  .provider-card.disabled {
    opacity: 0.7;
  }

  .provider-row {
    display: flex;
    justify-content: space-between;
    gap: 16px;
  }

  .provider-info {
    flex: 1;
    display: grid;
    gap: 8px;
  }

  .provider-header {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .provider-logo {
    width: 32px;
    height: 32px;
    background: var(--ring);
    border-radius: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--accent);
  }

  .provider-name {
    font-weight: 600;
    font-size: 15px;
  }

  .status-badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 6px;
    background: var(--ring);
    color: var(--muted);
  }

  .status-badge.enabled {
    background: var(--accent);
    color: white;
  }

  .config-badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 6px;
    background: #22c55e;
    color: white;
  }

  .provider-capabilities {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .capability-badge {
    font-size: 10px;
    padding: 2px 6px;
    border-radius: 4px;
    background: var(--ring);
    color: var(--muted);
  }

  .provider-description {
    font-size: 13px;
    margin: 0;
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

  /* Edit form */
  .provider-edit-form {
    display: grid;
    gap: 16px;
  }

  .provider-edit-header {
    display: flex;
    align-items: center;
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

  .provider-edit-fields input[type='password'] {
    padding: 10px 12px;
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

  .checkbox-label {
    display: flex !important;
    flex-direction: row !important;
    align-items: center;
    gap: 8px !important;
    cursor: pointer;
    color: var(--ink);
    font-size: 13px !important;
  }

  .checkbox-label input[type='checkbox'] {
    width: 16px;
    height: 16px;
    cursor: pointer;
  }

  .field-hint {
    font-size: 11px;
    color: var(--muted);
  }

  .field-hint a {
    color: var(--accent);
  }

  /* Buttons */
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

  .btn.danger {
    color: var(--danger, #ef4444);
  }

  /* Status messages */
  .sync-info {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 16px;
    background: rgba(34, 197, 94, 0.1);
    border: 1px solid rgba(34, 197, 94, 0.3);
    border-radius: 10px;
    font-size: 13px;
    color: #22c55e;
  }

  .sync-info.warning {
    background: rgba(245, 158, 11, 0.1);
    border-color: rgba(245, 158, 11, 0.3);
    color: #f59e0b;
  }

  .sync-info.muted {
    background: var(--ring);
    border-color: var(--ring);
    color: var(--muted);
  }

  .sync-info .material-icons-outlined {
    font-size: 18px;
  }

  /* Utilities */
  .loading-card {
    padding: 24px;
    text-align: center;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    color: var(--muted);
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
  }
</style>
