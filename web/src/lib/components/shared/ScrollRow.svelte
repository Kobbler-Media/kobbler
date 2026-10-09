<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    title: string;
    viewAllHref?: string;
    children: Snippet;
  }

  let { title, viewAllHref, children }: Props = $props();

  let scrollContainer: HTMLDivElement | undefined = $state();
  let canScrollLeft = $state(false);
  let canScrollRight = $state(false);

  function updateScrollState() {
    if (!scrollContainer) return;
    const { scrollLeft, scrollWidth, clientWidth } = scrollContainer;
    canScrollLeft = scrollLeft > 4;
    canScrollRight = scrollLeft + clientWidth < scrollWidth - 4;
  }

  $effect(() => {
    if (scrollContainer) {
      updateScrollState();
    }
  });

  function scroll(direction: 'left' | 'right') {
    if (!scrollContainer) return;
    const amount = scrollContainer.clientWidth * 0.75;
    scrollContainer.scrollBy({
      left: direction === 'left' ? -amount : amount,
      behavior: 'smooth'
    });
  }
</script>

<section class="row">
  <div class="row-header">
    <h2 class="row-title">{title}</h2>
    {#if viewAllHref}
      <a href={viewAllHref} class="view-all">View all</a>
    {/if}
  </div>

  <div class="scroll-wrapper">
    {#if canScrollLeft}
      <button
        class="scroll-btn scroll-btn-left"
        type="button"
        onclick={() => scroll('left')}
        aria-label="Scroll left"
      >
        <svg viewBox="0 0 24 24" width="20" height="20" fill="currentColor">
          <path d="M15.41 7.41L14 6l-6 6 6 6 1.41-1.41L10.83 12z"></path>
        </svg>
      </button>
    {/if}

    <div class="scroll-track" bind:this={scrollContainer} onscroll={updateScrollState}>
      {@render children()}
    </div>

    {#if canScrollRight}
      <button
        class="scroll-btn scroll-btn-right"
        type="button"
        onclick={() => scroll('right')}
        aria-label="Scroll right"
      >
        <svg viewBox="0 0 24 24" width="20" height="20" fill="currentColor">
          <path d="M10 6L8.59 7.41 13.17 12l-4.58 4.59L10 18l6-6z"></path>
        </svg>
      </button>
    {/if}
  </div>
</section>

<style>
  .row {
    margin-bottom: 28px;
  }

  .row-header {
    display: flex;
    align-items: baseline;
    gap: 12px;
    margin-bottom: 12px;
    padding: 0 24px;
  }

  .row-title {
    font-size: 17px;
    font-weight: 700;
    color: var(--ink);
    margin: 0;
    letter-spacing: -0.2px;
  }

  .view-all {
    font-size: 13px;
    color: var(--accent);
    text-decoration: none;
    margin-left: auto;
  }

  .view-all:hover {
    text-decoration: underline;
  }

  .scroll-wrapper {
    position: relative;
  }

  .scroll-track {
    display: flex;
    gap: 12px;
    overflow-x: auto;
    overflow-y: hidden;
    scroll-behavior: smooth;
    padding: 4px 24px 8px;
    -webkit-overflow-scrolling: touch;
    scrollbar-width: none;
  }

  .scroll-track :global(.media-card) {
    width: 160px;
    flex-shrink: 0;
  }

  .scroll-track::-webkit-scrollbar {
    display: none;
  }

  .scroll-btn {
    position: absolute;
    top: 50%;
    transform: translateY(-50%);
    z-index: 10;
    width: 36px;
    height: 36px;
    border-radius: 50%;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
    opacity: 0;
    transition:
      opacity 0.2s,
      background 0.15s;
  }

  .scroll-wrapper:hover .scroll-btn {
    opacity: 1;
  }

  .scroll-btn:hover {
    background: var(--bg-2);
  }

  .scroll-btn-left {
    left: 8px;
  }

  .scroll-btn-right {
    right: 8px;
  }

  @media (max-width: 640px) {
    .row-header {
      padding: 0 16px;
    }

    .scroll-track {
      padding: 4px 16px 8px;
    }

    .scroll-track :global(.media-card) {
      width: 130px;
    }

    .scroll-btn {
      opacity: 1;
      width: 32px;
      height: 32px;
    }
  }
</style>
