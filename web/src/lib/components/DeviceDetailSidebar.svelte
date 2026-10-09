<script lang="ts">
  import { untrack } from 'svelte';
  import type { DeviceSummary, UserSummary, DeviceType, DeviceActivity } from '$lib/types';
  import MiniBookCard from './MiniBookCard.svelte';
  import KindleAccountSection from './KindleAccountSection.svelte';
  import { formatDistanceToNow } from '$lib/utils/format';

  const DEVICE_TYPE_LABELS: Record<DeviceType, string> = {
    kobo: 'Kobo',
    kindle: 'Kindle',
    generic: 'Generic',
    web: 'Web Reader'
  };

  interface Props {
    device: DeviceSummary;
    users: UserSummary[];
    activity: DeviceActivity | null;
    activityLoading: boolean;
    editMode: boolean;
    loading: boolean;
    onCopyText: (value: string, label: string) => Promise<void>;
    onRotateToken: () => Promise<void>;
  }

  let {
    device,
    users,
    activity,
    activityLoading,
    editMode,
    loading,
    onCopyText,
    onRotateToken
  }: Props = $props();

  let kindleSection: KindleAccountSection | undefined = $state();

  // Edit form state - initialize with untrack to avoid warnings
  let editingName = $state(untrack(() => device.name) || '');
  let editingKoboApiUrl = $state(untrack(() => device.kobo_api_url) || '');
  let editingAutoSync = $state(untrack(() => device.auto_sync_new_books));
  // Reset form when device changes or when entering edit mode
  $effect(() => {
    // Track device.id to reset when switching devices
    const _deviceId = device.id;
    editingName = device.name || '';
    editingKoboApiUrl = device.kobo_api_url || '';
    editingAutoSync = device.auto_sync_new_books;
  });

  // Load kindle section when device changes (view mode)
  $effect(() => {
    if (device.device_type === 'kindle' && kindleSection && !editMode) {
      kindleSection.load();
    }
  });

  // Expose form values for parent to read on save
  export function getFormValues() {
    return {
      name: editingName,
      koboApiUrl: editingKoboApiUrl,
      autoSyncNewBooks: editingAutoSync
    };
  }

  const ownerEmail = $derived(users.find((u) => u.id === device.user_id)?.email || 'Unknown user');

  const lastSyncLabel = $derived(() => {
    if (device.device_type === 'kindle') {
      const lastDelivery = activity?.last_delivery_at;
      return lastDelivery
        ? `Last delivery: ${formatDistanceToNow(lastDelivery)}`
        : 'No deliveries yet';
    }
    return device.last_sync_at
      ? `Last sync: ${formatDistanceToNow(device.last_sync_at)}`
      : 'Never synced';
  });

  const hasPendingSection = $derived(
    device.device_type === 'kobo'
      ? (activity?.pending_sync?.length ?? 0) > 0
      : (activity?.pending_delivery?.length ?? 0) > 0
  );

  const hasRecentlyReadSection = $derived((activity?.recently_read?.length ?? 0) > 0);

  const hasRecentlySyncedSection = $derived(
    device.device_type === 'kobo'
      ? (activity?.recently_synced?.length ?? 0) > 0
      : (activity?.recently_delivered?.length ?? 0) > 0
  );
</script>

