<script lang="ts">
  import { Modal } from '@koblibs/ui';
  import { SearchSelect, type SearchSelectItem } from '@koblibs/ui';
  import KindleAccountSection from './KindleAccountSection.svelte';
  import type { UserSummary, DeviceType } from '#lib/types.js';
  import * as api from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';

  interface Props {
    users: UserSummary[];
    onClose: () => void;
    onCreated: () => void;
  }

  let { users, onClose, onCreated }: Props = $props();

  // Phase: 'create' or 'setup'
  let phase = $state<'create' | 'setup'>('create');

  // Create form state
  let userId = $state('');
  let deviceName = $state('');
  let deviceType = $state<DeviceType>('kobo');
  let loading = $state(false);

  // Created device info (for setup phase)
  let createdDevice = $state<{
    device_id: string;
    device_token: string;
    sync_url: string;
    device_type: DeviceType;
  } | null>(null);

  let kindleSection: KindleAccountSection | undefined = $state();

  // Auto-load kindle section when it becomes available in setup phase
  $effect(() => {
    if (phase === 'setup' && deviceType === 'kindle' && kindleSection) {
      kindleSection.load();
    }
  });

  const userOptions = $derived<SearchSelectItem[]>(
    users.map((user) => ({ value: user.id, label: user.email }))
  );

  const title = $derived(
    phase === 'create' ? 'Add Device' : `Setup ${deviceType === 'kobo' ? 'Kobo' : 'Kindle'} Device`
  );

  async function handleCreate() {
    if (!userId) {
      showMessage('Please select a user');
      return;
    }
    loading = true;
    try {
      const result = await api.createDevice(userId, deviceName || undefined, deviceType);
      createdDevice = {
        ...result,
        device_type: deviceType
      };
      phase = 'setup';
    } catch {
      showMessage('Failed to create device');
    } finally {
      loading = false;
    }
  }

  async function handleCopyUrl() {
    if (!createdDevice) return;
    try {
      await navigator.clipboard.writeText(createdDevice.sync_url);
      showMessage('Sync URL copied');
    } catch {
      showMessage('Failed to copy');
    }
  }

  function handleClose() {
    // If a device was created, refresh the list before closing
    if (createdDevice) {
      onCreated();
    }
    onClose();
  }
</script>

<Modal {title} onClose={handleClose} width="420px" overflowVisible={phase === 'create'}>
  {#if phase === 'create'}
    <form
      class="create-form"
      onsubmit={(e) => {
        e.preventDefault();
        handleCreate();
      }}
    >
      <label>
        <span class="label-text">Owner</span>
        <SearchSelect bind:value={userId} items={userOptions} placeholder="Select user..." />
      </label>

      <div class="field">
        <span class="label-text">Device Type</span>
        <div class="radio-group">
          <label class="radio-option">
            <input type="radio" bind:group={deviceType} value="kobo" />
            <span>Kobo</span>
          </label>
          <label class="radio-option">
            <input type="radio" bind:group={deviceType} value="kindle" />
            <span>Kindle</span>
          </label>
          <label class="radio-option">
            <input type="radio" bind:group={deviceType} value="generic" />
            <span>Generic</span>
          </label>
        </div>
      </div>

      <label>
        <span class="label-text">Name (optional)</span>
        <input type="text" bind:value={deviceName} placeholder="Kobo Libra 2" />
      </label>

      <div class="actions">
        <button class="btn" type="submit" disabled={loading || !userId}>
          {loading ? 'Creating...' : 'Create'}
        </button>
      </div>
    </form>
  {:else if phase === 'setup' && createdDevice}
    <div class="setup-content">
      <div class="success-header">
        <span class="success-icon">&#10003;</span>
        <span>Device created!</span>
      </div>

      {#if createdDevice.device_type === 'kobo'}
        <div class="setup-section">
          <p class="setup-intro">
            To connect your Kobo, add this URL to your device configuration:
          </p>

          <div class="url-field">
            <span class="label-text">Sync URL</span>
            <div class="url-row">
              <code class="url-display">{createdDevice.sync_url}</code>
              <button class="btn alt small" type="button" onclick={handleCopyUrl}>Copy</button>
            </div>
          </div>

          <div class="instructions">
            <div class="instructions-title">Instructions:</div>
            <ol>
              <li>Connect Kobo to your computer via USB</li>
              <li>
                Open <code>.kobo/Kobo/Kobo eReader.conf</code>
              </li>
              <li>
                Add or edit the <code>[OneStoreServices]</code> section
              </li>
              <li>
                Set <code>api_endpoint</code> to the URL above
              </li>
              <li>Save, eject, and sync your device</li>
            </ol>
          </div>
        </div>
      {:else if createdDevice.device_type === 'kindle'}
        <div class="setup-section">
          <p class="setup-intro">Connect your Amazon account to send books to Kindle devices.</p>
          <KindleAccountSection bind:this={kindleSection} deviceId={createdDevice.device_id} />
        </div>
      {:else}
        <div class="setup-section">
          <p class="setup-intro">Your generic device is ready to use.</p>
          <p class="muted">
            Use the sync URL to configure your device or application to connect to your library.
          </p>
          <div class="url-field">
            <span class="label-text">Sync URL</span>
            <div class="url-row">
              <code class="url-display">{createdDevice.sync_url}</code>
              <button class="btn alt small" type="button" onclick={handleCopyUrl}>Copy</button>
            </div>
          </div>
        </div>
      {/if}

      <div class="actions">
        <button class="btn" type="button" onclick={handleClose}>Done</button>
      </div>
    </div>
  {/if}
</Modal>

<style>
  .create-form {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .label-text {
    font-size: 12px;
    color: var(--muted);
    font-weight: 500;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .radio-group {
    display: flex;
    gap: 16px;
  }

  .radio-option {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    cursor: pointer;
    flex-direction: row;
  }

  .radio-option input[type='radio'] {
    accent-color: var(--accent);
    width: 16px;
    height: 16px;
  }

  input[type='text'] {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    margin-top: 8px;
  }

  .setup-content {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .success-header {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 600;
    color: var(--accent);
    font-size: 14px;
  }

  .success-icon {
    font-size: 18px;
  }

  .setup-section {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .setup-intro {
    font-size: 13px;
    color: var(--ink);
    margin: 0;
  }

  .url-field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .url-row {
    display: flex;
    gap: 8px;
    align-items: stretch;
  }

  .url-display {
    flex: 1;
    background: var(--bg);
    padding: 8px 10px;
    border-radius: 8px;
    font-size: 11px;
    word-break: break-all;
    border: 1px solid var(--ring);
    color: var(--muted);
    font-family: monospace;
  }

  .instructions {
    background: var(--bg);
    border-radius: 10px;
    padding: 12px;
    font-size: 12px;
  }

  .instructions-title {
    font-weight: 600;
    margin-bottom: 8px;
    color: var(--ink);
  }

  .instructions ol {
    margin: 0;
    padding-left: 20px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .instructions li {
    color: var(--muted);
  }

  .instructions code {
    background: var(--card);
    padding: 1px 4px;
    border-radius: 4px;
    font-size: 11px;
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

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .muted {
    color: var(--muted);
    font-size: 12px;
  }
</style>
