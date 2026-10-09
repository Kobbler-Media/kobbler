<script lang="ts">
  import { onMount } from 'svelte';
  import { initSettingsHighlight } from '$lib/utils/settings-highlight';

  let { children } = $props();

  onMount(() => {
    return initSettingsHighlight();
  });
</script>

<div class="settings-layout page-container">
  {@render children()}
</div>

<style>
  :global(.field-highlight) {
    animation: setting-pulse 2.5s ease-out;
    border-radius: 12px;
  }

  @keyframes setting-pulse {
    0%,
    30% {
      background: color-mix(in srgb, var(--accent) 15%, transparent);
    }
    100% {
      background: transparent;
    }
  }
</style>