<div class="sidebar-content">
  {#if editMode}
    <!-- EDIT MODE -->
    <div class="edit-form">
      <label>
        <span class="label-text">Device Name</span>
        <input type="text" bind:value={editingName} placeholder="Device name" />
      </label>

      <label class="toggle-label">
        <input type="checkbox" bind:checked={editingAutoSync} />
        <span class="label-text">Auto-sync new books</span>
        <span class="hint">Automatically add newly ingested books to this device</span>
      </label>

      {#if device.device_type === 'kobo'}
        <details class="advanced-section">
          <summary>Advanced Settings</summary>
          <label>
            <span class="label-text">Kobo Store API URL (optional)</span>
            <input
              type="text"
              bind:value={editingKoboApiUrl}
              placeholder="https://storeapi.kobo.com"
            />
            <span class="hint">Override the upstream Kobo API. Leave empty to use default.</span>
          </label>
        </details>

        <div class="token-section">
          <button class="btn alt" type="button" onclick={onRotateToken} disabled={loading}>
            Rotate Token
          </button>
          <span class="hint">Generate a new sync URL. You'll need to update your device.</span>
        </div>
      {/if}
    </div>
  {:else}
    <!-- VIEW MODE -->
    <!-- Header info -->
    <div class="device-header">
      <span class="device-owner muted">{ownerEmail}</span>
      <span class="device-type-badge">
        {DEVICE_TYPE_LABELS[device.device_type] || device.device_type}
      </span>
      <span class="device-sync muted">{lastSyncLabel()}</span>
      {#if device.auto_sync_new_books}
        <span class="auto-sync-badge">Auto-sync on</span>
      {/if}
    </div>

    <!-- Activity sections -->
    {#if activityLoading}
      <div class="loading-section">Loading activity...</div>
    {:else}
      <!-- Pending Sync/Delivery -->
      {#if hasPendingSection}
        <div class="activity-section">
          <div class="section-title">
            {device.device_type === 'kindle' ? 'Pending Delivery' : 'Pending Sync'}
            <span class="count">
              ({device.device_type === 'kindle'
                ? activity?.pending_delivery?.length
                : activity?.pending_sync?.length})
            </span>
          </div>
          <div class="book-row">
            {#if device.device_type === 'kindle'}
              {#each activity?.pending_delivery ?? [] as book (book.id)}
                <MiniBookCard id={book.id} title={book.title} coverUrl={book.cover_url} />
              {/each}
            {:else}
              {#each activity?.pending_sync ?? [] as book (book.id)}
                <MiniBookCard id={book.id} title={book.title} coverUrl={book.cover_url} />
              {/each}
            {/if}
          </div>
        </div>
      {/if}

      <!-- Recently Read -->
      {#if hasRecentlyReadSection}
        <div class="activity-section">
          <div class="section-title">Recently Read</div>
          <div class="book-row">
            {#each activity?.recently_read ?? [] as book (book.id)}
              <MiniBookCard
                id={book.id}
                title={book.title}
                coverUrl={book.cover_url}
                progress={book.progress_percent}
              />
            {/each}
          </div>
        </div>
      {/if}

      <!-- Recently Synced/Delivered -->
      {#if hasRecentlySyncedSection}
        <div class="activity-section">
          <div class="section-title">
            {device.device_type === 'kindle' ? 'Recently Delivered' : 'Recently Synced'}
          </div>
          <div class="book-row">
            {#if device.device_type === 'kindle'}
              {#each activity?.recently_delivered ?? [] as book (book.id)}
                <MiniBookCard id={book.id} title={book.title} coverUrl={book.cover_url} />
              {/each}
            {:else}
              {#each activity?.recently_synced ?? [] as book (book.id)}
                <MiniBookCard id={book.id} title={book.title} coverUrl={book.cover_url} />
              {/each}
            {/if}
          </div>
        </div>
      {/if}

      <!-- Empty state if no activity -->
      {#if !hasPendingSection && !hasRecentlyReadSection && !hasRecentlySyncedSection}
        <div class="empty-activity">
          <span class="muted">No recent activity</span>
        </div>
      {/if}
    {/if}

    <!-- Setup/Status section -->
    <div class="setup-section">
      <div class="section-title">Setup</div>

      {#if device.device_type === 'kobo'}
        <div class="setup-field">
          <span class="field-label">Sync URL</span>
          <div class="url-row">
            <code class="url-display">{device.sync_url}</code>
            <button
              class="btn alt small"
              type="button"
              onclick={() => onCopyText(device.sync_url, 'Sync URL')}
            >
              Copy
            </button>
          </div>
        </div>
      {:else if device.device_type === 'kindle'}
        <KindleAccountSection bind:this={kindleSection} deviceId={device.id} />
      {:else}
        <div class="setup-field">
          <span class="field-label">Sync URL</span>
          <div class="url-row">
            <code class="url-display">{device.sync_url}</code>
            <button
              class="btn alt small"
              type="button"
              onclick={() => onCopyText(device.sync_url, 'Sync URL')}
            >
              Copy
            </button>
          </div>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .sidebar-content {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  /* Edit mode styles */
  .edit-form {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .edit-form label {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .label-text {
    font-size: 12px;
    color: var(--muted);
    font-weight: 500;
  }

  .edit-form input[type='text'] {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .hint {
    font-size: 11px;
    color: var(--muted);
    margin-top: -8px;
  }

  .advanced-section {
    padding: 12px;
    border-radius: 10px;
    background: var(--bg);
    margin-top: 4px;
  }

  .advanced-section summary {
    cursor: pointer;
    font-size: 12px;
    color: var(--muted);
    margin-bottom: 8px;
  }

  .advanced-section[open] summary {
    margin-bottom: 12px;
  }

  .token-section {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-top: 8px;
    border-top: 1px solid var(--ring);
  }

  .token-section .btn {
    align-self: flex-start;
  }

  /* View mode styles */
  .device-header {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .device-owner {
    font-size: 12px;
    color: var(--muted);
  }

  .device-type-badge {
    font-size: 11px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .device-sync {
    font-size: 12px;
    color: var(--muted);
  }

  .loading-section {
    padding: 20px;
    text-align: center;
    color: var(--muted);
    font-size: 12px;
  }

  .activity-section {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .section-title {
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--muted);
  }

  .section-title .count {
    font-weight: 500;
  }

  .book-row {
    display: flex;
    gap: 10px;
    overflow-x: auto;
    padding-bottom: 4px;
  }

  .book-row::-webkit-scrollbar {
    height: 4px;
  }

  .book-row::-webkit-scrollbar-track {
    background: var(--bg);
    border-radius: 2px;
  }

  .book-row::-webkit-scrollbar-thumb {
    background: var(--ring);
    border-radius: 2px;
  }

  .empty-activity {
    padding: 20px;
    text-align: center;
    background: var(--bg);
    border-radius: 10px;
  }

  .setup-section {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding-top: 12px;
    border-top: 1px solid var(--ring);
  }

  .setup-field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .field-label {
    font-size: 12px;
    color: var(--muted);
    font-weight: 500;
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
  }

  .toggle-label {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .toggle-label input[type='checkbox'] {
    width: 16px;
    height: 16px;
    accent-color: var(--accent);
  }

  .toggle-label .hint {
    width: 100%;
    margin-top: -4px;
    padding-left: 24px;
  }

  .auto-sync-badge {
    font-size: 11px;
    color: var(--accent);
    font-weight: 500;
  }
</style>
