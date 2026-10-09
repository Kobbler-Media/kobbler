<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import {
    getVideoSyncProviders,
    updateVideoSyncProvider,
    traktAuthorize,
    traktPollDeviceCode,
    traktDisconnect,
    traktImportHistory
  } from '$lib/api/client';
  import type { VideoSyncProvider } from '$lib/types';

  let provider = $state<VideoSyncProvider | null>(null);
  let loading = $state(true);

  // OAuth device code flow state
  let oauthStep = $state<'idle' | 'waiting' | 'polling'>('idle');
  let clientId = $state('');
  let clientSecret = $state('');
  let userCode = $state('');
  let verificationUrl = $state('');
  let deviceCode = $state('');
  let pollInterval: ReturnType<typeof setInterval> | null = null;
  let importing = $state(false);
  let reconnecting = $state(false);

  // Masked secrets show as "••••••••" which means a token exists
  const isConnected = $derived(provider?.config?.access_token === '••••••••');

  onMount(async () => {
    await loadProvider();
  });

  onDestroy(() => {
    if (pollInterval) clearInterval(pollInterval);
  });

  async function loadProvider() {
    loading = true;
    try {
      const providers = await getVideoSyncProviders();
      provider = providers.find((p) => p.provider_type === 'trakt') ?? null;
    } catch {
      showMessage('Failed to load Trakt settings');
    } finally {
      loading = false;
    }
  }

  async function startOAuth() {
    if (!clientId.trim()) {
      showMessage('Please enter your Trakt Client ID');
      return;
    }
    if (!clientSecret.trim()) {
      showMessage('Please enter your Trakt Client Secret');
      return;
    }

    try {
      const result = await traktAuthorize(clientId.trim());
      userCode = result.user_code;
      verificationUrl = result.verification_url;
      deviceCode = result.device_code;
      oauthStep = 'polling';

      // Start polling
      pollInterval = setInterval(
        async () => {
          try {
            const poll = await traktPollDeviceCode(
              deviceCode,
              clientId.trim(),
              clientSecret.trim()
            );

            if (poll.status === 'authorized') {
              stopPolling();
              oauthStep = 'idle';
              clientId = '';
              clientSecret = '';
              reconnecting = false;
              showMessage('Trakt connected successfully');
              await loadProvider();
            } else if (poll.status === 'expired' || poll.status === 'denied') {
              stopPolling();
              oauthStep = 'idle';
              showMessage(`Trakt authorization ${poll.status}`);
            }
          } catch {
            // Poll errors are expected, keep trying
          }
        },
        (result.interval || 5) * 1000
      );
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to start Trakt authorization');
    }
  }

  function stopPolling() {
    if (pollInterval) {
      clearInterval(pollInterval);
      pollInterval = null;
    }
  }

  function cancelOAuth() {
    stopPolling();
    oauthStep = 'idle';
    reconnecting = false;
    userCode = '';
    deviceCode = '';
  }

  async function handleDisconnect() {
    if (!confirm('Disconnect from Trakt? This will remove your access tokens.')) return;
    try {
      await traktDisconnect();
      showMessage('Disconnected from Trakt');
      await loadProvider();
    } catch {
      showMessage('Failed to disconnect from Trakt');
    }
  }

  async function handleToggleEnabled() {
    if (!provider) return;
    try {
      await updateVideoSyncProvider(provider.provider_type, { enabled: !provider.enabled });
      await loadProvider();
    } catch {
      showMessage('Failed to update Trakt settings');
    }
  }

  async function handleImportHistory() {
    importing = true;
    try {
      await traktImportHistory();
      showMessage('Import job queued — check Jobs page for progress');
    } catch {
      showMessage('Failed to queue import');
    } finally {
      importing = false;
    }
  }
</script>

<div class="section-header">
  <h2>Video Trackers</h2>
</div>

