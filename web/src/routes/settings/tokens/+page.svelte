<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { getLibraryState } from '#lib/stores/library.svelte.js';
  import { getDevicesState, setCreatedIngestToken } from '#lib/stores/devices.svelte.js';
  import { loadIngestTokens, showMessage, copyText } from '#lib/context/app.svelte.js';
  import { getAuthState } from '#lib/stores/auth.svelte.js';
  import MetaLine from '#lib/components/shared/MetaLine.svelte';
  import * as api from '#lib/api/client.js';

  const auth = getAuthState();

  // Store state
  const library = getLibraryState();
  const devicesStore = getDevicesState();

  // Derived state
  const ingestTokens = $derived(devicesStore.ingestTokens);
  const createdIngestToken = $derived(devicesStore.createdIngestToken);
  const loadState = $derived(library.loadState);

  // Local state
  let loading = $state(false);
  let newTokenName = $state('');
  let editingTokenId = $state<string | null>(null);
  let editingTokenName = $state('');

  onMount(async () => {
    await loadIngestTokens();
  });

  const handleCreateIngestToken = async () => {
    loading = true;
    try {
      const result = await api.createIngestToken(newTokenName || undefined);
      setCreatedIngestToken(result.token);
      showMessage('API token created');
      await loadIngestTokens();
      newTokenName = '';
    } catch {
      showMessage('Create token failed');
    } finally {
      loading = false;
    }
  };

  const handleRevokeIngestToken = async (tokenId: string) => {
    loading = true;
    try {
      await api.revokeIngestToken(tokenId);
      showMessage('Token revoked');
      await loadIngestTokens();
    } catch {
      showMessage('Revoke token failed');
    } finally {
      loading = false;
    }
  };

  const handleCopyText = async (value: string, label: string) => {
    await copyText(value, label);
  };

  const startEditing = async (token: { id: string; name: string | null }) => {
    editingTokenId = token.id;
    editingTokenName = token.name || '';
    await tick();
    const input = document.querySelector<HTMLInputElement>('.token-name-input');
    input?.focus();
  };

  const cancelEditing = () => {
    editingTokenId = null;
    editingTokenName = '';
  };

  const saveTokenName = async () => {
    if (!editingTokenId) return;
    loading = true;
    try {
      await api.renameIngestToken(editingTokenId, editingTokenName);
      showMessage('Token renamed');
      await loadIngestTokens();
    } catch {
      showMessage('Rename failed');
    } finally {
      loading = false;
      editingTokenId = null;
      editingTokenName = '';
    }
  };

  const handleEditKeydown = (e: KeyboardEvent) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      saveTokenName();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      cancelEditing();
    }
  };

  const ingestUrl = $derived(
    createdIngestToken && typeof window !== 'undefined'
      ? `${window.location.origin}/api/v1/ingest/${createdIngestToken}`
      : ''
  );
</script>

<svelte:head>
  <title>API Tokens — Kobbler</title>
</svelte:head>

