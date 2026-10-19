<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    items?: (string | number | null | undefined | false)[];
    children?: Snippet;
  }

  let { items = [], children }: Props = $props();

  const shown = $derived(items.filter((i) => !!i));

  // A "·" belongs between two items on the same line; when the next item wraps, the separator
  // would dangle at the end of this line, so it is hidden (not removed, which would reflow).
  function markLineEnds(node: HTMLElement) {
    const mark = () => {
      const kids = [...node.children] as HTMLElement[];
      kids.forEach((kid, i) => {
        const next = kids[i + 1];
        const wraps =
          !!next && next.getBoundingClientRect().top >= kid.getBoundingClientRect().bottom - 1;
        kid.toggleAttribute('data-line-end', wraps);
      });
    };
    const resize = new ResizeObserver(mark);
    resize.observe(node);
    const mutate = new MutationObserver(mark);
    mutate.observe(node, { childList: true, subtree: true, characterData: true });
    mark();
    return {
      destroy() {
        resize.disconnect();
        mutate.disconnect();
      }
    };
  }
</script>

<span class="meta-line" use:markLineEnds>
  {#each shown as item, i (i)}<span>{item}</span>{/each}{@render children?.()}
</span>

<style>
  .meta-line {
    --meta-sep-width: 16px;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: var(--meta-line-justify, flex-start);
    row-gap: 4px;
    min-width: 0;
  }

  .meta-line > :global(*) {
    position: relative;
  }

  .meta-line > :global(:not(:last-child)) {
    margin-right: var(--meta-sep-width);
  }

  .meta-line > :global(:not(:last-child))::after {
    content: '·';
    position: absolute;
    top: 0;
    bottom: 0;
    right: calc(-1 * var(--meta-sep-width));
    width: var(--meta-sep-width);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--muted);
    font-size: 14px;
    font-weight: 400;
    text-transform: none;
  }

  .meta-line > :global([data-line-end])::after {
    visibility: hidden;
  }
</style>
