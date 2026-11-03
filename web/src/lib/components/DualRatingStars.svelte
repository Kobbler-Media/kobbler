<script lang="ts">
  import { BOOK_RATING_SCALE, formatRating, starFill } from '#lib/utils/rating.js';

  /**
   * DualRatingStars - Shows community rating (gold fill) + user rating (accent outline)
   * Click to change user's rating.
   */

  interface Props {
    communityRating: number | null | undefined;
    communityScale?: number;
    /** Community rating count */
    communityRatingCount: number | null;
    /** User's rating (1-5 or null) */
    userRating: number | null;
    /** Size: 'small' | 'medium' */
    size?: 'small' | 'medium';
    /** Whether rating can be edited */
    editable?: boolean;
    /** Callback when user rating changes */
    onRatingChange?: (rating: number | null) => void;
  }

  let {
    communityRating,
    communityScale = BOOK_RATING_SCALE,
    communityRatingCount,
    userRating,
    size = 'medium',
    editable = false,
    onRatingChange
  }: Props = $props();

  let hoverRating = $state<number | null>(null);

  const formatCount = (count: number | null): string => {
    if (!count) return '';
    if (count >= 1000000) return `${(count / 1000000).toFixed(1)}M`;
    if (count >= 1000) return `${(count / 1000).toFixed(0)}k`;
    return count.toString();
  };

  const handleClick = (star: number) => {
    if (!editable || !onRatingChange) return;
    // Toggle off if clicking the same rating
    if (userRating === star) {
      onRatingChange(null);
    } else {
      onRatingChange(star);
    }
  };

  const handleMouseEnter = (star: number) => {
    if (editable) hoverRating = star;
  };

  const handleMouseLeave = () => {
    hoverRating = null;
  };
</script>

<div class="dual-rating" class:small={size === 'small'} class:editable>
  <div class="stars" role="presentation" onmouseleave={handleMouseLeave}>
    {#each [1, 2, 3, 4, 5] as star (star)}
      {@const communityFill = starFill(communityRating, communityScale, star)}
      {@const showUserOutline =
        hoverRating != null ? star <= hoverRating : userRating != null && star <= userRating}
      <button
        type="button"
        class="star"
        class:has-user-rating={showUserOutline}
        class:hovering={hoverRating != null && star <= hoverRating}
        onclick={() => handleClick(star)}
        onmouseenter={() => handleMouseEnter(star)}
        disabled={!editable}
        aria-label={`Rate ${star} star${star > 1 ? 's' : ''}`}
      >
        <!-- Background star (empty) -->
        <svg class="star-bg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path
            d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z"
          ></path>
        </svg>
        <!-- Community fill (gold, partial) -->
        {#if communityFill > 0}
          <svg
            class="star-community"
            viewBox="0 0 24 24"
            style="clip-path: inset(0 {(1 - communityFill) * 100}% 0 0)"
          >
            <path
              d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z"
              fill="currentColor"
            ></path>
          </svg>
        {/if}
        <!-- User rating outline (accent) -->
        {#if showUserOutline}
          <svg
            class="star-user"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2.5"
          >
            <path
              d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z"
            ></path>
          </svg>
        {/if}
      </button>
    {/each}
  </div>
  {#if communityRating != null || communityRatingCount}
    <span class="rating-text">
      {#if communityRating != null}
        {formatRating(communityRating, communityScale)}
      {/if}
      {#if communityRatingCount}
        <span class="count">({formatCount(communityRatingCount)})</span>
      {/if}
    </span>
  {/if}
</div>

<style>
  .dual-rating {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .stars {
    display: flex;
    gap: 2px;
  }

  .star {
    position: relative;
    width: 18px;
    height: 18px;
    padding: 0;
    border: none;
    background: none;
    cursor: default;
  }

  .small .star {
    width: 14px;
    height: 14px;
  }

  .editable .star {
    cursor: pointer;
  }

  .star svg {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
  }

  .star-bg {
    color: var(--muted);
    opacity: 0.3;
  }

  .star-community {
    color: var(--gold, #f59e0b);
  }

  .star-user {
    color: var(--accent);
  }

  .editable .star:hover .star-bg {
    opacity: 0.5;
  }

  .star.hovering .star-user {
    opacity: 0.7;
  }

  .rating-text {
    font-size: 13px;
    color: var(--ink);
    white-space: nowrap;
  }

  .small .rating-text {
    font-size: 11px;
  }

  .count {
    color: var(--muted);
    font-size: 0.9em;
  }
</style>
