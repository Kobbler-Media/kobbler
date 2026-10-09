<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';

  onMount(() => {
    goto('/settings/devices', { replaceState: true });
  });
</script>

<svelte:head>
  <title>Devices — Kobbler</title>
</svelte:head>

<div class="redirect">Redirecting to settings...</div>

<style>
  .redirect {
    padding: 24px;
    text-align: center;
    color: var(--muted);
  }
</style>
