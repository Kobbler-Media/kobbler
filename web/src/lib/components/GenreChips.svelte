<script lang="ts">
  interface Props {
    genres: string[];
    clickable?: boolean;
    onGenreClick?: (genre: string) => void;
    maxDisplay?: number;
    size?: 'small' | 'medium';
  }

  let { genres, clickable = false, onGenreClick, maxDisplay, size = 'medium' }: Props = $props();

  const uniqueGenres = $derived([...new Set(genres)]);

  const displayedGenres = $derived(
    maxDisplay && uniqueGenres.length > maxDisplay
      ? uniqueGenres.slice(0, maxDisplay)
      : uniqueGenres
  );

  const hiddenCount = $derived(
    maxDisplay && uniqueGenres.length > maxDisplay ? uniqueGenres.length - maxDisplay : 0
  );

  const handleClick = (genre: string, event: MouseEvent) => {
    if (clickable && onGenreClick) {
      event.stopPropagation();
      onGenreClick(genre);
    }
  };
</script>

<div class="genre-chips" class:small={size === 'small'}>
  {#each displayedGenres as genre, i (`${i}-${genre}`)}
    {#if clickable}
      <button
        type="button"
        class="genre-chip clickable"
        onclick={(e) => handleClick(genre, e)}
        title={genre}
      >
        {genre}
      </button>
    {:else}
      <span class="genre-chip" title={genre}>
        {genre}
      </span>
    {/if}
  {/each}
  {#if hiddenCount > 0}
    <span class="genre-chip more" title={genres.slice(maxDisplay).join(', ')}>
      +{hiddenCount}
    </span>
  {/if}
</div>

<style>
  .genre-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
  }

  .genre-chip {
    display: inline-flex;
    align-items: center;
    padding: 3px 10px;
    border-radius: 12px;
    background: var(--bg-2, rgba(255, 255, 255, 0.05));
    border: 1px solid var(--ring);
    font-size: 11px;
    color: var(--ink);
    white-space: nowrap;
  }

  .genre-chip.clickable {
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .genre-chip.clickable:hover {
    background: var(--accent-2, #1d3a38);
    border-color: var(--accent-2, #1d3a38);
    color: #e7f4f2;
  }

  .genre-chip.more {
    background: transparent;
    color: var(--muted);
    border-style: dashed;
    cursor: default;
  }

  /* Small size variant */
  .genre-chips.small .genre-chip {
    padding: 2px 8px;
    font-size: 10px;
  }
</style>
