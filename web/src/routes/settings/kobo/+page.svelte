<script lang="ts">
  import { onMount } from 'svelte';
  import { SearchSelect } from '@koblibs/ui';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { getAuthState } from '#lib/stores/auth.svelte.js';
  import {
    getSystemConfig,
    updateSystemConfig,
    type SystemConfig,
    type SystemConfigUpdate
  } from '#lib/api/client.js';

  let systemConfig = $state<SystemConfig | null>(null);
  let systemConfigLoading = $state(true);
  let systemConfigError = $state<string | null>(null);
  let systemConfigSaving = $state(false);

  const auth = getAuthState();

  const koboModeOptions = [
    { value: 'merge', label: 'Merge (local + upstream)' },
    { value: 'passthrough', label: 'Passthrough (upstream only)' },
    { value: 'local', label: 'Local (no upstream)' }
  ];

  const entitlementModeOptions = [
    { value: 'all_devices', label: 'All Devices' },
    { value: 'none', label: 'None' }
  ];

  onMount(async () => {
    await loadSystemConfig();
  });

  const loadSystemConfig = async () => {
    systemConfigLoading = true;
    systemConfigError = null;
    try {
      systemConfig = await getSystemConfig();
    } catch (e) {
      systemConfigError = e instanceof Error ? e.message : 'Failed to load config';
    } finally {
      systemConfigLoading = false;
    }
  };

  const saveSystemConfig = async () => {
    if (!systemConfig) return;
    systemConfigSaving = true;
    systemConfigError = null;
    try {
      const updates: SystemConfigUpdate = {
        public_proxy_url: systemConfig.public_proxy_url,
        kobo_storeapi_base_url: systemConfig.kobo_storeapi_base_url,
        kobo_mode: systemConfig.kobo_mode,
        proxy_logging_enabled: systemConfig.proxy_logging_enabled,
        proxy_log_retention_days: systemConfig.proxy_log_retention_days,
        blob_capture_enabled: systemConfig.blob_capture_enabled,
        rewrite_kobo_urls: systemConfig.rewrite_kobo_urls,
        kobo_sync_batch_size: systemConfig.kobo_sync_batch_size,
        default_entitlement_mode: systemConfig.default_entitlement_mode
      };
      systemConfig = await updateSystemConfig(updates);
      showMessage('Configuration saved');
    } catch (e) {
      systemConfigError = e instanceof Error ? e.message : 'Failed to save config';
    } finally {
      systemConfigSaving = false;
    }
  };
</script>

<svelte:head>
  <title>Kobo Proxy Settings — Kobbler</title>
</svelte:head>

{#if !auth.isAdmin}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <div class="kobo-section">
    <div class="section-header">
      <h2>Kobo Proxy</h2>
    </div>

    {#if systemConfigLoading}
      <div class="loading-card">Loading configuration...</div>
    {:else if systemConfigError}
      <div class="error">{systemConfigError}</div>
    {:else if systemConfig}
      <div class="config-form">
        <div class="config-card">
          <h3>URLs</h3>
          <div data-field="public_proxy_url">
            <label>
              Public Proxy URL
              <input
                type="text"
                bind:value={systemConfig.public_proxy_url}
                placeholder="https://kobbler.example.com/kobo"
              />
              <span class="field-hint"
                >URL for Kobo device sync (include mount point like /kobo)</span
              >
            </label>
          </div>
          <div data-field="kobo_storeapi_base_url">
            <label>
              Kobo Store API URL
              <input
                type="text"
                bind:value={systemConfig.kobo_storeapi_base_url}
                placeholder="https://storeapi.kobo.com"
              />
              <span class="field-hint">Default upstream Kobo API (devices can override)</span>
            </label>
          </div>
        </div>

        <div class="config-card">
          <h3>Proxy</h3>
          <div data-field="kobo_mode">
            <div class="field">
              <span class="field-label">Kobo Mode</span>
              <SearchSelect items={koboModeOptions} bind:value={systemConfig.kobo_mode} />
              <span class="field-hint">How to handle Kobo sync requests</span>
            </div>
          </div>
          <div data-field="proxy_logging_enabled">
            <label class="checkbox-label">
              <input type="checkbox" bind:checked={systemConfig.proxy_logging_enabled} />
              Enable proxy logging
            </label>
          </div>
          <div data-field="blob_capture_enabled">
            <label class="checkbox-label">
              <input type="checkbox" bind:checked={systemConfig.blob_capture_enabled} />
              Enable blob capture (stores full request/response bodies)
            </label>
          </div>
          <div data-field="rewrite_kobo_urls">
            <label class="checkbox-label">
              <input type="checkbox" bind:checked={systemConfig.rewrite_kobo_urls} />
              Rewrite Kobo URLs (route all Kobo traffic through proxy for debugging)
            </label>
          </div>
          <div data-field="proxy_log_retention_days">
            <label>
              Log Retention Days
              <input
                type="number"
                bind:value={systemConfig.proxy_log_retention_days}
                min="1"
                max="365"
              />
            </label>
          </div>
        </div>

        <div class="config-card">
          <h3>Sync</h3>
          <div data-field="kobo_sync_batch_size">
            <label>
              Sync Batch Size
              <input
                type="number"
                bind:value={systemConfig.kobo_sync_batch_size}
                min="10"
                max="1000"
              />
              <span class="field-hint">Max books per sync response</span>
            </label>
          </div>
          <div data-field="default_entitlement_mode">
            <div class="field">
              <span class="field-label">Default Entitlement Mode</span>
              <SearchSelect
                items={entitlementModeOptions}
                bind:value={systemConfig.default_entitlement_mode}
              />
              <span class="field-hint">Default ebook access for new Kobo books</span>
            </div>
          </div>
        </div>

        <div class="config-actions">
          <button
            class="btn"
            type="button"
            onclick={saveSystemConfig}
            disabled={systemConfigSaving}
          >
            {systemConfigSaving ? 'Saving...' : 'Save Configuration'}
          </button>
          <button
            class="btn alt"
            type="button"
            onclick={loadSystemConfig}
            disabled={systemConfigLoading}
          >
            Reset
          </button>
        </div>
      </div>
    {:else}
      <div class="muted">No configuration loaded</div>
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

  .kobo-section {
    display: grid;
    gap: 16px;
  }

  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .section-header h2 {
    margin: 0;
    font-size: 18px;
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

  .config-card label:not(.checkbox-label) {
    display: grid;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .config-card input[type='text'],
  .config-card input[type='number'] {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .field {
    display: grid;
    gap: 6px;
  }

  .field-label {
    font-size: 12px;
    color: var(--muted);
  }

  .field-hint {
    font-size: 11px;
    color: var(--muted);
  }

  .config-actions {
    display: flex;
    gap: 12px;
    padding-top: 8px;
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
