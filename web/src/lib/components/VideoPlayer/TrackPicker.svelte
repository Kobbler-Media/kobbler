<script lang="ts">
  import { onMount, onDestroy } from 'svelte';

  interface Track {
    id: string;
    label: string;
    active: boolean;
  }

  interface Props {
    tracks: Track[];
    icon: string;
    onSelect: (id: string | null) => void;
    showOff?: boolean;
    isOpen?: boolean;
    onToggle?: () => void;
  }

  let { tracks, icon, onSelect, showOff = true, isOpen, onToggle }: Props = $props();

  let internalOpen = $state(false);
  let open = $derived(isOpen ?? internalOpen);
  let menuEl: HTMLDivElement | undefined = $state();

  function toggle() {
    if (onToggle) onToggle();
    else internalOpen = !internalOpen;
  }

  function close() {
    if (onToggle && open) onToggle();
    else internalOpen = false;
  }

  function select(id: string | null) {
    onSelect(id);
    close();
  }

  function handleClickOutside(e: MouseEvent) {
    if (menuEl && !menuEl.contains(e.target as Node)) {
      close();
    }
  }

  onMount(() => {
    document.addEventListener('click', handleClickOutside, true);
  });

  onDestroy(() => {
    document.removeEventListener('click', handleClickOutside, true);
  });
</script>

<div class="track-picker" bind:this={menuEl}>
  <button class="picker-btn" type="button" onclick={toggle} aria-label="Select track">
    <span class="material-icons-outlined">{icon}</span>
  </button>

  {#if open}
    <div class="picker-menu">
      {#if showOff}
        <button
          class="picker-item"
          class:active={tracks.every((t) => !t.active)}
          type="button"
          onclick={() => select(null)}
        >
          {#if tracks.every((t) => !t.active)}
            <span class="material-icons-outlined check-icon">check</span>
          {:else}
            <span class="check-spacer"></span>
          {/if}
          <span>Off</span>
        </button>
      {/if}
      {#each tracks as track (track.id)}
        <button
          class="picker-item"
          class:active={track.active}
          type="button"
          onclick={() => select(track.id)}
        >
          {#if track.active}
            <span class="material-icons-outlined check-icon">check</span>
          {:else}
            <span class="check-spacer"></span>
          {/if}
          <span>{track.label}</span>
        </button>
      {/each}
    </div>
  {/if}
</div>

<style>
  .track-picker {
    position: relative;
  }

  .picker-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 40px;
    height: 40px;
    border-radius: 50%;
    border: none;
    background: transparent;
    color: #fff;
    cursor: pointer;
    transition: background 0.15s;
  }

  .picker-btn:hover {
    background: rgba(255, 255, 255, 0.15);
  }

  .picker-btn .material-icons-outlined {
    font-size: 22px;
  }

  .picker-menu {
    position: absolute;
    bottom: 48px;
    right: 0;
    min-width: 200px;
    max-height: 300px;
    overflow-y: auto;
    background: rgba(20, 20, 20, 0.95);
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 8px;
    padding: 4px 0;
    z-index: 10;
    backdrop-filter: blur(12px);
  }

  .picker-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 12px;
    border: none;
    background: transparent;
    color: #ccc;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
    transition: background 0.1s;
  }

  .picker-item:hover {
    background: rgba(255, 255, 255, 0.1);
  }

  .picker-item.active {
    color: #fff;
    font-weight: 600;
  }

  .check-icon {
    font-size: 16px;
    color: var(--accent, #e8a87c);
  }

  .check-spacer {
    display: inline-block;
    width: 16px;
  }
</style>
