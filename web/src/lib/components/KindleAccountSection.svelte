<script lang="ts">
  import * as api from '$lib/api/client';
  import type { KindleTarget } from '$lib/api/client';
  import { showMessage } from '$lib/context/app.svelte';

  interface Props {
    /** The Kobbler device ID to link kindle targets to. */
    deviceId: string;
  }

  let { deviceId }: Props = $props();

  let connected = $state(false);
  let targets = $state<KindleTarget[]>([]);
  let loading = $state(false);
  let loaded = $state(false);

  let oauthStep = $state<'idle' | 'waiting_for_url'>('idle');
  let pastedUrl = $state('');

  export async function load() {
    try {
      const account = await api.getKindleAccount();
      connected = account.connected;
      targets = account.targets;
    } catch {
      connected = false;
      targets = [];
    }
    loaded = true;
  }

  async function handleStartConnect() {
    loading = true;
    try {
      const { url } = await api.getKindleOAuthUrl();
      window.open(url, '_blank');
      oauthStep = 'waiting_for_url';
    } catch {
      showMessage('Failed to get OAuth URL');
    } finally {
      loading = false;
    }
  }

  async function handleCompleteOAuth() {
    if (!pastedUrl.trim()) return;
    loading = true;
    try {
      await api.completeKindleOAuth(pastedUrl.trim(), deviceId);
      showMessage('Kindle account connected!');
      oauthStep = 'idle';
      pastedUrl = '';
      await load();
    } catch {
      showMessage('OAuth failed - check the URL and try again');
    } finally {
      loading = false;
    }
  }

  async function handleDisconnect() {
    if (!confirm('Disconnect Kindle account? This removes all targets and delivery history.'))
      return;
    loading = true;
    try {
      await api.disconnectKindle();
      connected = false;
      targets = [];
      showMessage('Kindle account disconnected');
    } catch {
      showMessage('Failed to disconnect');
    } finally {
      loading = false;
    }
  }

  async function handleRefreshDevices() {
    loading = true;
    try {
      const result = await api.refreshKindleDevices(deviceId);
      targets = result.targets;
      showMessage('Devices refreshed');
    } catch {
      showMessage('Failed to refresh devices');
    } finally {
      loading = false;
    }
  }

  async function handleToggleTarget(target: KindleTarget) {
    try {
      await api.updateKindleTarget(target.id, !target.enabled);
      target.enabled = !target.enabled;
      targets = [...targets];
    } catch {
      showMessage('Failed to update target');
    }
  }
</script>

<div class="kindle-section">
  <div class="section-label">Kindle Account</div>

  {#if !loaded}
    <div class="muted loading-text">Loading...</div>
  {:else if !connected}
    {#if oauthStep === 'idle'}
      <button class="btn" type="button" onclick={handleStartConnect} disabled={loading}>
        {loading ? 'Opening...' : 'Connect Amazon Account'}
      </button>
      <span class="hint">Link your Amazon account to send books to Kindle devices.</span>
    {:else}
      <div class="oauth-paste">
        <p class="hint">
          Sign in on the tab that opened, then copy the full URL from your browser and paste below.
        </p>
        <input
          type="text"
          bind:value={pastedUrl}
          placeholder="Paste the redirect URL here..."
          class="paste-input"
        />
        <div class="paste-actions">
          <button
            class="btn"
            type="button"
            onclick={handleCompleteOAuth}
            disabled={loading || !pastedUrl.trim()}
          >
            {loading ? 'Connecting...' : 'Connect'}
          </button>
          <button class="btn alt" type="button" onclick={() => (oauthStep = 'idle')}>
            Cancel
          </button>
        </div>
      </div>
    {/if}
  {:else}
    <div class="connected-header">
      <span class="connected-badge">Connected</span>
      <div class="connected-actions">
        <button
          class="btn alt small"
          type="button"
          onclick={handleRefreshDevices}
          disabled={loading}
        >
          Refresh
        </button>
        <button class="btn alt small" type="button" onclick={handleDisconnect} disabled={loading}>
          Disconnect
        </button>
      </div>
    </div>

    {#if targets.length === 0}
      <div class="muted">No Kindle devices found. Click Refresh to discover them.</div>
    {:else}
      <div class="target-list">
        {#each targets as target (target.id)}
          <label class="target-item">
            <div class="target-info">
              <span class="target-name">{target.device_name || target.device_serial}</span>
              {#if target.device_name}
                <span class="target-serial muted">{target.device_serial}</span>
              {/if}
            </div>
            <input
              type="checkbox"
              checked={target.enabled}
              onchange={() => handleToggleTarget(target)}
            />
          </label>
        {/each}
      </div>
    {/if}

    <span class="hint"> Enabled targets receive books when entitled to this device. </span>
  {/if}
</div>

<style>
  .kindle-section {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding-top: 12px;
    border-top: 1px solid var(--ring);
  }

  .section-label {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
  }

  .loading-text {
    font-size: 12px;
  }

  .hint {
    font-size: 11px;
    color: var(--muted);
  }

  .oauth-paste {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .paste-input {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 12px;
    width: 100%;
  }

  .paste-actions {
    display: flex;
    gap: 8px;
  }

  .connected-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .connected-badge {
    font-size: 12px;
    color: var(--accent);
    font-weight: 600;
  }

  .connected-actions {
    display: flex;
    gap: 6px;
  }

  .target-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .target-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 10px;
    border: 1px solid var(--ring);
    border-radius: 10px;
    cursor: pointer;
    gap: 8px;
  }

  .target-item:hover {
    border-color: var(--muted);
  }

  .target-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .target-name {
    font-size: 13px;
    font-weight: 500;
  }

  .target-serial {
    font-size: 10px;
    font-family: monospace;
  }

  .target-item input[type='checkbox'] {
    width: 16px;
    height: 16px;
    accent-color: var(--accent);
    cursor: pointer;
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

  .btn.alt {
    background: var(--card);
    border: 1px solid var(--ring);
    color: var(--ink);
  }

  .btn.small {
    padding: 4px 10px;
    font-size: 11px;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
