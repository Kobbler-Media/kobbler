<script lang="ts">
  import type { LivetvGuideEntry } from '$lib/types';

  interface Props {
    entry: LivetvGuideEntry;
    tint: string;
    left: number;
    width: number;
    live: boolean;
    selected: boolean;
    onSelect: (entry: LivetvGuideEntry) => void;
  }

  let { entry, tint, left, width, live, selected, onSelect }: Props = $props();

  const meta = $derived(
    [entry.episode_number, entry.genres?.[0]].filter((v): v is string => !!v).join(' · ')
  );
  const narrow = $derived(width < 38);
  const showMeta = $derived(width > 100 && meta.length > 0);
  const showDot = $derived(width > 72);
</script>

<button
  type="button"
  class="programme"
  class:live
  class:selected
  class:narrow
  style:left="{left}px"
  style:width="{Math.max(width - 2, 4)}px"
  title={entry.title}
  onclick={() => onSelect(entry)}
>
  {#if !narrow}
    <span class="title-row">
      {#if showDot}
        <span class="dot" style:background={tint}></span>
      {/if}
      <span class="title">{entry.title}</span>
      {#if entry.recording}
        <span class="rec" title={`Recording: ${entry.recording.status}`}></span>
      {/if}
    </span>
    {#if showMeta}
      <span class="meta">{meta}</span>
    {/if}
  {/if}
</button>

<style>
  .programme {
    position: absolute;
    top: 3px;
    height: 39px;
    box-sizing: border-box;
    border-radius: 6px;
    padding: 0 9px;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 1px;
    overflow: hidden;
    text-align: left;
    cursor: pointer;
    background: rgba(255, 255, 255, 0.06);
    border: 1px solid var(--ring);
    color: var(--ink);
  }

  :global([data-theme='light']) .programme {
    background: rgba(0, 0, 0, 0.04);
  }

  .programme.narrow {
    padding: 0;
    border-radius: 5px;
  }

  .programme:hover {
    border-color: var(--muted);
  }

  .programme.live {
    background: var(--accent-dim);
    border-color: var(--accent);
  }

  .programme.selected {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }

  .title-row {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11.5px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
  }

  .programme.live .title-row {
    font-weight: 600;
  }

  .title {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .dot {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .rec {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--danger);
    flex-shrink: 0;
  }

  .meta {
    font-size: 10px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
