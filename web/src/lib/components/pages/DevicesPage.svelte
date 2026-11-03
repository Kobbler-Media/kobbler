<script lang="ts">
  import type {
    DeviceSummary,
    UserSummary,
    LoadState,
    DeviceType,
    DeviceActivity
  } from '#lib/types.js';
  import { ExpandableSidebar } from '@koblibs/ui';
  import DeviceDetailSidebar from '#lib/components/DeviceDetailSidebar.svelte';
  import AddDeviceModal from '#lib/components/AddDeviceModal.svelte';
  import { createDelayedLoading } from '#lib/utils/delayed-loading.svelte.js';
  import { pluralize } from '#lib/utils/format.js';
  import * as api from '#lib/api/client.js';

  const DEVICE_TYPE_LABELS: Record<DeviceType, string> = {
    kobo: 'Kobo',
    kindle: 'Kindle',
    generic: 'Generic',
    web: 'Web Reader'
  };

  type SelectDeviceHandler = (deviceId: string | null) => void;
  type UpdateDeviceHandler = (
    deviceId: string,
    name: string,
    koboApiUrl: string,
    autoSyncNewBooks: boolean
  ) => Promise<void>;
  type RotateTokenHandler = (deviceId: string) => Promise<void>;
  type ResyncDeviceHandler = (deviceId: string) => Promise<void>;
  type DeleteDeviceHandler = (deviceId: string) => Promise<void>;
  type CopyTextHandler = (value: string, label: string) => Promise<void>;
  type RefreshDevicesHandler = () => Promise<void>;

  interface Props {
    devices: DeviceSummary[];
    users: UserSummary[];
    loading: boolean;
    loadStateDevices: LoadState;
    loadStateUsers: LoadState;
    onSelectDevice: SelectDeviceHandler;
    onUpdateDevice: UpdateDeviceHandler;
    onRotateToken: RotateTokenHandler;
    onResyncDevice: ResyncDeviceHandler;
    onDeleteDevice: DeleteDeviceHandler;
    onCopyText: CopyTextHandler;
    onRefreshDevices: RefreshDevicesHandler;
  }

  let {
    devices,
    users,
    loading,
    loadStateDevices,
    loadStateUsers,
    onSelectDevice,
    onUpdateDevice,
    onRotateToken,
    onResyncDevice,
    onDeleteDevice,
    onCopyText,
    onRefreshDevices
  }: Props = $props();

  const initialLoadingState = createDelayedLoading(false);
  $effect(() => {
    initialLoadingState.loading =
      loadStateDevices.status === 'loading' || loadStateUsers.status === 'loading';
  });

  // Modal state
  let showAddDeviceModal = $state(false);

  // Sidebar state
  let selectedDeviceId = $state<string | null>(null);
  let sidebarExpanded = $state(false);
  let editMode = $state(false);
  let wasExpandedBeforeEdit = $state(false);

  // Activity state
  let deviceActivity = $state<DeviceActivity | null>(null);
  let activityLoading = $state(false);

  // Reference to sidebar component for getting form values
  let sidebarRef: DeviceDetailSidebar | undefined = $state();

  const selectedDevice = $derived(
    selectedDeviceId ? devices.find((d) => d.id === selectedDeviceId) : null
  );

  // Load activity when device is selected
  $effect(() => {
    if (selectedDeviceId) {
      loadDeviceActivity(selectedDeviceId);
    } else {
      deviceActivity = null;
    }
  });

  async function loadDeviceActivity(deviceId: string) {
    activityLoading = true;
    try {
      deviceActivity = await api.getDeviceActivity(deviceId);
    } catch (e) {
      console.error('Failed to load device activity:', e);
      deviceActivity = null;
    } finally {
      activityLoading = false;
    }
  }

  const handleSelectDevice = (deviceId: string | null) => {
    if (deviceId && selectedDeviceId === deviceId) {
      selectedDeviceId = null;
      editMode = false;
      onSelectDevice(null);
      return;
    }
    selectedDeviceId = deviceId;
    editMode = false;
    onSelectDevice(deviceId);
  };

  const closeSidebar = () => {
    selectedDeviceId = null;
    sidebarExpanded = false;
    editMode = false;
    onSelectDevice(null);
  };

  const handleRefresh = () => {
    if (selectedDevice?.device_type === 'kobo' && selectedDeviceId) {
      onResyncDevice(selectedDeviceId);
    }
  };

  const handleToggleEdit = () => {
    if (!editMode) {
      // Entering edit mode - remember current state and expand
      wasExpandedBeforeEdit = sidebarExpanded;
      sidebarExpanded = true;
      editMode = true;
    } else {
      // Exiting edit mode - restore previous state
      sidebarExpanded = wasExpandedBeforeEdit;
      editMode = false;
    }
  };

  const handleDeviceCreated = async () => {
    await onRefreshDevices();
  };

  const handleSaveDevice = async () => {
    if (!selectedDeviceId || !sidebarRef) return;
    const formValues = sidebarRef.getFormValues();
    await onUpdateDevice(
      selectedDeviceId,
      formValues.name,
      formValues.koboApiUrl,
      formValues.autoSyncNewBooks
    );
    editMode = false;
    sidebarExpanded = wasExpandedBeforeEdit;
  };

  const handleCancelEdit = () => {
    editMode = false;
    sidebarExpanded = wasExpandedBeforeEdit;
  };

  const handleRotateToken = async () => {
    if (selectedDeviceId) {
      await onRotateToken(selectedDeviceId);
    }
  };

  const handleDeleteDevice = async () => {
    if (!selectedDeviceId) return;
    if (!confirm('Delete this device? This cannot be undone.')) return;
    await onDeleteDevice(selectedDeviceId);
    closeSidebar();
  };
