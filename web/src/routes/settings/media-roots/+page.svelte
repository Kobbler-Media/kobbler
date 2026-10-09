<script lang="ts">
  import MediaRootsPage from '$lib/components/pages/MediaRootsPage.svelte';
  import { getAuthState } from '$lib/stores/auth.svelte';

  const auth = getAuthState();
</script>

<svelte:head>
  <title>Media Roots — Kobbler</title>
</svelte:head>

{#if !auth.isAdmin}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <MediaRootsPage />
{/if}

<style>
  .permission-denied {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 4rem 2rem;
    color: var(--muted);
    text-align: center;
  }

  .permission-denied h2 {
    font-size: 1.25rem;
    margin-bottom: 0.5rem;
    color: var(--ink);
  }
</style>
