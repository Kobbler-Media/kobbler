<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import type { TvEpisode } from '#lib/types.js';

  interface Props {
    episodes: TvEpisode[];
    currentEpisodeId: string;
    onSelectEpisode: (episode: TvEpisode) => void;
    isOpen?: boolean;
    onToggle?: () => void;
  }

  let { episodes, currentEpisodeId, onSelectEpisode, isOpen, onToggle }: Props = $props();

  let internalOpen = $state(false);
  let open = $derived(isOpen ?? internalOpen);
  let panelEl: HTMLDivElement | undefined = $state();
  let listEl: HTMLDivElement | undefined = $state();

  // Auto-scroll to current episode when panel opens
  $effect(() => {
    if (open && listEl) {
      tick().then(() => {
        const activeEl = listEl?.querySelector('.episode-item.active');
        activeEl?.scrollIntoView({ block: 'center', behavior: 'instant' });
      });
    }
  });

  function toggle() {
    if (onToggle) onToggle();
    else internalOpen = !internalOpen;
  }

  function close() {
    if (onToggle && open) onToggle();
    else internalOpen = false;
  }

  function handleClickOutside(e: MouseEvent) {
    if (panelEl && !panelEl.contains(e.target as Node)) {
      close();
    }
  }

  function formatRuntime(minutes: number | null): string {
    if (!minutes) return '';
    if (minutes >= 60) {
      const h = Math.floor(minutes / 60);
      const m = minutes % 60;
      return m > 0 ? `${h}h ${m}m` : `${h}h`;
    }
    return `${minutes}m`;
  }

  onMount(() => {
    document.addEventListener('click', handleClickOutside, true);
  });

  onDestroy(() => {
    document.removeEventListener('click', handleClickOutside, true);
  });
</script>

<div class="episode-panel" bind:this={panelEl}>
  <button class="panel-btn" type="button" onclick={toggle} aria-label="Episodes">
    <span class="material-icons-outlined">playlist_play</span>
  </button>

  {#if open}
    <div class="panel-menu">
      <div class="panel-header">Episodes</div>
      <div class="episode-list" bind:this={listEl}>
        {#each episodes as ep (ep.id)}
          {@const isCurrent = ep.id === currentEpisodeId}
          {@const available =
            ep.is_in_library || (ep.air_date != null && new Date(ep.air_date) <= new Date())}
          <button
            class="episode-item"
            class:active={isCurrent}
            class:unavailable={!available}
            type="button"
            disabled={!available || isCurrent}
            onclick={() => {
              if (available && !isCurrent) {
                close();
                onSelectEpisode(ep);
              }
            }}
          >
            <span class="ep-number">{ep.episode_sort_order}</span>
            <div class="ep-info">
              <span class="ep-title">{ep.title || `Episode ${ep.episode_sort_order}`}</span>
              {#if ep.runtime}
                <span class="ep-runtime">{formatRuntime(ep.runtime)}</span>
              {/if}
            </div>
            {#if isCurrent}
              <span class="material-icons-outlined now-playing-icon">play_arrow</span>
            {/if}
          </button>
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .episode-panel {
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
    font-size: 24px;
  }

  .panel-menu {
    position: absolute;
    bottom: 48px;
    right: 0;
    width: 360px;
    max-width: calc(100vw - 48px);
    max-height: 500px;
    display: flex;
    flex-direction: column;
    background: rgba(20, 20, 20, 0.95);
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 8px;
    z-index: 10;
    backdrop-filter: blur(12px);
  }

  .panel-header {
    padding: 10px 14px 6px;
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: rgba(255, 255, 255, 0.5);
    flex-shrink: 0;
  }

  .episode-list {
    overflow-y: auto;
    padding: 0 4px 4px;
  }

  .episode-item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 10px;
    border: none;
    background: transparent;
    color: rgba(255, 255, 255, 0.8);
    font-size: 13px;
    cursor: pointer;
    border-radius: 4px;
    text-align: left;
    transition: background 0.1s;
  }

  .episode-item:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.1);
  }

  .episode-item.active {
    color: var(--accent, #818cf8);
    font-weight: 600;
    cursor: default;
  }

  .episode-item.unavailable {
    opacity: 0.35;
    cursor: default;
  }

  .ep-number {
    font-variant-numeric: tabular-nums;
    color: rgba(255, 255, 255, 0.4);
    min-width: 24px;
    flex-shrink: 0;
    text-align: right;
  }

  .episode-item.active .ep-number {
    color: var(--accent, #818cf8);
    opacity: 0.7;
  }

  .ep-info {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
    flex: 1;
  }

  .ep-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .ep-runtime {
    font-size: 11px;
    color: rgba(255, 255, 255, 0.35);
  }

  .episode-item.active .ep-runtime {
    color: var(--accent, #818cf8);
    opacity: 0.5;
  }

  .now-playing-icon {
    font-size: 18px;
    color: var(--accent, #818cf8);
    flex-shrink: 0;
  }
</style>
