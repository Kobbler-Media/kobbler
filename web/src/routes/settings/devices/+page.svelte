<script lang="ts">
  import { onMount } from 'svelte';
  import DevicesPage from '#lib/components/pages/DevicesPage.svelte';
  import { getLibraryState } from '#lib/stores/library.svelte.js';
  import { getDevicesState } from '#lib/stores/devices.svelte.js';
  import { loadDevices, loadUsers, showMessage, copyText } from '#lib/context/app.svelte.js';
  import * as api from '#lib/api/client.js';

  // Store state
  const library = getLibraryState();
  const devicesStore = getDevicesState();

  // Derived state
  const devices = $derived(devicesStore.devices);
  const users = $derived(devicesStore.users);
  const loadState = $derived(library.loadState);

  // Local state
  let loading = $state(false);

  onMount(async () => {
    await Promise.all([loadDevices(), loadUsers()]);
  });

  // Handlers
  const handleSelectDevice = (_deviceId: string | null) => {
    // Device selection is handled internally by the component
  };

  const handleUpdateDevice = async (
    deviceId: string,
    name: string,
    koboApiUrl: string,
    autoSyncNewBooks: boolean
  ): Promise<void> => {
    loading = true;
    try {
      await api.updateDevice(deviceId, {
        name,
        kobo_api_url: koboApiUrl || undefined,
        auto_sync_new_books: autoSyncNewBooks
      });
      showMessage('Device updated');
      await loadDevices();
    } catch {
      showMessage('Update device failed');
    } finally {
      loading = false;
    }
  };

  const handleRotateToken = async (deviceId: string): Promise<void> => {
    loading = true;
    try {
      await api.rotateDeviceToken(deviceId);
      showMessage('Token rotated');
      await loadDevices();
    } catch {
      showMessage('Rotate token failed');
    } finally {
      loading = false;
    }
  };

  const handleResyncDevice = async (deviceId: string) => {
    loading = true;
    try {
      await api.resyncDevice(deviceId);
      showMessage('Device resync queued');
    } catch {
      showMessage('Resync failed');
    } finally {
      loading = false;
    }
  };

  const handleDeleteDevice = async (deviceId: string) => {
    loading = true;
    try {
      await api.deleteDevice(deviceId);
      showMessage('Device deleted');
      await loadDevices();
    } catch {
      showMessage('Delete device failed');
    } finally {
      loading = false;
    }
  };

  const handleCopyText = async (value: string, label: string) => {
    await copyText(value, label);
  };

  const handleRefreshDevices = async () => {
    await loadDevices();
  };
</script>

<svelte:head>
  <title>Device Settings — Kobbler</title>
</svelte:head>

<DevicesPage
  {devices}
  {users}
  {loading}
  loadStateDevices={loadState.devices}
  loadStateUsers={loadState.users}
  onSelectDevice={handleSelectDevice}
  onUpdateDevice={handleUpdateDevice}
  onRotateToken={handleRotateToken}
  onResyncDevice={handleResyncDevice}
  onDeleteDevice={handleDeleteDevice}
  onCopyText={handleCopyText}
  onRefreshDevices={handleRefreshDevices}
/>
