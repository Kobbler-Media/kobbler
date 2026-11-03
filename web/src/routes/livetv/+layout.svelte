<script lang="ts">
  import type { Snippet } from 'svelte';
  import LiveTvPlayer from '#lib/components/livetv/player/LiveTvPlayer.svelte';
  import { getLivetvPlayerState } from '#lib/stores/livetvPlayer.svelte.js';

  interface Props {
    children: Snippet;
  }

  let { children }: Props = $props();

  const player = getLivetvPlayerState();
</script>

{@render children()}

{#if player.channel}
  <LiveTvPlayer channel={player.channel} />
{/if}