{#if loading}
  <div class="loading-card">Loading settings...</div>
{:else if provider}
  <p class="muted description">
    Connect your Trakt account to sync watch history for movies and TV shows. When enabled, your
    watch state changes will be pushed to Trakt automatically.
  </p>

  <div class="provider-card" class:disabled={!provider.enabled}>
    {#if oauthStep === 'polling'}
      <div class="oauth-flow">
        <div class="provider-edit-header">
          <div class="provider-logo">
            <svg viewBox="0 0 24 24" width="24" height="24" fill="currentColor">
              <path
                d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm-2 15l-5-5 1.41-1.41L10 14.17l7.59-7.59L19 8l-9 9z"
              ></path>
            </svg>
          </div>
          <h4>Authorize Trakt</h4>
        </div>

        <div class="device-code-section">
          <p>
            Go to <a href={verificationUrl} target="_blank" rel="noopener">{verificationUrl}</a> and enter
            this code:
          </p>
          <div class="user-code">{userCode}</div>
          <p class="muted hint">Waiting for authorization...</p>
        </div>

        <div class="provider-edit-actions">
          <button class="btn alt small" type="button" onclick={cancelOAuth}>Cancel</button>
        </div>
      </div>
    {:else if oauthStep === 'idle' && (!isConnected || reconnecting)}
      <div class="provider-edit-form">
        <div class="provider-edit-header">
          <div class="provider-logo">
            <svg viewBox="0 0 24 24" width="24" height="24" fill="currentColor">
              <path
                d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm-2 15l-5-5 1.41-1.41L10 14.17l7.59-7.59L19 8l-9 9z"
              ></path>
            </svg>
          </div>
          <span class="provider-name">Trakt</span>
          <span class="status-badge">{reconnecting ? 'Reconnecting' : 'Not Connected'}</span>
        </div>

        <p class="provider-description muted">
          Track your movies and TV shows on Trakt. Syncs watch state and history.
        </p>

        <div class="provider-capabilities">
          <span class="capability-badge">Watch State</span>
          <span class="capability-badge">History</span>
          <span class="capability-badge">Import</span>
        </div>

        <div class="provider-edit-fields">
          <label>
            Client ID
            <input type="text" bind:value={clientId} placeholder="Your Trakt API app client ID" />
            <span class="field-hint">
              Create an app at <a
                href="https://trakt.tv/oauth/applications"
                target="_blank"
                rel="noopener">trakt.tv/oauth/applications</a
              >
            </span>
          </label>
          <label>
            Client Secret
            <input
              type="password"
              bind:value={clientSecret}
              placeholder="Your Trakt API app client secret"
            />
          </label>
        </div>

        <div class="provider-edit-actions">
          <button class="btn small" type="button" onclick={startOAuth}> Connect to Trakt </button>
        </div>
      </div>
    {:else}
      <div class="provider-row">
        <div class="provider-info">
          <div class="provider-header">
            <div class="provider-logo">
              <svg viewBox="0 0 24 24" width="24" height="24" fill="currentColor">
                <path
                  d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm-2 15l-5-5 1.41-1.41L10 14.17l7.59-7.59L19 8l-9 9z"
                ></path>
              </svg>
            </div>
            <span class="provider-name">Trakt</span>
            <span class="config-badge">Connected</span>
            <span class="status-badge" class:enabled={provider.enabled}>
              {provider.enabled ? 'Enabled' : 'Disabled'}
            </span>
          </div>

          <p class="provider-description muted">
            Track your movies and TV shows on Trakt. Syncs watch state and history.
          </p>

          <div class="provider-capabilities">
            <span class="capability-badge">Watch State</span>
            <span class="capability-badge">History</span>
            <span class="capability-badge">Import</span>
          </div>

          <a href="https://trakt.tv" target="_blank" class="provider-website muted">trakt.tv</a>
        </div>

        <div class="provider-actions">
          <button class="btn alt small" type="button" onclick={handleToggleEnabled}>
            {provider.enabled ? 'Disable' : 'Enable'}
          </button>
          <button
            class="btn alt small"
            type="button"
            onclick={handleImportHistory}
            disabled={importing || !provider.enabled}
          >
            {importing ? 'Importing...' : 'Import History'}
          </button>
          <button class="btn alt small" type="button" onclick={() => (reconnecting = true)}>
            Reconnect
          </button>
          <button class="btn alt small danger" type="button" onclick={handleDisconnect}>
            Disconnect
          </button>
        </div>
      </div>
    {/if}
  </div>

  {#if isConnected && provider.enabled}
    <div class="sync-info">
      <span class="material-icons-outlined">check_circle</span>
      <span>Watch state changes will sync to Trakt automatically.</span>
    </div>
  {:else if isConnected && !provider.enabled}
    <div class="sync-info warning">
      <span class="material-icons-outlined">pause_circle</span>
      <span>Sync paused. Enable to resume syncing to Trakt.</span>
    </div>
  {/if}
{/if}

<style>
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

  /* OAuth device code */
  .oauth-flow {
    display: grid;
    gap: 16px;
  }

  .device-code-section {
    text-align: center;
  }

  .device-code-section p {
    margin: 0 0 8px;
    font-size: 13px;
  }

  .user-code {
    font-size: 28px;
    font-weight: 700;
    letter-spacing: 4px;
    font-family: monospace;
    padding: 12px 24px;
    background: var(--ring);
    border-radius: 12px;
    display: inline-block;
  }

  .hint {
    font-size: 12px;
    margin-top: 8px;
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

  .provider-edit-fields input[type='text'],
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

  .sync-info .material-icons-outlined {
    font-size: 18px;
  }

  .loading-card {
    padding: 24px;
    text-align: center;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    color: var(--muted);
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
