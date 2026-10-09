<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    /** Image URL for the poster. */
    posterUrl?: string | null;
    /** Material icon name for the placeholder (e.g. "movie", "tv", "auto_stories"). */
    placeholderIcon?: string;
    /** CSS aspect-ratio for the poster area (default "2 / 3"). */
    aspectRatio?: string;
    /** CSS object-fit for the poster image (default "cover"). Use "contain" for logos. */
    posterFit?: string;
    /** CSS padding around the poster image (e.g. "12px"). Useful for logos with contain fit. */
    posterPadding?: string;
    /** Navigation href — if set, renders as <a> instead of <button>. */
    href?: string | null;
    /** Click handler — used when rendering as <button>. */
    onclick?: (event: MouseEvent) => void;
    /** Visual selected state (accent border). */
    selected?: boolean;
    /** Visual wanted state (dimmed + desaturated). */
    wanted?: boolean;
    /** data-item-id for selection/scroll logic in parent grids. */
    itemId?: string | null;
    /** Hide the placeholder background and icon (for transparent logos). */
    hidePlaceholder?: boolean;
    /** Svelte transition (e.g. fade). Applied to the root element. */
    transition?: any;
    /** Content rendered inside the poster area (badges, progress bars, etc). */
    posterOverlay?: Snippet;
    /** Content rendered below the poster (title, subtitle, accent text). */
    children: Snippet;
  }

  let {
    posterUrl = null,
    placeholderIcon = 'movie',
    aspectRatio = '2 / 3',
    posterFit = 'cover',
    posterPadding,
    href = null,
    onclick,
    selected = false,
    wanted = false,
    hidePlaceholder = false,
    itemId = null,
    posterOverlay,
    children
  }: Props = $props();
</script>

{#snippet posterContent()}
  <div
    class="poster"
    class:wanted-poster={wanted}
    class:no-placeholder={hidePlaceholder}
    style:aspect-ratio={aspectRatio}
  >
    {#if !hidePlaceholder}
      <span class="material-icons-outlined placeholder-icon">{placeholderIcon}</span>
    {/if}
    {#if posterUrl}
      <img
        src={posterUrl}
        alt=""
        class="poster-img"
        style:object-fit={posterFit}
        style:padding={posterPadding}
        loading="lazy"
        onerror={(e) => {
          (e.currentTarget as HTMLImageElement).style.display = 'none';
        }}
      />
    {/if}
    {#if posterOverlay}
      {@render posterOverlay()}
    {/if}
  </div>
  <div class="meta">
    {@render children()}
  </div>
{/snippet}

{#if href}
  <a class="media-card" class:wanted {href} data-item-id={itemId} {onclick}>
    {@render posterContent()}
  </a>
{:else}
  <button
    class="media-card"
    class:selected
    class:wanted
    type="button"
    data-item-id={itemId}
    {onclick}
  >
    {@render posterContent()}
  </button>
{/if}

<style>
  .media-card {
    appearance: none;
    text-align: left;
    color: inherit;
    font: inherit;
    text-decoration: none;
    cursor: pointer;
    padding: 0;
    margin: 0;
    border-radius: 10px;
    background: var(--card);
    border: 1px solid var(--ring);
    display: flex;
    flex-direction: column;
    position: relative;
    overflow: hidden;
    transition:
      transform 0.15s ease,
      box-shadow 0.15s ease;
  }

  .media-card:hover {
    transform: translateY(-3px);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
  }

  :global([data-theme='light']) .media-card:hover {
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.12);
  }

  .media-card.selected {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-dim);
  }

  .media-card.wanted {
    opacity: 0.85;
  }

  .poster {
    /* aspect-ratio set via inline style */
    width: 100%;
    background: linear-gradient(140deg, #2e323a, #1a1d23);
    position: relative;
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  :global([data-theme='light']) .poster {
    background: linear-gradient(140deg, #e5e0d8, #d5cfc5);
  }

  .poster.no-placeholder {
    background: transparent;
  }

  .poster.wanted-poster {
    filter: saturate(0.7);
  }

  .poster-img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    /* object-fit set via inline style */
    z-index: 1;
  }

  .placeholder-icon {
    font-size: 48px;
    color: var(--muted);
    opacity: 0.3;
    z-index: 0;
  }

  .meta {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    padding: 8px 10px 10px;
  }

  /* Shared child styles — callers use these classes */
  .meta :global(.card-title) {
    font-size: 14px;
    font-weight: 700;
    line-height: 1.3;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .meta :global(.card-sub) {
    color: var(--muted);
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .meta :global(.card-accent) {
    color: var(--accent);
    font-size: 11px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    margin-top: 2px;
  }

  /* Shared poster overlay positions */
  .poster :global(.chip-pos) {
    position: absolute;
    top: 8px;
    right: 8px;
    z-index: 2;
  }

  .poster :global(.chip-pos-left) {
    position: absolute;
    top: 8px;
    left: 8px;
    z-index: 2;
  }

  .poster :global(.format-icons) {
    position: absolute;
    bottom: 8px;
    left: 8px;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 2px;
    background: rgba(0, 0, 0, 0.7);
    border-radius: 6px;
    padding: 5px;
    color: #fff;
    box-shadow: 0 2px 4px rgba(0, 0, 0, 0.3);
  }

  .poster :global(.format-icons .material-icons-outlined) {
    font-size: 14px;
  }

  .poster :global(.progress-track) {
    position: absolute;
    bottom: 0;
    left: 0;
    right: 0;
    height: 3px;
    background: rgba(255, 255, 255, 0.15);
    z-index: 2;
  }

  :global([data-theme='light']) .poster :global(.progress-track) {
    background: rgba(0, 0, 0, 0.1);
  }

  .poster :global(.progress-fill) {
    height: 100%;
    background: var(--accent);
    transition: width 0.3s ease;
  }

  .poster :global(.progress-fill.complete) {
    background: var(--success, #22c55e);
  }
</style>
