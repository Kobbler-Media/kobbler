<script lang="ts">
  import type { IptvChannel } from '#lib/types.js';
  import ChannelCard from './ChannelCard.svelte';

  interface Props {
    channels: IptvChannel[];
    selectedChannelId?: string | null;
    onSelect: (channel: IptvChannel) => void;
  }

  let { channels, selectedChannelId = null, onSelect }: Props = $props();
</script>

{#if channels.length === 0}
  <div class="empty-state">
    <span class="material-icons-outlined empty-icon">live_tv</span>
    <p>No channels found</p>
  </div>
{:else}
  <div class="channel-grid">
    {#each channels as channel (channel.id)}
      <ChannelCard {channel} selected={selectedChannelId === channel.id} {onSelect} />
    {/each}
  </div>
{/if}

<style>
  .channel-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
    gap: 12px;
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 48px 24px;
    color: var(--muted);
    text-align: center;
  }

  .empty-icon {
    font-size: 48px;
    opacity: 0.3;
  }

  .empty-state p {
    font-size: 14px;
    margin: 0;
  }

  @media (max-width: 768px) {
    .channel-grid {
      grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
      gap: 10px;
    }
  }
</style>
