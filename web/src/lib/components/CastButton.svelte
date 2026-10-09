<script lang="ts">
  import { getCastState } from '$lib/stores/cast.svelte';

  interface Props {
    class?: string;
  }

  let { class: className = '' }: Props = $props();
  const cast = getCastState();
</script>

{#if cast.available}
  <google-cast-launcher
    class="cast-btn {className}"
    title={cast.connected ? `Casting to ${cast.deviceName}` : 'Cast'}
  ></google-cast-launcher>
{/if}

<style>
  .cast-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    cursor: pointer;
    border-radius: 4px;
    --connected-color: #7c5cfc;
    --disconnected-color: rgba(255, 255, 255, 0.85);
  }

  .cast-btn:hover {
    background: rgba(255, 255, 255, 0.1);
  }
</style>
