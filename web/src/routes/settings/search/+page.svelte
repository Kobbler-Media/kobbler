<script lang="ts">
  import { onMount } from 'svelte';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { getAuthState } from '#lib/stores/auth.svelte.js';
  import {
    getSearchStatus,
    testSearchConnection,
    triggerReindex,
    reindexTranscripts,
    getSystemConfig,
    updateSystemConfig,
    type SearchBackendStatus,
    type SystemConfig,
    type SystemConfigUpdate
  } from '#lib/api/client.js';

  const auth = getAuthState();

  let status = $state<SearchBackendStatus | null>(null);
  let statusLoading = $state(true);
  let statusError = $state<string | null>(null);

  // System config state (for saving)
  let systemConfig = $state<SystemConfig | null>(null);
  let systemConfigLoading = $state(true);
  let systemConfigError = $state<string | null>(null);
  let systemConfigSaving = $state(false);

  // When a koblem service URL is set, the backend uses koblem as the chat LLM
  // and ignores the external provider fields (see chatbot::llm::resolve).
  const koblemActive = $derived(!!systemConfig?.koblem_service_url?.trim());
  // Semantic search embeds through the embedding service (Settings → Processing),
  // or koblem when GPU calls are routed there.
  const embedderActive = $derived(
    !!systemConfig?.vision_service_url?.trim() ||
      (!!systemConfig?.route_gpu_via_koblem && koblemActive)
  );

  // Test connection state (uses current form values, not saved values)
  let testing = $state(false);
  let testResult = $state<{ success: boolean; message: string; version?: string | null } | null>(
    null
  );

  // Reindex state
  let reindexing = $state(false);
  let reindexingTranscripts = $state(false);

  onMount(async () => {
    await Promise.all([loadStatus(), loadSystemConfig()]);
  });

  const loadStatus = async () => {
    statusLoading = true;
    statusError = null;
    try {
      status = await getSearchStatus();
    } catch (e) {
      statusError = e instanceof Error ? e.message : 'Failed to load search status';
    } finally {
      statusLoading = false;
    }
  };

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

  const handleTestConnection = async () => {
    const url = systemConfig?.meili_url?.trim();
    const key = systemConfig?.meili_master_key?.trim();

    if (!url) {
      testResult = { success: false, message: 'URL is required' };
      return;
    }

    testing = true;
    testResult = null;
    try {
      const result = await testSearchConnection({
        url,
        master_key: key || ''
      });
      testResult = result;
    } catch (e) {
      testResult = {
        success: false,
        message: e instanceof Error ? e.message : 'Test failed'
      };
    } finally {
      testing = false;
    }
  };

  const saveSystemConfig = async () => {
    if (!systemConfig) return;
    systemConfigSaving = true;
    systemConfigError = null;
    try {
      const updates: SystemConfigUpdate = {
        meili_url: systemConfig.meili_url ?? '',
        meili_master_key: systemConfig.meili_master_key ?? '',
        semantic_search_enabled: systemConfig.semantic_search_enabled,
        semantic_search_ratio: systemConfig.semantic_search_ratio,
        koblem_service_url: systemConfig.koblem_service_url,
        koblem_api_key: systemConfig.koblem_api_key,
        chat_enabled: systemConfig.chat_enabled,
        chat_llm_provider: systemConfig.chat_llm_provider ?? '',
        chat_llm_base_url: systemConfig.chat_llm_base_url ?? '',
        chat_llm_api_key: systemConfig.chat_llm_api_key ?? '',
        chat_llm_model: systemConfig.chat_llm_model ?? '',
        chat_llm_thinking: systemConfig.chat_llm_thinking ?? ''
      };
      systemConfig = await updateSystemConfig(updates);
      showMessage('Search settings saved');
      // Reload status to reflect new connection
      await loadStatus();
    } catch (e) {
      systemConfigError = e instanceof Error ? e.message : 'Failed to save config';
    } finally {
      systemConfigSaving = false;
    }
  };

  const handleReindex = async () => {
    reindexing = true;
    try {
      const result = await triggerReindex();
      showMessage(result.message);
      // Reload status after a short delay to show updated counts
      setTimeout(loadStatus, 2000);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to trigger reindex');
    } finally {
      reindexing = false;
    }
  };

  const handleReindexTranscripts = async () => {
    reindexingTranscripts = true;
    try {
      const result = await reindexTranscripts();
      showMessage(result.message);
      setTimeout(loadStatus, 2000);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to trigger transcript reindex');
    } finally {
      reindexingTranscripts = false;
    }
  };
