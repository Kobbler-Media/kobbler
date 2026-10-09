<script lang="ts">
  import { fade } from 'svelte/transition';
  import type { IngestQueueGroup } from '$lib/types';
  import { ingestStatusChip } from '$lib/utils/chip-mappings';
  import IconChip from '$lib/components/shared/IconChip.svelte';

  interface Props {
    group: IngestQueueGroup;
    selected: boolean;
    selectedItemId: string | null;
    multiSelectMode: boolean;
    selectedIds: Set<string>;
    onSelect: (groupKey: string, itemId: string, event?: MouseEvent | KeyboardEvent) => void;
  }

  let {
    group,
    selected,
    selectedItemId: _selectedItemId,
    multiSelectMode,
    selectedIds,
    onSelect
  }: Props = $props();

  // Use the primary item for display
  let primaryItem = $derived(
    group.items.find((i) => i.id === group.primary_item_id) || group.items[0]
  );
  let title = $derived(primaryItem.title || primaryItem.ingest_info?.file_name || 'Untitled');
  let authors = $derived(
    primaryItem.authors?.length > 0 ? primaryItem.authors.map((a) => a.name).join(', ') : null
  );
  let coverUrl = $derived(
    primaryItem.cover_url
      ? `${primaryItem.cover_url}?t=${encodeURIComponent(primaryItem.updated_at || '')}`
      : null
  );
  let series = $derived(primaryItem.series_name);
  let seriesIndex = $derived(primaryItem.series_index);

  // In multi-select mode, check if any items in this group are selected
  let multiSelectActive = $derived(
    multiSelectMode && group.items.some((item) => selectedIds.has(item.id))
  );

  // Determine status badge
  const statusBadge = $derived.by(() => {
    const status: string = group.display_status;
    switch (status) {
      case 'pending':
        return { label: 'REVIEW', class: 'review' };
      case 'approved':
        return { label: 'MATCHED', class: 'matched' };
      case 'extracting':
        return { label: 'EXTRACTING', class: 'extracting' };
      case 'converting':
        return { label: 'CONVERTING', class: 'converting' };
      case 'error':
        return { label: 'ERROR', class: 'error' };
      case 'rejected':
        return { label: 'REJECTED', class: 'rejected' };
      default:
        return { label: status.toUpperCase(), class: 'default' };
    }
  });

  function handleClick(event: MouseEvent | KeyboardEvent) {
    onSelect(group.group_key, group.primary_item_id, event);
  }
</script>

<button
  class:selected
  class:multi-select-active={multiSelectActive}
  class="card"
  type="button"
  onclick={handleClick}
  in:fade={{ duration: 220 }}
>
  <div class="cover" style={coverUrl ? `background-image: url(${coverUrl});` : ''}>
    <span class="chip-pos">
      <IconChip chip={ingestStatusChip(statusBadge.class)} />
    </span>
  </div>
  <div class="meta">
    <strong class="title">{title}</strong>
    <div class="author">
      {#if authors}
        {authors}
      {:else}
        <em>Unknown author</em>
      {/if}
    </div>
    {#if series}
      <div class="series">
        {series}{#if seriesIndex != null}{` #${seriesIndex}`}{/if}
      </div>
    {/if}
  </div>
</button>

<style>
  .card {
    appearance: none;
    text-align: left;
    color: inherit;
    font: inherit;
    cursor: pointer;
    border-radius: 12px;
    padding: 10px;
    background: var(--card);
    border: 1px solid var(--ring);
    display: flex;
    flex-direction: column;
    gap: 8px;
    position: relative;
    overflow: hidden;
    transition:
      transform 0.15s ease,
      box-shadow 0.15s ease;
  }

  .card:hover {
    transform: translateY(-3px);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
  }

  :global([data-theme='light']) .card:hover {
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.12);
  }

  .card.selected {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-dim);
  }

  .card.multi-select-active {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-dim);
    opacity: 0.85;
  }

  .cover {
    aspect-ratio: 2 / 3;
    width: 100%;
    border-radius: 12px;
    background: linear-gradient(140deg, #2e323a, #1a1d23);
    background-size: cover;
    background-position: center;
    position: relative;
    overflow: hidden;
    border: 1px solid var(--ring);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
  }

  :global([data-theme='light']) .cover {
    background: linear-gradient(140deg, #e5e0d8, #d5cfc5);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.12);
  }

  .chip-pos {
    position: absolute;
    top: 8px;
    right: 8px;
  }

  .meta {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .title {
    font-size: 14px;
    font-weight: 700;
    line-height: 1.3;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .author {
    color: var(--muted);
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .author em {
    opacity: 0.7;
  }

  .series {
    color: var(--accent);
    font-size: 11px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    margin-top: 2px;
  }
</style>
