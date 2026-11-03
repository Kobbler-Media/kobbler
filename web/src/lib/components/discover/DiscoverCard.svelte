<script lang="ts">
  import type { DiscoverItem } from '#lib/api/client.js';
  import MediaCard from '#lib/components/shared/MediaCard.svelte';
  import { setDiscoverPreview } from '#lib/stores/discover-preview.svelte.js';
  import { discoverLibraryRoute } from '#lib/utils/entity-routes.js';

  interface Props {
    item: DiscoverItem;
    subtitle?: string | null;
    progressPercent?: number | null;
    season?: number | null;
    ondismiss?: () => void;
  }

  let { item, subtitle = null, progressPercent = null, season = null, ondismiss }: Props = $props();

  function dismiss(e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    ondismiss?.();
  }

  const libraryHref = $derived(discoverLibraryRoute(item.media_type, item.library_id));
  const href = $derived(
    libraryHref ??
      `/discover/${item.media_type}/${item.provider}/${item.provider_id}${season ? `?season=${season}` : ''}`
  );

  function handleClick() {
    if (libraryHref) return;
    setDiscoverPreview({
      media_type: item.media_type,
      provider: item.provider,
      provider_id: item.provider_id,
      title: item.title,
      year: item.year,
      description: item.description,
      poster_url: item.poster_url,
      backdrop_url: item.backdrop_url,
      genres: item.genres,
      rating: item.rating,
      rating_count: item.rating_count,
      identifiers: item.identifiers,
      library_id: item.library_id,
      want_status: item.want_status,
      is_in_library: item.is_in_library
    });
  }
  const isWanted = $derived(item.want_status != null && item.want_status !== 'none');

  const placeholderIcon = $derived(
    item.media_type === 'movie' ? 'movie' : item.media_type === 'tv' ? 'tv' : 'auto_stories'
  );

  const secondaryText = $derived.by(() => {
    if (subtitle) return subtitle;
    if (item.media_type === 'book') {
      const authors = item.authors ?? [];
      return authors.length > 0 ? authors.slice(0, 2).join(', ') : null;
    }
    return item.year ? String(item.year) : null;
  });

  const accentText = $derived.by(() => {
    const genres = item.genres ?? [];
    if (item.media_type === 'book') return null;
    return genres.length > 0 ? genres.slice(0, 2).join(', ') : null;
  });
</script>

<MediaCard
  posterUrl={item.poster_url}
  {placeholderIcon}
  {href}
  wanted={isWanted}
  onclick={handleClick}
>
  {#snippet posterOverlay()}
    {#if ondismiss}
      <button
        class="dismiss chip-pos-left"
        type="button"
        title="Not interested"
        aria-label="Not interested"
        onclick={dismiss}
      >
        <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor">
          <path
            d="M19 6.41 17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z"
          ></path>
        </svg>
      </button>
    {/if}
    {#if item.is_in_library}
      <span class="chip-pos">
        <span class="status-chip status-library">
          <svg viewBox="0 0 24 24" width="10" height="10" fill="currentColor">
            <path d="M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41L9 16.17z"></path>
          </svg>
          Owned
        </span>
      </span>
    {:else if isWanted}
      <span class="chip-pos">
        <span class="status-chip status-wanted">Wanted</span>
      </span>
    {/if}
    {#if progressPercent != null}
      <div class="progress-track">
        <div class="progress-fill" style="width: {progressPercent}%"></div>
      </div>
    {/if}
  {/snippet}
  <strong class="card-title">{item.title}</strong>
  {#if secondaryText}
    <div class="card-sub">{secondaryText}</div>
  {/if}
  {#if accentText}
    <div class="card-accent">{accentText}</div>
  {/if}
</MediaCard>

<style>
  .status-chip {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 2px 7px;
    border-radius: 6px;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.3px;
    text-transform: uppercase;
    backdrop-filter: blur(8px);
    -webkit-backdrop-filter: blur(8px);
    color: #fff;
    line-height: 1.4;
  }

  .status-library {
    background: rgba(34, 197, 94, 0.85);
  }

  .dismiss {
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    padding: 0;
    border: none;
    border-radius: 999px;
    background: rgba(0, 0, 0, 0.65);
    color: #fff;
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.15s;
  }

  :global(.poster:hover) .dismiss,
  .dismiss:focus-visible {
    opacity: 1;
  }

  @media (hover: none) {
    .dismiss {
      opacity: 1;
    }
  }

  .status-wanted {
    background: rgba(59, 130, 246, 0.85);
  }
</style>
