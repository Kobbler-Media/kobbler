<script lang="ts">
  import type { BookCastLineContext } from '#lib/types.js';

  interface Props {
    context: BookCastLineContext | null;
    fallback: string | null;
    expanded?: boolean;
  }

  let { context, fallback, expanded = $bindable(false) }: Props = $props();

  let hasMore = $derived(!!context && (context.before !== null || context.after !== null));
</script>

<div class="reading">
  {#if context}
    {#if expanded && context.before}
      <p class="around">{context.before}</p>
    {/if}
    <p class="para">{context.pre}<mark>{context.quote}</mark>{context.post}</p>
    {#if expanded && context.after}
      <p class="around">{context.after}</p>
    {/if}
    {#if hasMore}
      <button type="button" class="more" onclick={() => (expanded = !expanded)}>
        <span class="material-icons-outlined">{expanded ? 'unfold_less' : 'unfold_more'}</span>
        {expanded ? 'Less context' : 'More context'}
      </button>
    {/if}
  {:else}
    <p class="para"><mark>“{fallback ?? ''}”</mark></p>
  {/if}
</div>

<style>
  .reading {
    display: grid;
    gap: 8px;
    font-family: Georgia, 'Iowan Old Style', 'Times New Roman', serif;
    font-size: 15px;
    line-height: 1.6;
  }

  p {
    margin: 0;
  }

  .around {
    color: var(--muted);
    font-size: 14px;
  }

  mark {
    background: color-mix(in srgb, var(--accent) 22%, transparent);
    color: var(--ink);
    border-radius: 4px;
    padding: 1px 2px;
    box-decoration-break: clone;
    -webkit-box-decoration-break: clone;
  }

  .more {
    justify-self: start;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font: inherit;
    font-family: system-ui, sans-serif;
    font-size: 12px;
    color: var(--muted);
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
  }

  .more .material-icons-outlined {
    font-size: 16px;
  }
</style>
