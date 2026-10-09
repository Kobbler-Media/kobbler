<script lang="ts">
  interface Props {
    filled: number;
    total?: number;
  }

  let { filled, total = 4 }: Props = $props();
</script>

<span class="dots" aria-label="{filled} of {total} confidence">
  {#each Array.from({ length: total }, (_, i) => i) as i (i)}
    <span class="dot" class:on={i < filled}></span>
  {/each}
</span>

<style>
  .dots {
    display: flex;
    gap: 2px;
    flex-shrink: 0;
  }

  .dot {
    width: 4px;
    height: 4px;
    border-radius: 50%;
    background: var(--ring);
  }

  .dot.on {
    background: var(--accent);
  }
</style>
