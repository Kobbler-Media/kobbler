<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    children: Snippet;
  }

  let { children }: Props = $props();
</script>

<!--
  Reader layout: full-screen overlay that covers the app chrome (navbar, sidebar).
  Uses fixed positioning to sit on top of everything.

  Safe-area padding keeps interactive elements clear of the device system
  bars (Android gesture bar, iOS home indicator/notch). VideoPlayer lives
  outside this layout so it can render full-bleed for aspect-ratio scaling.
-->
<div class="reader-layout">
  {@render children()}
</div>

<style>
  .reader-layout {
    position: fixed;
    inset: 0;
    z-index: 1000;
    background: var(--bg, #ffffff);
    display: flex;
    flex-direction: column;
    padding-top: env(safe-area-inset-top);
    padding-right: env(safe-area-inset-right);
    padding-bottom: env(safe-area-inset-bottom);
    padding-left: env(safe-area-inset-left);
    box-sizing: border-box;
  }
</style>