</script>

{#if showAddDeviceModal}
  <AddDeviceModal
    {users}
    onClose={() => (showAddDeviceModal = false)}
    onCreated={handleDeviceCreated}
  />
{/if}

<div class="page-layout">
  <div class="library-main">
    {#if initialLoadingState.showLoading}
      <div class="loading-card">Loading devices...</div>
    {/if}
    {#if loadStateDevices.status === 'error' || loadStateUsers.status === 'error'}
      <div class="error">{loadStateDevices.error || loadStateUsers.error}</div>
    {/if}
    <div class="section-header">
      <h2>Devices</h2>
      <div class="section-row">
        <span>{pluralize(devices.length, 'device')}</span>
        <button class="btn" type="button" onclick={() => (showAddDeviceModal = true)}>
          Add Device
        </button>
      </div>
    </div>
    {#if devices.length === 0}
      <div class="empty-state full-width">
        <div class="empty-icon">
          <svg viewBox="0 0 24 24" width="48" height="48" fill="currentColor">
            <path
              d="M17 1.01L7 1c-1.1 0-2 .9-2 2v18c0 1.1.9 2 2 2h10c1.1 0 2-.9 2-2V3c0-1.1-.9-1.99-2-1.99zM17 19H7V5h10v14z"
            ></path>
          </svg>
        </div>
        <h3>No devices yet</h3>
        <p class="muted">Create a device to start syncing books to your e-reader.</p>
        <button class="btn" type="button" onclick={() => (showAddDeviceModal = true)}>
          Add Device
        </button>
      </div>
    {:else}
      <div class="device-grid">
        {#each devices as device (device.id)}
          <button
            class="device-card"
            class:selected={selectedDeviceId === device.id}
            type="button"
            onclick={() => handleSelectDevice(device.id)}
          >
            <div class="device-name">{device.name || 'Unnamed device'}</div>
            <div class="device-type-badge">
              {DEVICE_TYPE_LABELS[device.device_type] || device.device_type}
            </div>
            <div class="device-owner muted">
              {users.find((u) => u.id === device.user_id)?.email || 'Unknown user'}
            </div>
            <div class="device-sync muted">
              {device.last_sync_at
                ? new Date(device.last_sync_at).toLocaleDateString()
                : 'Never synced'}
            </div>
          </button>
        {/each}
      </div>
    {/if}
  </div>

  <ExpandableSidebar
    open={selectedDeviceId !== null}
    expanded={sidebarExpanded}
    title={selectedDevice?.name || 'Unnamed device'}
    showRefresh={selectedDevice?.device_type === 'kobo' && !editMode}
    showEdit={true}
    {editMode}
    onClose={closeSidebar}
    onRefresh={handleRefresh}
    onToggleEdit={handleToggleEdit}
  >
    {#if selectedDevice}
      <DeviceDetailSidebar
        bind:this={sidebarRef}
        device={selectedDevice}
        {users}
        activity={deviceActivity}
        {activityLoading}
        {editMode}
        {loading}
        {onCopyText}
        onRotateToken={handleRotateToken}
      />
    {:else}
      <div class="sidebar-empty">
        <div class="sidebar-empty-icon">
          <svg viewBox="0 0 24 24" width="32" height="32" fill="currentColor">
            <path
              d="M17 1.01L7 1c-1.1 0-2 .9-2 2v18c0 1.1.9 2 2 2h10c1.1 0 2-.9 2-2V3c0-1.1-.9-1.99-2-1.99zM17 19H7V5h10v14z"
            ></path>
          </svg>
        </div>
        <div class="sidebar-empty-title">Select a Device</div>
        <p class="muted">Click on a device to view its details and configuration.</p>
      </div>
    {/if}

    {#snippet footer()}
      {#if editMode}
        <div class="form-footer">
          <button class="btn-danger" type="button" onclick={handleDeleteDevice} disabled={loading}>
            Delete Device
          </button>
          <div class="spacer"></div>
          <button class="btn alt" type="button" onclick={handleCancelEdit} disabled={loading}>
            Cancel
          </button>
          <button class="btn" type="button" onclick={handleSaveDevice} disabled={loading}>
            {loading ? 'Saving...' : 'Save'}
          </button>
        </div>
      {/if}
    {/snippet}
  </ExpandableSidebar>
</div>

<style>
  .loading-card,
  .error {
    padding: 24px;
    text-align: center;
    border-radius: 16px;
    border: 1px solid var(--ring);
    background: var(--card);
  }

  .error {
    color: var(--danger);
    border-color: var(--danger);
  }

  .section-header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    margin-bottom: 16px;
  }

  .section-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .section-row {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .section-row span {
    font-size: 12px;
    color: var(--muted);
  }

  .empty-state {
    padding: 24px;
    border-radius: 16px;
    border: 1px dashed var(--ring);
    background: var(--card);
    text-align: center;
  }

  .empty-state h3 {
    margin: 0 0 8px;
    font-size: 16px;
  }

  .empty-state p {
    margin: 0 0 16px;
  }

  .empty-state.full-width {
    padding: 48px 24px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
  }

  .empty-icon {
    color: var(--muted);
    opacity: 0.5;
    margin-bottom: 8px;
  }

  .device-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
    gap: 14px;
  }

  .device-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 14px;
    padding: 14px;
    text-align: left;
    cursor: pointer;
    transition:
      border-color 0.15s,
      box-shadow 0.15s;
    display: grid;
    gap: 6px;
  }

  .device-card:hover {
    border-color: var(--accent);
  }

  .device-card.selected {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent);
  }

  .device-name {
    font-weight: 600;
    color: var(--ink);
  }

  .device-type-badge {
    font-size: 11px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .device-owner,
  .device-sync {
    font-size: 12px;
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

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .sidebar-empty {
    text-align: center;
    padding: 24px 16px;
  }

  .sidebar-empty-icon {
    color: var(--muted);
    margin-bottom: 12px;
    opacity: 0.5;
  }

  .sidebar-empty-title {
    font-size: 14px;
    font-weight: 600;
    margin-bottom: 8px;
  }

  .sidebar-empty p {
    font-size: 12px;
    margin: 0;
  }

  .form-footer {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .spacer {
    flex: 1;
  }

  .btn-danger {
    padding: 8px 16px;
    border: 1px solid var(--danger);
    border-radius: 12px;
    background: transparent;
    color: var(--danger);
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
  }

  .btn-danger:hover:not(:disabled) {
    background: var(--danger);
    color: white;
  }

  .btn-danger:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .muted {
    color: var(--muted);
  }
</style>
