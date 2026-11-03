<script lang="ts">
  import type { DiscoverItem } from '#lib/api/client.js';
  import DiscoverCard from './DiscoverCard.svelte';
  import ScrollRow from '#lib/components/shared/ScrollRow.svelte';

  interface Props {
    title: string;
    items: DiscoverItem[];
  }

  let { title, items }: Props = $props();
</script>

{#if items.length > 0}
  <ScrollRow {title}>
    {#each items as item (item.provider_id)}
      <DiscoverCard {item} />
    {/each}
  </ScrollRow>
{/if}
