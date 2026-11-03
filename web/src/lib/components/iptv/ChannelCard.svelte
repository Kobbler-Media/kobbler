<script lang="ts">
  import type { IptvChannel } from '#lib/types.js';
  import IconChip from '#lib/components/shared/IconChip.svelte';
  import MediaCard from '#lib/components/shared/MediaCard.svelte';

  interface Props {
    channel: IptvChannel;
    selected?: boolean;
    onSelect: (channel: IptvChannel) => void;
  }

  let { channel, selected = false, onSelect }: Props = $props();

  const sourceLabel = $derived(
    channel.source_count > 0
      ? channel.source_count === 1
        ? '1 source'
        : `${channel.source_count} sources`
      : ''
  );
</script>

<MediaCard
  posterUrl={channel.icon_url}
  placeholderIcon="live_tv"
  aspectRatio="4 / 3"
  posterFit="contain"
  posterPadding="12px"
  hidePlaceholder={!!channel.icon_url}
  {selected}
  itemId={channel.id}
  onclick={() => onSelect(channel)}
>
  {#snippet posterOverlay()}
    {#if channel.has_epg}
      <span class="chip-pos">
        <IconChip icon="schedule" label="EPG" color="var(--accent)" />
      </span>
    {/if}
    {#if channel.source_count > 0}
      <div class="format-icons">
        <span class="material-icons-outlined">cell_tower</span>
        <span class="source-count">{channel.source_count}</span>
      </div>
    {/if}
  {/snippet}
  <strong class="card-title">{channel.name}</strong>
  {#if channel.current_program}
    <div class="card-sub">{channel.current_program}</div>
  {/if}
  {#if channel.category || sourceLabel}
    <div class="card-accent">{channel.category ?? sourceLabel}</div>
  {/if}
</MediaCard>

<style>
  .source-count {
    font-size: 11px;
    font-weight: 600;
  }
</style>
