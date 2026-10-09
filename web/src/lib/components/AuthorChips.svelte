<script lang="ts">
  import type { AuthorRef } from '$lib/types';

  interface Props {
    authors: AuthorRef[];
    clickable?: boolean;
    onAuthorClick?: (authorName: string, authorId: string) => void;
    maxDisplay?: number;
    size?: 'small' | 'medium';
  }

  let { authors, clickable = false, onAuthorClick, maxDisplay, size = 'medium' }: Props = $props();

  const displayedAuthors = $derived(
    maxDisplay && authors.length > maxDisplay ? authors.slice(0, maxDisplay) : authors
  );

  const hiddenCount = $derived(
    maxDisplay && authors.length > maxDisplay ? authors.length - maxDisplay : 0
  );

  const handleClick = (author: AuthorRef, event: MouseEvent) => {
    if (clickable && onAuthorClick && author.id) {
      event.stopPropagation();
      onAuthorClick(author.name, author.id);
    }
  };
</script>

<div class="author-chips" class:small={size === 'small'}>
  {#each displayedAuthors as author, i (author.id || author.name + ':' + i)}
    {#if clickable && author.id}
      <button
        type="button"
        class="author-chip clickable"
        onclick={(e) => handleClick(author, e)}
        title={author.name}
      >
        {author.name}
      </button>
    {:else}
      <span class="author-chip" title={author.name}>
        {author.name}
      </span>
    {/if}
  {/each}
  {#if hiddenCount > 0}
    <span
      class="author-chip more"
      title={authors
        .slice(maxDisplay)
        .map((a) => a.name)
        .join(', ')}
    >
      +{hiddenCount} more
    </span>
  {/if}
</div>

<style>
  .author-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
  }

  .author-chip {
    display: inline-flex;
    align-items: center;
    padding: 4px 10px;
    border-radius: 999px;
    background: var(--bg-2, rgba(255, 255, 255, 0.05));
    border: 1px solid var(--ring);
    font-size: 12px;
    color: var(--ink);
    white-space: nowrap;
    max-width: 180px;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .author-chip.clickable {
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .author-chip.clickable:hover {
    background: var(--accent);
    color: #1d130c;
    border-color: var(--accent);
  }

  .author-chip.more {
    background: transparent;
    color: var(--muted);
    border-style: dashed;
    cursor: default;
  }

  /* Small size variant */
  .author-chips.small .author-chip {
    padding: 2px 8px;
    font-size: 11px;
    max-width: 140px;
  }
</style>
