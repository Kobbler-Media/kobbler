<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';

  onMount(() => {
    goto('/settings/devices', { replaceState: true });
  });
</script>

<svelte:head>
  <title>Settings — Kobbler</title>
</svelte:head>

<div class="loading">Redirecting...</div>

<style>
  .loading {
    padding: 24px;
    text-align: center;
    color: var(--muted);
  }
</style>
