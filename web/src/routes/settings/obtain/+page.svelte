<script lang="ts">
  import { onMount } from 'svelte';
  import { SearchSelect } from '@koblibs/ui';
  import { showMessage } from '$lib/context/app.svelte';
  import { getAuthState } from '$lib/stores/auth.svelte';
  import {
    getSystemConfig,
    updateSystemConfig,
    testKobloadConnection,
    type SystemConfig,
    type SystemConfigUpdate,
    type TestConnectionResult
  } from '$lib/api/client';

  const auth = getAuthState();

  // Local state
  let systemConfig = $state<SystemConfig | null>(null);
  let systemConfigLoading = $state(true);
  let systemConfigError = $state<string | null>(null);
  let systemConfigSaving = $state(false);

  // Kobload state
  let kobloadTesting = $state(false);
  let kobloadTestResult = $state<TestConnectionResult | null>(null);

  const metadataProviderOptions = [
    { value: '', label: 'No default' },
    { value: 'hardcover', label: 'Hardcover' },
    { value: 'googlebooks', label: 'Google Books' },
    { value: 'openlibrary', label: 'Open Library' }
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

  const friendlyKobloadError = (message: string): string => {
    if (message.includes('502') || message.includes('Bad Gateway')) {
      return 'Cannot reach Kobload. Check the Base URL and ensure Kobload is running.';
    }
    if (message.includes('401') || message.includes('Unauthorized')) {
      return 'Invalid API key. Generate a new one in Kobload Settings > API Keys.';
    }
    if (message.includes('connect') || message.includes('ECONNREFUSED')) {
      return 'Cannot connect to Kobload. Check the Base URL.';
    }
    return message;
  };

  const saveSystemConfig = async () => {
    if (!systemConfig) return;
    systemConfigSaving = true;
    systemConfigError = null;
    try {
      const updates: SystemConfigUpdate = {
        wanted_default_provider: systemConfig.wanted_default_provider ?? '',
        wanted_min_score: systemConfig.wanted_min_score,
        kobload_base_url: systemConfig.kobload_base_url ?? '',
        kobload_api_key: systemConfig.kobload_api_key ?? '',
        kobload_ingest_url: systemConfig.kobload_ingest_url ?? '',
        kobload_communication_mode: systemConfig.kobload_communication_mode
      };
      systemConfig = await updateSystemConfig(updates);
      showMessage('Settings saved');
    } catch (e) {
      systemConfigError = e instanceof Error ? e.message : 'Failed to save config';
    } finally {
      systemConfigSaving = false;
    }
  };

  const handleTestKobload = async () => {
    kobloadTesting = true;
    kobloadTestResult = null;
    try {
      const result = await testKobloadConnection({
        kobload_base_url: systemConfig?.kobload_base_url ?? undefined,
        kobload_api_key: systemConfig?.kobload_api_key ?? undefined
      });
      kobloadTestResult = {
        success: result.success,
        message: result.success ? result.message : friendlyKobloadError(result.message)
      };
    } catch (e) {
      const raw = e instanceof Error ? e.message : 'Connection test failed';
      kobloadTestResult = {
        success: false,
        message: friendlyKobloadError(raw)
      };
    } finally {
      kobloadTesting = false;
    }
  };
</script>

<svelte:head>
  <title>Obtain Settings — Kobbler</title>
</svelte:head>

{#if !auth.isAdmin}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <div class="obtain-section">
    <div class="section-header">
      <h2>Obtain Settings</h2>
    </div>

    {#if systemConfigLoading}
      <div class="loading-card">Loading configuration...</div>
    {:else if systemConfigError}
      <div class="error">{systemConfigError}</div>
    {:else if systemConfig}
      <div class="config-form">
        <div class="config-card">
          <h3>Kobload Connection</h3>
          <p class="card-description muted">Connect to Kobload for automated book downloads.</p>
          <label>
            Base URL
            <input
              type="text"
              data-field="kobload_base_url"
              value={systemConfig.kobload_base_url ?? ''}
              oninput={(e) => {
                if (systemConfig)
                  systemConfig.kobload_base_url = (e.target as HTMLInputElement).value || null;
              }}
              placeholder="http://localhost:7080"
            />
            <span class="field-hint">Kobload API URL</span>
          </label>
          <label>
            API Key
            <input
              type="password"
              data-field="kobload_api_key"
              value={systemConfig.kobload_api_key ?? ''}
              oninput={(e) => {
                if (systemConfig)
                  systemConfig.kobload_api_key = (e.target as HTMLInputElement).value || null;
              }}
              placeholder="Kobload API key"
            />
            <span class="field-hint">Generate in Kobload Settings > API Keys</span>
          </label>
          <label>
            Communication Mode
            <select
              data-field="kobload_communication_mode"
              value={systemConfig.kobload_communication_mode ?? 'nats'}
              onchange={(e) => {
                if (systemConfig)
                  systemConfig.kobload_communication_mode =
                    (e.target as HTMLSelectElement).value || null;
              }}
            >
              <option value="nats">NATS (default)</option>
              <option value="http">HTTP</option>
            </select>
            <span class="field-hint">Protocol for Kobbler→Kobload communication</span>
          </label>
          <div class="test-section">
            <button
              class="btn alt"
              type="button"
              onclick={handleTestKobload}
              disabled={kobloadTesting || !systemConfig.kobload_base_url}
            >
              {kobloadTesting ? 'Testing...' : 'Test Connection'}
            </button>
            {#if kobloadTestResult}
              <div class="test-result" class:success={kobloadTestResult.success}>
                {kobloadTestResult.message}
              </div>
            {/if}
          </div>
        </div>

        <div class="config-card">
          <h3>Ingestion</h3>
          <p class="card-description muted">Configure where Kobload sends completed downloads.</p>
          <label>
            Ingest URL
            <input
              type="text"
              data-field="kobload_ingest_url"
              value={systemConfig.kobload_ingest_url ?? ''}
              oninput={(e) => {
                if (systemConfig)
                  systemConfig.kobload_ingest_url = (e.target as HTMLInputElement).value || null;
              }}
              placeholder="http://kobbler:6766/api/v1/ingest/your-token"
            />
            <span class="field-hint">Where Kobload sends completed downloads</span>
          </label>
        </div>

        <div class="config-card">
          <h3>Wanted Defaults</h3>
          <p class="card-description muted">
            Default settings for searching and downloading books.
          </p>
          <div class="field" data-field="wanted_default_provider">
            <span class="field-label">Default Metadata Provider</span>
            <SearchSelect
              items={metadataProviderOptions}
              value={systemConfig.wanted_default_provider ?? ''}
              onchange={(v) => {
                if (systemConfig) {
                  systemConfig.wanted_default_provider = v || null;
                }
              }}
            />
            <span class="field-hint">Provider used to fetch book metadata when searching</span>
          </div>
          <label>
            Minimum Match Score
            <input
              type="number"
              data-field="wanted_min_score"
              min="0"
              max="1"
              step="0.05"
              value={systemConfig.wanted_min_score ?? 0.65}
              oninput={(e) => {
                if (systemConfig)
                  systemConfig.wanted_min_score =
                    parseFloat((e.target as HTMLInputElement).value) || null;
              }}
            />
            <span class="field-hint"
              >Fuzzy matching threshold for wanted items (0.0-1.0, default 0.65)</span
            >
          </label>
        </div>

        <div class="config-actions">
          <button
            class="btn"
            type="button"
            onclick={saveSystemConfig}
            disabled={systemConfigSaving}
          >
            {systemConfigSaving ? 'Saving...' : 'Save Settings'}
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

  .obtain-section {
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

  .card-description {
    font-size: 13px;
    margin: 0 0 8px;
  }

  .config-card label {
    display: grid;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .config-card input[type='text'],
  .config-card input[type='password'],
  .config-card input[type='number'],
  .config-card select {
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

  .test-section {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }

  .test-result {
    font-size: 13px;
    padding: 8px 12px;
    border-radius: 8px;
    background: var(--danger);
    color: white;
  }

  .test-result.success {
    background: var(--accent-2);
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