</script>

<svelte:head>
  <title>Search Settings — Kobbler</title>
</svelte:head>

{#if !auth.isAdmin}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <div class="search-settings">
    <div class="section-header">
      <h2>Search</h2>
    </div>

    {#if statusLoading}
      <div class="loading-card">Loading search status...</div>
    {:else if statusError}
      <div class="error">{statusError}</div>
    {:else if status}
      <div class="config-card">
        <h3>Status</h3>
        <div class="status-grid">
          <div class="status-item">
            <span class="status-label">Backend</span>
            <span class="status-value">{status.backend_type}</span>
          </div>
          <div class="status-item">
            <span class="status-label">Connection</span>
            <span
              class="status-value"
              class:connected={status.connected}
              class:error={!status.connected}
            >
              {status.connected ? 'Connected' : 'Disconnected'}
            </span>
          </div>
          <div class="status-item">
            <span class="status-label">URL</span>
            <span class="status-value url">{status.url}</span>
          </div>
          {#if status.error}
            <div class="status-item full-width">
              <span class="status-label">Error</span>
              <span class="status-value error">{status.error}</span>
            </div>
          {/if}
        </div>

        {#if status.connected}
          <div class="index-stats">
            <h4>Indexed Documents</h4>
            <div class="stats-grid">
              <div class="stat-item">
                <span class="stat-count">{status.book_count ?? 0}</span>
                <span class="stat-label">Books</span>
              </div>
              <div class="stat-item">
                <span class="stat-count">{status.author_count ?? 0}</span>
                <span class="stat-label">Authors</span>
              </div>
              <div class="stat-item">
                <span class="stat-count">{status.series_count ?? 0}</span>
                <span class="stat-label">Series</span>
              </div>
              <div class="stat-item">
                <span class="stat-count">{status.genre_count ?? 0}</span>
                <span class="stat-label">Genres</span>
              </div>
              {#if systemConfig?.semantic_search_enabled}
                <div class="stat-item">
                  <span class="stat-count">{status.embedded_document_count ?? 0}</span>
                  <span class="stat-label">Titles with vectors</span>
                </div>
              {/if}
            </div>
          </div>
        {/if}
      </div>

      <div class="config-card">
        <h3>Rebuild Index</h3>
        <p class="field-hint">
          Rebuild all search indexes from the database. Use this after changing index settings or if
          search results seem stale.
        </p>
        <button class="btn" type="button" onclick={handleReindex} disabled={reindexing}>
          {reindexing ? 'Rebuilding...' : 'Rebuild Search Index'}
        </button>
      </div>

      <div class="config-card">
        <h3>Rebuild Transcript Index</h3>
        <p class="field-hint">
          Rebuild transcript indexes for audiobook transcriptions and ebook full text. This
          re-extracts and indexes all text content, which may take a while for large libraries.
        </p>
        <button
          class="btn"
          type="button"
          onclick={handleReindexTranscripts}
          disabled={reindexingTranscripts}
        >
          {reindexingTranscripts ? 'Rebuilding...' : 'Rebuild Transcript Index'}
        </button>
      </div>
    {/if}

    {#if systemConfigLoading}
      <div class="loading-card">Loading configuration...</div>
    {:else if systemConfigError}
      <div class="error">{systemConfigError}</div>
    {:else if systemConfig}
      <div class="config-card">
        <h3>MeiliSearch Connection</h3>
        <p class="card-description muted">
          Configure MeiliSearch connection. Test the connection before saving to verify credentials.
        </p>
        <label>
          MeiliSearch URL
          <input
            type="text"
            data-field="meili_url"
            autocomplete="off"
            value={systemConfig.meili_url ?? ''}
            oninput={(e) => {
              if (systemConfig)
                systemConfig.meili_url = (e.target as HTMLInputElement).value || null;
            }}
            placeholder="http://localhost:7700"
          />
          <span class="field-hint">MeiliSearch server URL</span>
        </label>
        <label>
          Master Key
          <input
            type="password"
            data-field="meili_master_key"
            autocomplete="off"
            value={systemConfig.meili_master_key ?? ''}
            oninput={(e) => {
              if (systemConfig)
                systemConfig.meili_master_key = (e.target as HTMLInputElement).value || null;
            }}
            placeholder="Enter master key"
          />
          <span class="field-hint">MeiliSearch master key for authentication</span>
        </label>
        <div class="test-section">
          <button
            class="btn alt"
            type="button"
            onclick={handleTestConnection}
            disabled={testing || !systemConfig.meili_url}
          >
            {testing ? 'Testing...' : 'Test Connection'}
          </button>
          {#if testResult}
            <div class="test-result" class:success={testResult.success}>
              <span class="result-icon">{testResult.success ? '✓' : '✗'}</span>
              <span class="result-message">{testResult.message}</span>
              {#if testResult.version}
                <span class="result-version">MeiliSearch v{testResult.version}</span>
              {/if}
            </div>
          {/if}
        </div>
      </div>

      <div class="config-card">
        <h3>Semantic Search</h3>
        <p class="card-description muted">
          Also match movies, TV shows and library books by how their summary — and, more lightly,
          their poster or cover — fits the query, so "a slow-burn thriller about grief" finds titles
          that share none of those words. Summaries, artwork and queries are embedded with
          EmbeddingGemma-2 by the embedding service; search falls back to keywords whenever it is
          unavailable.
        </p>

        <label class="toggle-label">
          <input
            type="checkbox"
            data-field="semantic_search_enabled"
            checked={systemConfig.semantic_search_enabled}
            disabled={!embedderActive && !systemConfig.semantic_search_enabled}
            onchange={(e) => {
              if (systemConfig)
                systemConfig.semantic_search_enabled = (e.target as HTMLInputElement).checked;
            }}
          />
          <span>Enable semantic search</span>
        </label>
        {#if !embedderActive}
          <span class="field-hint">Needs an Embedding Service URL (Settings → Processing).</span>
        {:else}
          <span class="field-hint">
            Saving a change starts a background job that embeds or removes the summary and artwork
            vectors.
          </span>
        {/if}

        {#if systemConfig.semantic_search_enabled}
          <label>
            Semantic ratio: {systemConfig.semantic_search_ratio.toFixed(1)}
            <input
              type="range"
              data-field="semantic_search_ratio"
              min="0"
              max="1"
              step="0.1"
              value={systemConfig.semantic_search_ratio}
              oninput={(e) => {
                if (systemConfig)
                  systemConfig.semantic_search_ratio = Number((e.target as HTMLInputElement).value);
              }}
            />
            <span class="field-hint">0 = keyword matches only, 1 = semantic similarity only</span>
          </label>
        {/if}
      </div>

      <div class="config-card">
        <h3>Chat (AI Assistant)</h3>
        <p class="card-description muted">
          Enable conversational AI search powered by your own LLM. Ask natural language questions
          about your library.
        </p>

        <label class="toggle-label">
          <input
            type="checkbox"
            data-field="chat_enabled"
            checked={systemConfig.chat_enabled}
            onchange={(e) => {
              if (systemConfig) systemConfig.chat_enabled = (e.target as HTMLInputElement).checked;
            }}
          />
          <span>Enable Chat</span>
        </label>

        {#if systemConfig.chat_enabled && koblemActive}
          <div class="koblem-active-note">
            <span class="result-icon">✓</span>
            <span>
              Chat is using your local <strong>Koblem</strong> LLM ({systemConfig.koblem_service_url}).
              The external provider settings below are ignored. To use an external provider instead,
              clear the Koblem Service URL in the Local LLM section.
            </span>
          </div>
        {/if}

        {#if systemConfig.chat_enabled}
          <label>
            Thinking level
            <select
              data-field="chat_llm_thinking"
              value={systemConfig.chat_llm_thinking ?? ''}
              oninput={(e) => {
                if (systemConfig)
                  systemConfig.chat_llm_thinking = (e.target as HTMLSelectElement).value || null;
              }}
            >
              <option value="">Auto (default by model)</option>
              <option value="off">Off</option>
              <option value="low">Low</option>
              <option value="medium">Medium</option>
              <option value="high">High</option>
            </select>
            <span class="field-hint"
              >Applies to the Koblem route; auto is medium for Qwen models, off otherwise</span
            >
          </label>

          <fieldset class="external-provider" disabled={koblemActive} class:dimmed={koblemActive}>
            <legend>External LLM provider {koblemActive ? '(unused — Koblem active)' : ''}</legend>
            <label>
              LLM Provider
              <select
                data-field="chat_llm_provider"
                value={systemConfig.chat_llm_provider ?? ''}
                onchange={(e) => {
                  if (!systemConfig) return;
                  const provider = (e.target as HTMLSelectElement).value || null;
                  systemConfig.chat_llm_provider = provider;
                  // Auto-populate base URL and default model for known providers
                  const defaults: Record<string, { url: string | null; model: string }> = {
                    gemini: {
                      url: 'https://generativelanguage.googleapis.com/v1beta/openai/',
                      model: 'gemini-2.5-flash-lite'
                    },
                    openai: { url: 'https://api.openai.com/v1/', model: 'gpt-4o-mini' },
                    mistral: { url: null, model: 'mistral-small-latest' }
                  };
                  const d = provider ? defaults[provider] : undefined;
                  if (d) {
                    systemConfig.chat_llm_base_url = d.url;
                    if (!systemConfig.chat_llm_model) systemConfig.chat_llm_model = d.model;
                  }
                }}
              >
                <option value="">Select provider...</option>
                <option value="mistral">Mistral</option>
                <option value="gemini">Gemini</option>
                <option value="openai">OpenAI</option>
                <option value="custom">Custom (OpenAI-compatible)</option>
              </select>
              <span class="field-hint">Choose your LLM provider</span>
            </label>

            <label>
              Base URL
              <input
                type="text"
                data-field="chat_llm_base_url"
                autocomplete="off"
                value={systemConfig.chat_llm_base_url ?? ''}
                oninput={(e) => {
                  if (systemConfig)
                    systemConfig.chat_llm_base_url = (e.target as HTMLInputElement).value || null;
                }}
                placeholder="https://api.example.com/v1/"
                disabled={systemConfig.chat_llm_provider !== 'custom'}
              />
              <span class="field-hint"
                >API endpoint URL (auto-set for known providers, editable for custom)</span
              >
            </label>

            <label>
              API Key
              <input
                type="password"
                data-field="chat_llm_api_key"
                autocomplete="off"
                value={systemConfig.chat_llm_api_key ?? ''}
                oninput={(e) => {
                  if (systemConfig)
                    systemConfig.chat_llm_api_key = (e.target as HTMLInputElement).value || null;
                }}
                placeholder="Enter your API key"
              />
              <span class="field-hint">Your LLM provider API key</span>
            </label>

            <label>
              Model
              <input
                type="text"
                data-field="chat_llm_model"
                autocomplete="off"
                value={systemConfig.chat_llm_model ?? ''}
                oninput={(e) => {
                  if (systemConfig)
                    systemConfig.chat_llm_model = (e.target as HTMLInputElement).value || null;
                }}
                placeholder={systemConfig.chat_llm_provider === 'gemini'
                  ? 'gemini-2.5-flash-lite'
                  : systemConfig.chat_llm_provider === 'mistral'
                    ? 'mistral-small-latest'
                    : systemConfig.chat_llm_provider === 'openai'
                      ? 'gpt-4o-mini'
                      : 'model-name'}
              />
              <span class="field-hint">LLM model to use for chat</span>
            </label>
          </fieldset>
        {/if}
      </div>

      {#if systemConfig.koblem_ui_enabled}
        <div class="config-card">
          <h3>Local LLM (Koblem)</h3>
          <p class="card-description muted">
            Connect a local LLM service for enhanced features like review analysis.
          </p>
          <label>
            Koblem Service URL
            <input
              type="text"
              data-field="koblem_service_url"
              autocomplete="off"
              value={systemConfig.koblem_service_url ?? ''}
              oninput={(e) => {
                if (systemConfig)
                  systemConfig.koblem_service_url = (e.target as HTMLInputElement).value || null;
              }}
              placeholder="http://koblem:8090"
            />
            <span class="field-hint">Local LLM service for enhanced features</span>
          </label>
          <label>
            Koblem API Key
            <input
              type="password"
              data-field="koblem_api_key"
              autocomplete="off"
              value={systemConfig.koblem_api_key ?? ''}
              oninput={(e) => {
                if (systemConfig)
                  systemConfig.koblem_api_key = (e.target as HTMLInputElement).value || null;
              }}
              placeholder="(required for tier-2 streaming TTS)"
            />
            <span class="field-hint"
              >Matches koblem's KOBLEM_API_KEY env. Required for streaming TTS with word timings.</span
            >
          </label>
        </div>
      {/if}

      <div class="config-actions">
        <button class="btn" type="button" onclick={saveSystemConfig} disabled={systemConfigSaving}>
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
    {/if}
  </div>
{/if}

<style>
  .koblem-active-note {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    padding: 10px 12px;
    margin: 8px 0 12px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: color-mix(in srgb, var(--accent, #2e7d32) 8%, transparent);
    font-size: 13px;
    line-height: 1.4;
  }

  .external-provider {
    border: 1px solid var(--ring);
    border-radius: 8px;
    padding: 12px;
    margin: 0;
  }

  .external-provider legend {
    font-size: 12px;
    color: var(--muted);
    padding: 0 6px;
  }

  .external-provider.dimmed {
    opacity: 0.55;
  }

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

  .search-settings {
    display: grid;
    gap: 16px;
  }

  .section-header h2 {
    margin: 0;
    font-size: 18px;
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
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }

  .config-card h4 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--muted);
  }

  .card-description {
    font-size: 13px;
    margin: 0;
  }

  .field-hint {
    font-size: 11px;
    color: var(--muted);
  }

  .status-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 12px;
  }

  .status-item {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .status-item.full-width {
    grid-column: 1 / -1;
  }

  .status-label {
    font-size: 11px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .status-value {
    font-size: 14px;
    font-weight: 500;
  }

  .status-value.url {
    font-family: monospace;
    font-size: 13px;
    word-break: break-all;
  }

  .status-value.connected {
    color: var(--accent-2, #22c55e);
  }

  .status-value.error {
    color: var(--danger, #ef4444);
  }

  .index-stats {
    padding-top: 12px;
    border-top: 1px solid var(--ring);
    display: grid;
    gap: 8px;
  }

  .stats-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(120px, 1fr));
    gap: 12px;
  }

  .stat-item {
    text-align: center;
    padding: 12px;
    background: var(--bg);
    border-radius: 10px;
  }

  .stat-count {
    display: block;
    font-size: 24px;
    font-weight: 600;
  }

  .stat-label {
    font-size: 11px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .config-card label {
    display: grid;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .config-card input[type='text'],
  .config-card input[type='password'] {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .test-section {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }

  .test-result {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-radius: 8px;
    font-size: 13px;
    background: rgba(239, 68, 68, 0.1);
    color: var(--danger, #ef4444);
  }

  .test-result.success {
    background: rgba(34, 197, 94, 0.1);
    color: var(--accent-2, #22c55e);
  }

  .result-icon {
    font-size: 16px;
    font-weight: 600;
  }

  .result-message {
    flex: 1;
  }

  .result-version {
    font-size: 11px;
    opacity: 0.8;
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
    width: fit-content;
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

  .muted {
    color: var(--muted);
  }

  .toggle-label {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    font-size: 13px;
    color: var(--ink);
  }

  .toggle-label input[type='checkbox'] {
    width: 16px;
    height: 16px;
    accent-color: var(--accent);
  }

  .config-card input[type='range'] {
    accent-color: var(--accent);
  }

  .config-card select {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .config-card input:disabled,
  .config-card select:disabled {
    opacity: 0.5;
  }

  @media (max-width: 768px) {
    .stats-grid {
      grid-template-columns: repeat(2, 1fr);
    }
  }

  @media (max-width: 480px) {
    .stats-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
