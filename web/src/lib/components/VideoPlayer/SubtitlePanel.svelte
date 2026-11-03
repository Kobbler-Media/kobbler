<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { PlaybackSubtitleTrack } from '#lib/types.js';

  interface Props {
    subtitles: PlaybackSubtitleTrack[];
    activeSubtitleId: string | null;
    onSubtitleSelect: (trackId: string | null) => void;
    onSearchSubtitles?: () => void;
    isOpen?: boolean;
    onToggle?: () => void;
  }

  let {
    subtitles,
    activeSubtitleId,
    onSubtitleSelect,
    onSearchSubtitles,
    isOpen,
    onToggle
  }: Props = $props();

  let internalOpen = $state(false);
  let open = $derived(isOpen ?? internalOpen);
  let panelEl: HTMLDivElement | undefined = $state();

  let tracks = $derived(
    subtitles.map((s) => ({
      id: s.track_id,
      label:
        ([s.title, s.language, s.codec].filter(Boolean).join(' - ') || `Track ${s.stream_index}`) +
        (s.requires_transcode ? ' (transcode)' : ''),
      active: s.track_id === activeSubtitleId
    }))
  );

  function toggle() {
    if (onToggle) onToggle();
    else internalOpen = !internalOpen;
  }

  function close() {
    if (onToggle && open) onToggle();
    else internalOpen = false;
  }

  function select(id: string | null) {
    onSubtitleSelect(id);
    close();
  }

  function handleClickOutside(e: MouseEvent) {
    if (panelEl && !panelEl.contains(e.target as Node)) {
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

<div class="subtitle-panel" bind:this={panelEl}>
  <button class="panel-btn" type="button" onclick={toggle} aria-label="Subtitles">
    <span class="material-icons-outlined">subtitles</span>
  </button>

  {#if open}
    <div class="panel-menu">
      <button
        class="panel-item"
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

      {#each tracks as track (track.id)}
        <button
          class="panel-item"
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

      {#if onSearchSubtitles}
        <div class="panel-divider"></div>
        <button
          class="panel-item search-item"
          type="button"
          onclick={() => {
            close();
            onSearchSubtitles?.();
          }}
        >
          <span class="material-icons-outlined search-icon">search</span>
          <span>Search for subtitles</span>
        </button>
      {/if}
    </div>
  {/if}
</div>

<style>
  .subtitle-panel {
    position: relative;
  }

  .panel-btn {
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

  .panel-btn:hover {
    background: rgba(255, 255, 255, 0.15);
  }

  .panel-btn .material-icons-outlined {
    font-size: 22px;
  }

  .panel-menu {
    position: absolute;
    bottom: 48px;
    right: 0;
    min-width: 240px;
    max-width: calc(100vw - 48px);
    max-height: 350px;
    overflow-y: auto;
    background: rgba(20, 20, 20, 0.95);
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 8px;
    padding: 4px 0;
    z-index: 10;
    backdrop-filter: blur(12px);
  }

  .panel-item {
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

  .panel-item:hover {
    background: rgba(255, 255, 255, 0.1);
  }

  .panel-item.active {
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

  .panel-divider {
    height: 1px;
    background: rgba(255, 255, 255, 0.1);
    margin: 4px 0;
  }

  .search-item {
    color: rgba(255, 255, 255, 0.7);
  }

  .search-item:hover {
    color: #fff;
  }

  .search-icon {
    font-size: 16px;
    color: rgba(255, 255, 255, 0.5);
  }
</style>
