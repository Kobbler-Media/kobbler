<script lang="ts">
  import type { SearchSuggestion } from '#lib/types.js';
  import { suggestionLabel } from '#lib/composables/useSearchSuggestions.svelte.js';

  let {
    id,
    items,
    highlighted,
    onpick
  }: {
    id: string;
    items: SearchSuggestion[];
    highlighted: number;
    onpick: (title: string) => void;
  } = $props();
</script>

<ul class="suggestions" role="listbox" {id}>
  {#each items as item, i (`${item.media_type}:${item.title}`)}
    <li
      id="{id}-{i}"
      role="option"
      aria-selected={i === highlighted}
      class:highlighted={i === highlighted}
      onmousedown={(e) => {
        e.preventDefault();
        onpick(item.title);
      }}
    >
      <span class="title">{item.title}</span>
      <span class="meta">{suggestionLabel(item)}</span>
    </li>
  {/each}
</ul>

<style>
  .suggestions {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    right: 0;
    z-index: 20;
    margin: 0;
    padding: 4px;
    list-style: none;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 10px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
  }

  li {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 12px;
    border-radius: 6px;
    cursor: pointer;
    color: var(--ink);
  }

  li:hover,
  li.highlighted {
    background: var(--accent-dim);
  }

  .title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .meta {
    flex-shrink: 0;
    color: var(--muted);
    font-size: 12px;
  }
</style>
