<script lang="ts">
  import type { Snippet } from 'svelte';
  import { statusChip } from '#lib/utils/chip-mappings.js';
  import IconChip from '#lib/components/shared/IconChip.svelte';
  import { BOOK_RATING_SCALE, formatRating } from '#lib/utils/rating.js';

  interface Props {
    title: string;
    subtitle?: string | Snippet;
    description?: string | null;
    coverUrl?: string | null;
    backdropUrl?: string | null;
    logoUrl?: string | null;
    avatar?: boolean; // If true, render cover as a circle (for authors/cast)
    landscape?: boolean; // If true, render cover in 16:9 (for episode stills)
    statusBadge?: { label: string; class: string } | null;
    rating?: number | null;
    ratingScale?: number;
    ratingCount?: number | null;
    ratingContent?: Snippet; // When provided, replaces the static rating display
    meta?: Snippet;
    actions?: Snippet;
    secondaryActions?: Snippet;
    extraContent?: Snippet;
    embedded?: boolean; // If true, optimized for sidebar use
    hideRating?: boolean;
  }

  let {
    title,
    subtitle,
    description,
    coverUrl,
    backdropUrl,
    logoUrl,
    avatar = false,
    landscape = false,
    statusBadge,
    rating,
    ratingScale = BOOK_RATING_SCALE,
    ratingCount,
    ratingContent,
    meta,
    actions,
    secondaryActions,
    extraContent,
    embedded = false,
    hideRating = false
  }: Props = $props();

  let logoError = $state(false);

  let descriptionExpanded = $state(false);
  const isLongDescription = $derived(description && description.length > 240);
  const truncatedDescription = $derived(description ? description.slice(0, 240) + '...' : '');

  const finalBackdropUrl = $derived(backdropUrl || coverUrl);
</script>

