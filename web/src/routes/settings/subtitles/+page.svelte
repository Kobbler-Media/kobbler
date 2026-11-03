<script lang="ts">
  import { onMount } from 'svelte';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { getAuthState } from '#lib/stores/auth.svelte.js';
  import { getSubtitleProviders, updateSubtitleProvider } from '#lib/api/client.js';
  import type { SubtitleProvider } from '#lib/types.js';

  const auth = getAuthState();

  let providers = $state<SubtitleProvider[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let editingId = $state<string | null>(null);
  let editingEnabled = $state(false);
  let editingPriority = $state(1);
  let editingConfig = $state<Record<string, unknown>>({});

  const sortedProviders = $derived([...providers].sort((a, b) => a.priority - b.priority));

  onMount(async () => {
    await loadProviders();
  });

  const loadProviders = async () => {
    loading = true;
    error = null;
    try {
      providers = await getSubtitleProviders();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load subtitle providers';
    } finally {
      loading = false;
    }
  };

  const startEdit = (provider: SubtitleProvider) => {
    editingId = provider.id;
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

  const cancelEdit = () => {
    editingId = null;
    editingConfig = {};
  };

  const saveProvider = async (providerId: string) => {
    try {
      const configUpdate: Record<string, unknown> = {};
      const provider = providers.find((p) => p.id === providerId);
      for (const [key, value] of Object.entries(editingConfig)) {
        const fieldDef = provider?.config_fields.find((f) => f.key === key);
        if (value !== '') {
          configUpdate[key] = value;
        } else if (fieldDef?.field_type === 'secret' && fieldDef?.current_value) {
          configUpdate[key] = null;
        }
      }

      await updateSubtitleProvider(providerId, {
        enabled: editingEnabled,
        priority: editingPriority,
        config: Object.keys(configUpdate).length > 0 ? configUpdate : undefined
      });
      await loadProviders();
      editingId = null;
      editingConfig = {};
      showMessage('Subtitle provider updated');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update subtitle provider');
    }
  };

  const toggleEnabled = async (provider: SubtitleProvider) => {
    try {
      await updateSubtitleProvider(provider.id, { enabled: !provider.enabled });
      await loadProviders();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to toggle subtitle provider');
    }
  };

  const formatCapability = (cap: string): string => {
    return cap
      .split('_')
      .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
      .join(' ');
  };
</script>

<svelte:head>
  <title>Subtitle Settings — Kobbler</title>
</svelte:head>

{#if !auth.isAdmin}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <div class="subtitles-page">
    <h2>Subtitles</h2>

    <div class="section">
      <h3>Providers</h3>

      {#if loading}
        <div class="loading-card">Loading subtitle providers...</div>
      {:else if error}
        <div class="error">{error}</div>
      {:else if providers.length === 0}
        <div class="empty-state">
          <h4>No subtitle providers configured</h4>
          <p class="muted">
            Subtitle providers are used to search and download subtitles for movies and TV episodes.
          </p>
        </div>
      {:else}
        <p class="muted" style="margin-bottom: 16px;">
          Configure subtitle providers for downloading subtitles. Lower priority numbers are
          searched first. Enabled providers are also used for automatic forced subtitle detection on
          foreign-language content.
        </p>
        <div class="provider-list">
          {#each sortedProviders as provider (provider.id)}
            <div class="provider-row" class:disabled={!provider.enabled}>
              {#if editingId === provider.id}
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
                    <button class="btn alt small" type="button" onclick={cancelEdit}>Cancel</button>
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
                    onclick={() => toggleEnabled(provider)}
                  >
                    {provider.enabled ? 'Disable' : 'Enable'}
                  </button>
                  <button class="btn alt small" type="button" onclick={() => startEdit(provider)}>
                    Edit
                  </button>
                </div>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
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

  .subtitles-page {
    display: grid;
    gap: 24px;
  }

  .subtitles-page > h2 {
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
  }
</style>
