<script lang="ts">
  import { untrack } from 'svelte';
  import type { DeviceSummary } from '#lib/types.js';
  import { Modal } from '@koblibs/ui';

  interface Props {
    devices: DeviceSummary[];
    bookCount: number;
    loading: boolean;
    onClose: () => void;
    onSend: (deviceIds: string[]) => Promise<void>;
  }

  let { devices, bookCount, loading, onClose, onSend }: Props = $props();

  // Pre-select all devices by default (untrack to capture initial value)
  let selectedDeviceIds = $state<string[]>(untrack(() => devices.map((d) => d.id)));

  const toggleDevice = (deviceId: string) => {
    if (selectedDeviceIds.includes(deviceId)) {
      selectedDeviceIds = selectedDeviceIds.filter((id) => id !== deviceId);
    } else {
      selectedDeviceIds = [...selectedDeviceIds, deviceId];
    }
  };

  const handleSend = async () => {
    await onSend(selectedDeviceIds);
  };
</script>

<Modal title="Send to Device" {onClose}>
  <p class="modal-description">
    Select which devices should receive {bookCount === 1
      ? 'this book'
      : `these ${bookCount} books`}:
  </p>
  <div class="device-list">
    {#each devices as device (device.id)}
      <label class="device-checkbox">
        <input
          type="checkbox"
          checked={selectedDeviceIds.includes(device.id)}
          onchange={() => toggleDevice(device.id)}
        />
        <span class="device-name">{device.name || 'Unnamed device'}</span>
      </label>
    {/each}
  </div>

  {#snippet footer()}
    <button class="btn alt" type="button" onclick={onClose}>Cancel</button>
    <button
      class="btn"
      type="button"
      onclick={handleSend}
      disabled={loading || selectedDeviceIds.length === 0}
    >
      Send to {selectedDeviceIds.length} device{selectedDeviceIds.length !== 1 ? 's' : ''}
    </button>
  {/snippet}
</Modal>

<style>
  .modal-description {
    margin: 0 0 16px;
    font-size: 13px;
    color: var(--muted);
  }

  .device-list {
    display: grid;
    gap: 8px;
  }

  .device-checkbox {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    background: var(--bg);
    border-radius: 10px;
    cursor: pointer;
    transition: background 0.15s;
  }

  .device-checkbox:hover {
    background: var(--ring);
  }

  .device-checkbox input[type='checkbox'] {
    width: 16px;
    height: 16px;
    accent-color: var(--accent);
  }

  .device-name {
    font-size: 13px;
    color: var(--ink);
  }

  .btn {
    border-radius: 999px;
    padding: 8px 14px;
    border: 1px solid #3b2b1f;
    background: var(--accent);
    color: #1d130c;
    font-size: 12px;
    letter-spacing: 0.6px;
    text-transform: uppercase;
    cursor: pointer;
    transition:
      transform 0.12s ease,
      box-shadow 0.12s ease;
  }

  .btn:disabled {
    cursor: not-allowed;
    opacity: 0.6;
  }

  .btn:not(:disabled):hover {
    transform: translateY(-1px);
    box-shadow: 0 6px 14px rgba(0, 0, 0, 0.2);
  }

  .btn.alt {
    background: var(--accent-2);
    color: #e7f4f2;
    border-color: #1d3a38;
  }
</style>
