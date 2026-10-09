<script lang="ts">
  import { onMount, type Snippet } from 'svelte';
  import {
    captureScrollAnchor,
    getScrollContainer,
    restoreScrollAnchor,
    type ScrollAnchor
  } from '$lib/utils/scroll-anchor';

  interface Props {
    hasMore: boolean;
    hasPrevious: boolean;
    loading: boolean;
    onLoadMore: () => void;
    onLoadPrevious: () => void;
    threshold?: number;
    children?: Snippet;
  }

  let {
    hasMore,
    hasPrevious,
    loading,
    onLoadMore,
    onLoadPrevious,
    threshold = 200,
    children
  }: Props = $props();

  let topSentinel: HTMLDivElement | undefined = $state();
  let bottomSentinel: HTMLDivElement | undefined = $state();
  let topObserver: IntersectionObserver | undefined;
  let bottomObserver: IntersectionObserver | undefined;

  // Track state for scroll position restoration after prepending
  // This is a "busy" flag that blocks new loads until the entire cycle completes
  let isBusy = $state(false);
  let prevLoadingState = false;
  let scrollAnchor: ScrollAnchor | null = null;

  // Use derived to access current prop values in callbacks
  let currentState = $derived({ hasMore, hasPrevious, loading });

  // Restore scroll position after prepending items
  async function restoreScrollPosition() {
    const anchor = scrollAnchor;
    scrollAnchor = null;
    await restoreScrollAnchor(anchor);

    // Wait a moment for scroll to settle before allowing new loads
    // This prevents the observer from immediately firing again
    await new Promise<void>((resolve) => {
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          resolve();
        });
      });
    });

    isBusy = false;
  }

  // Detect when loading previous completes (loading transitions from true to false while busy)
  $effect(() => {
    if (isBusy && prevLoadingState && !loading) {
      // Loading finished, now restore scroll position
      restoreScrollPosition();
    }
    prevLoadingState = loading;
  });

  function handleLoadPrevious() {
    // Guard against double-triggering - use synchronous check of isBusy
    if (isBusy) return;

    // Immediately set busy to prevent any further triggers
    isBusy = true;
    scrollAnchor = captureScrollAnchor();
    onLoadPrevious();
  }

  onMount(() => {
    // Find the scroll container to use as the IntersectionObserver root
    const scrollContainer = getScrollContainer();
    const observerRoot = scrollContainer === document.documentElement ? null : scrollContainer;

    // Create observers with the correct root
    topObserver = new IntersectionObserver(
      (entries) => {
        const entry = entries[0];
        if (entry.isIntersecting && currentState.hasPrevious && !currentState.loading && !isBusy) {
          handleLoadPrevious();
        }
      },
      { root: observerRoot, rootMargin: `${threshold}px` }
    );

    bottomObserver = new IntersectionObserver(
      (entries) => {
        const entry = entries[0];
        if (entry.isIntersecting && currentState.hasMore && !currentState.loading) {
          onLoadMore();
        }
      },
      { root: observerRoot, rootMargin: `${threshold}px` }
    );

    return () => {
      topObserver?.disconnect();
      bottomObserver?.disconnect();
    };
  });

  // Only observe top sentinel when hasPrevious is true
  // This prevents false triggers on initial page load
  $effect(() => {
    if (topObserver && topSentinel && hasPrevious) {
      topObserver.observe(topSentinel);
      return () => {
        topObserver?.unobserve(topSentinel!);
      };
    }
  });

  // Observe bottom sentinel when hasMore is true.
  // Track only `hasMore` (not `currentState.hasMore`): currentState is a
  // $derived that changes on every loading flip, which would unobserve+
  // reobserve and re-fire the IntersectionObserver callback after each
  // load — producing an endless paginate loop on pages where the sentinel
  // stays within the threshold (e.g. fewer items than viewport+threshold).
  $effect(() => {
    if (bottomObserver && bottomSentinel && hasMore) {
      bottomObserver.observe(bottomSentinel);
      return () => {
        bottomObserver?.unobserve(bottomSentinel!);
      };
    }
  });
</script>

<!-- Top sentinel for loading previous - always render but hide when not needed -->
<div
  bind:this={topSentinel}
  class="scroll-sentinel top-sentinel"
  class:hidden={!hasPrevious}
  aria-hidden="true"
>
  {#if loading && hasPrevious}
    <div class="loading-indicator">
      <span class="spinner"></span>
      Loading previous...
    </div>
  {/if}
</div>

{#if children}
  {@render children()}
{/if}

<!-- Bottom sentinel for loading more - always render but hide when not needed -->
<div
  bind:this={bottomSentinel}
  class="scroll-sentinel bottom-sentinel"
  class:hidden={!hasMore}
  aria-hidden="true"
>
  {#if loading && hasMore}
    <div class="loading-indicator">
      <span class="spinner"></span>
      Loading more...
    </div>
  {/if}
</div>

<style>
  .scroll-sentinel {
    min-height: 1px;
    padding: 0.5rem 0;
    text-align: center;
    color: var(--muted, #888);
    font-size: 0.875rem;
    /* Disable browser scroll anchoring - we handle it ourselves */
    overflow-anchor: none;
  }

  .scroll-sentinel.hidden {
    /* Use visibility instead of display:none so IntersectionObserver still works */
    visibility: hidden;
    height: 0;
    padding: 0;
    margin: 0;
    overflow: hidden;
  }

  .top-sentinel {
    margin-bottom: 0.5rem;
  }

  .bottom-sentinel {
    margin-top: 0.5rem;
  }

  .loading-indicator {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    padding: 0.5rem;
  }

  .spinner {
    display: inline-block;
    width: 1rem;
    height: 1rem;
    border: 2px solid currentColor;
    border-right-color: transparent;
    border-radius: 50%;
    animation: spin 0.75s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
