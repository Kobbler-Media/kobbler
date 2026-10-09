<script lang="ts">
  import { Modal } from '@koblibs/ui';

  interface DeviceChoice {
    id: string;
    name: string;
    selected: boolean;
  }

  interface Props {
    devices: DeviceChoice[];
    disabled?: boolean;
    onToggle: (deviceId: string) => void;
    onClose: () => void;
  }

  let { devices, disabled = false, onToggle, onClose }: Props = $props();
</script>

<Modal title="Select Devices" {onClose} width="min(90vw, 360px)">
  {#if devices.length === 0}
    <p class="empty-devices">No devices available.</p>
  {:else}
    <div class="device-picker-list">
      {#each devices as device (device.id)}
        <button
          type="button"
          class="device-picker-item"
          class:selected={device.selected}
          onclick={() => onToggle(device.id)}
          {disabled}
        >
          <span class="device-name">{device.name}</span>
          {#if device.selected}
            <span class="material-icons-outlined">check</span>
          {/if}
        </button>
      {/each}
    </div>
  {/if}
</Modal>

<style>
  .empty-devices {
    margin: 0;
    font-size: 13px;
    color: var(--muted);
  }

  .device-picker-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .device-picker-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 12px;
    border: none;
    background: transparent;
    border-radius: 8px;
    cursor: pointer;
    text-align: left;
    color: var(--ink);
    font-size: 13px;
    transition: background 0.15s;
  }

  .device-picker-item:hover:not(:disabled) {
    background: var(--bg-2);
  }

  .device-picker-item:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .device-picker-item.selected {
    background: var(--accent-dim);
  }

  .device-picker-item .material-icons-outlined {
    font-size: 18px;
    color: var(--accent);
  }
</style>