{#if !auth.isAdmin}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <div class="tokens-section">
    <div class="section-header">
      <h2>API Tokens</h2>
      <span class="muted">{ingestTokens.length} tokens</span>
    </div>

    <div class="token-create-form">
      <input
        type="text"
        class="input"
        placeholder="Token name (e.g., Calibre, Script)"
        bind:value={newTokenName}
        disabled={loading}
      />
      <button class="btn" type="button" onclick={handleCreateIngestToken} disabled={loading}>
        Generate Token
      </button>
    </div>

    {#if createdIngestToken}
      <div class="token-callout">
        <div class="muted">New token created:</div>
        <div class="code">{createdIngestToken}</div>
        <button
          class="btn alt small"
          type="button"
          onclick={() => handleCopyText(createdIngestToken, 'API token')}
          disabled={loading}
        >
          Copy token
        </button>
        <div class="muted" style="margin-top: 12px;">Ingest URL (for Kobload webhook):</div>
        <div class="code">{ingestUrl}</div>
        <button
          class="btn alt small"
          type="button"
          onclick={() => handleCopyText(ingestUrl, 'Ingest URL')}
          disabled={loading}
        >
          Copy URL
        </button>
      </div>
    {/if}

    {#if loadState.ingestTokens.status === 'loading'}
      <div class="muted">Loading API tokens…</div>
    {:else if loadState.ingestTokens.status === 'error'}
      <div class="error">{loadState.ingestTokens.error}</div>
    {:else if ingestTokens.length === 0}
      <div class="empty-state">
        <h3>No API tokens yet</h3>
        <p class="muted">Generate a token for service integrations (Kobload, Koblem, scripts).</p>
      </div>
    {:else}
      <div class="token-list">
        {#each ingestTokens as token (token.id)}
          <div class="token-row" class:revoked={token.revoked}>
            <div class="token-info">
              {#if editingTokenId === token.id}
                <input
                  type="text"
                  class="token-name-input"
                  bind:value={editingTokenName}
                  onkeydown={handleEditKeydown}
                  onblur={saveTokenName}
                  disabled={loading}
                />
              {:else}
                <button
                  type="button"
                  class="token-name-btn"
                  onclick={() => startEditing(token)}
                  title="Click to rename"
                >
                  {token.name || 'Unnamed token'}
                </button>
              {/if}
              <code class="token-value">{token.token}</code>
              <div class="token-meta muted">
                <MetaLine
                  items={[
                    `Created ${new Date(token.created_at).toLocaleString()}`,
                    token.revoked && 'Revoked'
                  ]}
                />
              </div>
            </div>
            <div class="token-actions">
              <button
                class="btn alt small"
                type="button"
                onclick={() => handleCopyText(token.token, 'Token')}
                disabled={loading}
              >
                Copy
              </button>
              {#if !token.revoked}
                <button
                  class="btn btn-danger small"
                  type="button"
                  onclick={() => handleRevokeIngestToken(token.id)}
                  disabled={loading}
                >
                  Revoke
                </button>
              {/if}
            </div>
          </div>
        {/each}
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

  .tokens-section {
    display: grid;
    gap: 16px;
  }

  .section-header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }

  .section-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .token-create-form {
    display: flex;
    gap: 12px;
    flex-wrap: wrap;
  }

  .input {
    flex: 1;
    min-width: 200px;
    padding: 8px 12px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .token-callout {
    background: var(--card);
    border: 1px solid var(--accent);
    border-radius: 12px;
    padding: 16px;
  }

  .code {
    font-family: monospace;
    font-size: 12px;
    background: var(--bg);
    padding: 8px 12px;
    border-radius: 8px;
    word-break: break-all;
    margin: 8px 0;
  }

  .token-list {
    display: grid;
    gap: 8px;
  }

  .token-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
  }

  .token-row.revoked {
    opacity: 0.5;
  }

  .token-info {
    flex: 1;
    min-width: 0;
  }

  .token-name-btn {
    font-weight: 600;
    font-size: 14px;
    margin-bottom: 4px;
    background: none;
    border: 1px solid transparent;
    border-radius: 6px;
    padding: 2px 6px;
    margin-left: -6px;
    cursor: pointer;
    color: var(--ink);
    text-align: left;
  }

  .token-name-btn:hover {
    border-color: var(--ring);
    background: var(--bg);
  }

  .token-name-input {
    font-weight: 600;
    font-size: 14px;
    margin-bottom: 4px;
    padding: 2px 6px;
    margin-left: -6px;
    border: 1px solid var(--accent);
    border-radius: 6px;
    background: var(--bg);
    color: var(--ink);
    outline: none;
    width: 100%;
    max-width: 300px;
  }

  .token-value {
    font-family: monospace;
    font-size: 12px;
    word-break: break-all;
  }

  .token-meta {
    font-size: 11px;
    margin-top: 4px;
  }

  .token-actions {
    display: flex;
    gap: 8px;
    flex-shrink: 0;
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

  .btn.small {
    padding: 6px 10px;
    font-size: 12px;
  }

  .btn-danger {
    background: var(--danger);
    color: white;
  }

  .empty-state {
    padding: 32px;
    text-align: center;
    background: var(--card);
    border: 1px dashed var(--ring);
    border-radius: 16px;
  }

  .empty-state h3 {
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
</style>
