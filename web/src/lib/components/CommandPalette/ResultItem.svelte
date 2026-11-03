<script lang="ts">
  import type { SearchResult } from './types';
  import { resultTypeChip, resultMetaChip } from '#lib/utils/chip-mappings.js';
  import IconChip from '#lib/components/shared/IconChip.svelte';

  interface Props {
    result: SearchResult;
    selected: boolean;
    onSelect: () => void;
  }

  let { result, selected, onSelect }: Props = $props();

  let buttonRef: HTMLButtonElement | undefined = $state();

  // Scroll into view when selected
  $effect(() => {
    if (selected && buttonRef) {
      buttonRef.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    }
  });

  function getTypeLabel(type: SearchResult['type']): string {
    switch (type) {
      case 'book':
        return 'Book';
      case 'ingest':
        return 'Ingest';
      case 'author':
        return 'Author';
      case 'series':
        return 'Series';
      case 'page':
        return 'Page';
      case 'command':
        return 'Command';
      case 'section':
        return 'Section';
      case 'movie':
        return 'Movie';
      case 'tv_show':
        return 'TV Show';
      case 'tv_season':
        return 'Season';
      case 'tv_episode':
        return 'Episode';
      case 'external_search':
        return 'Search';
      case 'transcript':
        return 'Transcript';
      case 'iptv_channel':
        return 'Channel';
      case 'livetv_channel':
        return 'Live TV';
      case 'setting':
        return 'Setting';
    }
  }
</script>

<button bind:this={buttonRef} class="result-item" class:selected onclick={onSelect} type="button">
  <div class="result-icon">
    {#if result.coverUrl}
      <img
        src={result.coverUrl}
        alt=""
        class="cover-image"
        onerror={(e) => {
          (e.currentTarget as HTMLImageElement).style.display = 'none';
        }}
      />
    {:else if result.imageUrl}
      <img
        src={result.imageUrl}
        alt=""
        class="author-image"
        onerror={(e) => {
          (e.currentTarget as HTMLImageElement).style.display = 'none';
        }}
      />
    {:else if result.icon}
      <svg viewBox="0 0 24 24" width="18" height="18" fill="currentColor">
        <path d={result.icon}></path>
      </svg>
    {:else}
      {@const chip = resultTypeChip(result.type)}
      <IconChip icon={chip.icon} label={chip.label} color={chip.color} />
    {/if}
  </div>

  <div class="result-content">
    <span class="result-label">{result.label}</span>
    {#if result.description}
      <span class="result-description">{result.description}</span>
    {/if}
  </div>

  <div class="result-meta">
    {#if result.isMonitored}
      {@const chip = resultMetaChip('monitored')}
      {#if chip}<IconChip icon={chip.icon} label={chip.label} color={chip.color} />{/if}
    {/if}
    {#if result.isInLibrary}
      {@const chip = resultMetaChip('in-library')}
      {#if chip}<IconChip icon={chip.icon} label={chip.label} color={chip.color} />{/if}
    {/if}
    {#if result.wantStatus === 'wanted'}
      {@const chip = resultMetaChip('wanted')}
      {#if chip}<IconChip icon={chip.icon} label={chip.label} color={chip.color} />{/if}
    {/if}
    {#if result.hasEbook || result.hasAudiobook}
      <span class="format-icons">
        {#if result.hasEbook}
          <span class="material-icons-outlined" title="Ebook">auto_stories</span>
        {/if}
        {#if result.hasAudiobook}
          <span class="material-icons-outlined" title="Audiobook">headphones</span>
        {/if}
      </span>
    {/if}
    <span class="type-label">{getTypeLabel(result.type)}</span>
  </div>
</button>

<style>
  .result-item {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 10px 16px;
    background: transparent;
    border: none;
    border-radius: 8px;
    cursor: pointer;
    text-align: left;
    color: var(--ink);
    transition: background 0.1s;
  }

  .result-item:hover {
    background: rgba(255, 255, 255, 0.06);
  }

  :global([data-theme='light']) .result-item:hover {
    background: rgba(0, 0, 0, 0.04);
  }

  .result-item.selected {
    background: rgba(211, 140, 90, 0.2);
  }

  :global([data-theme='light']) .result-item.selected {
    background: rgba(200, 122, 70, 0.15);
  }

  .result-icon {
    flex-shrink: 0;
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--muted);
  }

  .cover-image {
    width: 24px;
    height: 32px;
    object-fit: cover;
    border-radius: 2px;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.3);
  }

  .author-image {
    width: 32px;
    height: 32px;
    object-fit: cover;
    border-radius: 50%;
  }

  .result-content {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .result-label {
    font-size: 14px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .result-description {
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .result-meta {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .format-icons {
    display: flex;
    align-items: center;
    gap: 2px;
    color: var(--muted);
    opacity: 0.7;
  }

  .format-icons .material-icons-outlined {
    font-size: 14px;
  }

  .type-label {
    font-size: 11px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.02em;
  }
</style>
