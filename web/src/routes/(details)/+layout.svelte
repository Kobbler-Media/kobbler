<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    children: Snippet;
  }

  let { children }: Props = $props();
</script>

<!--
  This layout wraps all detail pages (books, authors, series).
  It stays mounted when navigating between detail pages, preventing flash.
  Each page renders its own full-page view inside this stable container.
-->
<div class="details-layout">
  {@render children()}
</div>

<style>
  .details-layout {
    height: 100%;
    display: contents;
  }
</style>