<div class="entity-hero" class:embedded class:has-backdrop={!!finalBackdropUrl}>
  {#if embedded && finalBackdropUrl}
    <div class="hero-backdrop" style="background-image: url({finalBackdropUrl})"></div>
  {/if}

  <div class="hero-content">
    <div class="hero-main">
      <div class="hero-visual">
        <div
          class="hero-cover"
          class:avatar
          class:landscape
          style={coverUrl ? `background-image: url(${coverUrl})` : ''}
        >
          {#if !coverUrl}
            <span class="material-icons-outlined placeholder-icon">
              {avatar ? 'person' : 'image'}
            </span>
          {/if}
          {#if statusBadge}
            {@const chip = statusChip(statusBadge.class)}
            <span class="chip-pos">
              <IconChip icon={chip.icon} label={chip.label} color={chip.color} size="md" />
            </span>
          {/if}
        </div>
      </div>

      <div class="hero-details">
        <div class="hero-header-row">
          <div class="hero-titles">
            {#if logoUrl && !logoError}
              <img src={logoUrl} alt={title} class="hero-logo" onerror={() => (logoError = true)} />
              <h1 class="hero-title sr-only">{title}</h1>
            {:else}
              <h1 class="hero-title">{title}</h1>
            {/if}
            {#if subtitle}
              <div class="hero-subtitle">
                {#if typeof subtitle === 'string'}
                  {subtitle}
                {:else}
                  {@render subtitle()}
                {/if}
              </div>
            {/if}
          </div>
          {#if secondaryActions}
            <div class="hero-secondary-actions">
              {@render secondaryActions()}
            </div>
          {/if}
        </div>

        {#if ratingContent || meta || (rating && !hideRating)}
          <div class="hero-meta-row">
            {#if ratingContent}
              {@render ratingContent()}
            {:else if rating && !hideRating}
              <div class="hero-rating">
                <span class="material-icons-outlined star-icon">star</span>
                <span class="rating-value">{formatRating(rating, ratingScale)}</span>
                {#if ratingCount}
                  <span class="rating-count">({ratingCount})</span>
                {/if}
              </div>
            {/if}
            {#if meta}
              <div class="hero-meta-snippet">
                {@render meta()}
              </div>
            {/if}
          </div>
        {/if}

        {#if actions}
          <div class="hero-actions">
            {@render actions()}
          </div>
        {/if}

        {#if description && !embedded}
          <div class="hero-description">
            <p>
              {descriptionExpanded ? description : truncatedDescription}
              {#if isLongDescription}
                <button
                  type="button"
                  class="expand-btn"
                  onclick={() => (descriptionExpanded = !descriptionExpanded)}
                >
                  {descriptionExpanded ? 'Show less' : 'Read more'}
                </button>
              {/if}
            </p>
          </div>
        {/if}

        {#if extraContent && !embedded}
          <div class="hero-extra">{@render extraContent()}</div>
        {/if}
      </div>
    </div>
  </div>
</div>

<style>
  .entity-hero {
    position: relative;
    border-radius: 0;
    margin-bottom: 0;
    color: var(--ink);
  }

  .entity-hero.embedded {
    margin: -24px -24px 24px;
    border-radius: 0;
    border: none;
    border-bottom: 1px solid var(--ring);
    background: var(--card);
    overflow: hidden;
  }

  .hero-backdrop {
    position: absolute;
    inset: 0;
    background-size: cover;
    background-position: center 20%;
    filter: blur(60px) saturate(1.8);
    opacity: 0.35;
    z-index: 0;
    -webkit-mask-image: linear-gradient(to bottom, black 0%, transparent 100%);
    mask-image: linear-gradient(to bottom, black 0%, transparent 100%);
  }

  :global([data-theme='light']) .hero-backdrop {
    opacity: 0.15;
    filter: blur(80px) saturate(1.2);
  }

  .hero-content {
    position: relative;
    z-index: 1;
    padding: 24px clamp(24px, 5vw, 80px);
    max-width: 1600px;
    margin: 0 auto;
    width: 100%;
  }

  /* Clears the floating glass buttons ContentPageView pins to the top-right corner. */
  .entity-hero:not(.embedded) .hero-content {
    padding-top: 68px;
  }

  .entity-hero.embedded .hero-content {
    padding: 24px 20px 20px;
    max-width: none;
    margin: 0;
  }

  .hero-main {
    display: flex;
    gap: 32px;
    align-items: flex-end;
  }

  .entity-hero.embedded .hero-main {
    gap: 20px;
  }

  /* Visual (Cover/Avatar) */
  .hero-visual {
    flex-shrink: 0;
  }

  .hero-cover {
    width: 180px;
    aspect-ratio: 2 / 3;
    background-size: cover;
    background-position: center;
    background-color: var(--bg-2);
    border-radius: 12px;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.4);
    border: 1px solid rgba(255, 255, 255, 0.1);
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .entity-hero.embedded .hero-cover {
    width: 110px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
  }

  .hero-cover.avatar {
    aspect-ratio: 1 / 1;
    border-radius: 50%;
  }

  .hero-cover.landscape {
    width: 280px;
    aspect-ratio: 16 / 9;
  }

  .entity-hero.embedded .hero-cover.landscape {
    width: 200px;
  }

  .placeholder-icon {
    font-size: 48px;
    color: var(--muted);
    opacity: 0.5;
  }

  .chip-pos {
    position: absolute;
    top: 10px;
    right: 10px;
  }

  /* Detail Panel */
  .hero-details {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .entity-hero.embedded .hero-details {
    gap: 8px;
  }

  .hero-header-row {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 16px;
  }

  .hero-logo {
    max-height: 80px;
    width: auto;
    object-fit: contain;
    display: block;
    filter: drop-shadow(0 2px 8px rgba(0, 0, 0, 0.5));
  }

  .entity-hero.embedded .hero-logo {
    max-height: 48px;
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border-width: 0;
  }

  .hero-title {
    margin: 0;
    font-size: 32px;
    font-weight: 900;
    line-height: 1.1;
    letter-spacing: -0.02em;
  }

  .entity-hero.embedded .hero-title {
    font-size: 20px;
  }

  .hero-subtitle {
    font-size: 16px;
    color: var(--muted);
    font-weight: 500;
    margin-top: 4px;
  }

  .entity-hero.embedded .hero-subtitle {
    font-size: 14px;
  }

  /* Meta & Rating */
  .hero-meta-row {
    display: flex;
    align-items: center;
    gap: 16px;
    flex-wrap: wrap;
  }

  .hero-rating {
    display: flex;
    align-items: center;
    gap: 4px;
    font-weight: 700;
  }

  .star-icon {
    font-size: 18px;
    color: #f5c518;
  }

  .rating-value {
    font-size: 15px;
  }

  .rating-count {
    font-size: 13px;
    color: var(--muted);
    font-weight: 400;
  }

  .hero-meta-snippet {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 8px;
    color: var(--muted);
    font-size: 14px;
    font-weight: 500;
  }

  /* Actions */
  .hero-actions {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-top: 8px;
    flex-wrap: wrap;
  }

  .entity-hero.embedded .hero-actions {
    margin-top: 4px;
    gap: 8px;
  }

  /* Description */
  .hero-description {
    max-width: 800px;
    font-size: 15px;
    line-height: 1.6;
    color: var(--ink);
    opacity: 0.9;
  }

  .hero-description p {
    margin: 0;
  }

  .expand-btn {
    background: none;
    border: none;
    color: var(--accent);
    font-weight: 600;
    cursor: pointer;
    padding: 0;
    margin-left: 4px;
  }

  .expand-btn:hover {
    text-decoration: underline;
  }

  .hero-extra {
    margin-top: 8px;
  }

  @media (max-width: 768px) {
    .hero-main {
      flex-direction: column;
      align-items: center;
      text-align: center;
      --meta-line-justify: center;
    }

    .hero-details {
      align-self: stretch;
    }

    .hero-header-row {
      flex-direction: column;
      align-items: center;
    }

    .hero-meta-row,
    .hero-meta-snippet,
    .hero-actions {
      justify-content: center;
    }

    .hero-cover {
      width: 140px;
    }

    .hero-cover.landscape {
      width: 220px;
    }

    .entity-hero.embedded .hero-cover {
      width: 100px;
    }

    .hero-title {
      font-size: 24px;
    }
  }
</style>
