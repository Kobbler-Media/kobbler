<script lang="ts">
  import { VIDEO_RATING_SCALE } from '#lib/utils/rating.js';
  import { onMount } from 'svelte';
  import type { TvShow } from '#lib/types.js';
  import { getTvShowRating, updateTvShowRating, deleteTvShowRating } from '#lib/api/client.js';
  import PersonCard from '#lib/components/shared/PersonCard.svelte';
  import GenreChips from '#lib/components/GenreChips.svelte';
  import DualRatingStars from '#lib/components/DualRatingStars.svelte';
  import { SearchSelect } from '@koblibs/ui';
  import EntityHero from '../shared/EntityHero.svelte';
  import { formatShowStatus } from '#lib/utils/format.js';
  import { hasOutstandingWants } from '#lib/utils/want-status.js';

  interface Props {
    show: TvShow;
    onMonitorToggle: (monitored: boolean, mode: string) => void;
    onAutoWantChange: (mode: string) => void;
    onWantAllMissing: () => void;
    onUnwantAll?: () => void;
    onBulkWatchToggle?: (watched: boolean) => void;
    onSeasonClick?: (seasonId: string) => void;
    onCastClick?: (personId: string, personName: string) => void;
    onGenreClick?: (genre: string) => void;
    onExpand?: () => void;
  }

  let {
    show,
    onMonitorToggle,
    onAutoWantChange,
    onBulkWatchToggle,
    onWantAllMissing,
    onUnwantAll,
    onSeasonClick,
    onCastClick,
    onGenreClick,
    onExpand: _onExpand
  }: Props = $props();

  let descriptionExpanded = $state(false);
  let showRating = $state<number | null>(null);

  async function fetchShowRating() {
    try {
      const result = await getTvShowRating(show.id);
      showRating = result.rating;
    } catch {
      showRating = null;
    }
  }

  async function handleRatingChange(rating: number | null) {
    try {
      if (rating === null) {
        await deleteTvShowRating(show.id);
      } else {
        await updateTvShowRating(show.id, rating);
      }
      showRating = rating;
    } catch (e) {
      console.error('Failed to update show rating:', e);
    }
  }

  onMount(() => {
    fetchShowRating();
  });

  $effect(() => {
    show.id;
    fetchShowRating();
  });

  const allWatched = $derived(
    (show.seasons ?? []).length > 0 &&
      (show.seasons ?? []).every((s) => s.episodes.every((e) => e.watched === true))
  );

  const sortedSeasons = $derived(
    [...(show.seasons || [])].sort((a, b) => a.season_sort_order - b.season_sort_order)
  );

  const anyWanted = $derived(
    hasOutstandingWants((show.seasons ?? []).flatMap((s) => s.episodes ?? []))
  );

  const descriptionNeedsExpand = $derived(!!show.description && show.description.length > 200);

  const autoWantMode = $derived(show.auto_want_mode || 'none');

  const autoWantOptions = [
    { value: 'none', label: 'None' },
    { value: 'all', label: 'All Episodes' },
    { value: 'new_only', label: 'New Only' },
    { value: 'fill_gaps', label: 'Fill Gaps' }
  ];
</script>

<div class="show-detail">
  <!-- Unified Hero Section -->
  <EntityHero
    title={show.title}
    coverUrl={show.poster_url}
    backdropUrl={show.backdrop_url}
    logoUrl={show.logo_url}
    embedded={true}
  >
    {#snippet ratingContent()}
      <DualRatingStars
        communityRating={show.rating}
        communityScale={VIDEO_RATING_SCALE}
        communityRatingCount={show.rating_count ?? null}
        userRating={showRating}
        size="small"
        editable={true}
        onRatingChange={handleRatingChange}
      />
    {/snippet}

    {#snippet subtitle()}
      <div class="panel-meta-row">
        {#if show.year}
          <span class="panel-meta-item">{show.year}</span>
        {/if}
        {#if show.status}
          <span class="meta-item status-pill">{formatShowStatus(show.status)}</span>
        {/if}
        {#if show.network}
          <span class="panel-meta-item">{show.network}</span>
        {/if}
        {#if show.content_rating}
          <span class="meta-item rating-badge">{show.content_rating}</span>
        {/if}
      </div>
      {#if show.genres?.length > 0}
        <div class="hero-genres">
          <GenreChips genres={show.genres} size="small" clickable={!!onGenreClick} {onGenreClick} />
        </div>
      {/if}
    {/snippet}

    {#snippet actions()}
      <button
        class="monitor-pill"
        class:active={show.is_monitored}
        type="button"
        onclick={() => onMonitorToggle(!show.is_monitored, autoWantMode)}
      >
        <span class="material-icons-outlined">
          {show.is_monitored ? 'star' : 'star_outline'}
        </span>
        {show.is_monitored ? 'Monitored' : 'Monitor'}
      </button>
    {/snippet}
  </EntityHero>

  <!-- Auto-want & Want Missing -->
  <div class="panel-section auto-want-section">
    <div class="auto-want-row">
      <div class="auto-want-select">
        <h4 class="panel-section-title">Auto-want</h4>
        <SearchSelect value={autoWantMode} items={autoWantOptions} onchange={onAutoWantChange} />
      </div>
      {#if onBulkWatchToggle}
        <button
          class="icon-btn small"
          class:watched={allWatched}
          type="button"
          title={allWatched ? 'Mark all unwatched' : 'Mark all watched'}
          onclick={() => onBulkWatchToggle!(!allWatched)}
        >
          <span class="material-icons-outlined">
            {allWatched ? 'visibility_off' : 'visibility'}
          </span>
        </button>
      {/if}
      <button
        class="icon-btn small"
        type="button"
        title="Want all missing episodes"
        onclick={onWantAllMissing}
      >
        <span class="material-icons-outlined">playlist_add</span>
      </button>
      {#if onUnwantAll && anyWanted}
        <button
          class="icon-btn small"
          type="button"
          title="Unwant every outstanding episode"
          onclick={onUnwantAll}
        >
          <span class="material-icons-outlined">playlist_remove</span>
        </button>
      {/if}
    </div>
  </div>

  <!-- Overview -->
  {#if show.description}
    <div class="panel-section">
      <h4 class="panel-section-title">Overview</h4>
      <p class="panel-overview" class:collapsed={!descriptionExpanded && descriptionNeedsExpand}>
        {show.description}
      </p>
      {#if descriptionNeedsExpand}
        <button
          class="panel-expand-link"
          type="button"
          onclick={() => (descriptionExpanded = !descriptionExpanded)}
        >
          {descriptionExpanded ? 'Show less' : 'Show more'}
        </button>
      {/if}
    </div>
  {/if}

  <!-- Seasons -->
  {#if show.seasons && show.seasons.length > 0}
    <div class="panel-section">
      <h4 class="panel-section-title">Seasons</h4>
      <div class="seasons-list">
        {#each sortedSeasons as season (season.id)}
          <button class="season-item" type="button" onclick={() => onSeasonClick?.(season.id)}>
            {#if season.poster_url}
              <img class="season-poster" src={season.poster_url} alt="" />
            {:else}
              <div class="season-poster-placeholder">S{season.season_sort_order}</div>
            {/if}
            <div class="season-info">
              <span class="season-title"
                >{season.title || `Season ${season.season_sort_order}`}</span
              >
              <span class="season-meta"
                >{season.episode_count || season.episodes?.length || 0} episodes</span
              >
            </div>
            <span class="material-icons-outlined chevron">chevron_right</span>
          </button>
        {/each}
      </div>
    </div>
  {/if}

  <!-- Cast -->
  {#if show.cast && show.cast.length > 0}
    <div class="panel-section">
      <h4 class="panel-section-title">Cast</h4>
      <div class="panel-cast-list">
        {#each show.cast.slice(0, 10) as member (member.id)}
          {#if onCastClick}
            <button
              type="button"
              class="panel-cast-btn"
              onclick={() => onCastClick?.(member.person_id, member.person_name)}
            >
              <PersonCard
                name={member.person_name}
                role={member.character_name}
                imageUrl={member.person_image_url}
              />
            </button>
          {:else}
            <PersonCard
              name={member.person_name}
              role={member.character_name}
              imageUrl={member.person_image_url}
            />
          {/if}
        {/each}
        {#if show.cast.length > 10}
          <div class="panel-more-count">+{show.cast.length - 10} more</div>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .show-detail {
    display: flex;
    flex-direction: column;
    gap: 0;
  }

  .status-pill {
    padding: 0 5px;
    border-radius: 3px;
    font-weight: 600;
    text-transform: capitalize;
    background: var(--bg-2);
    font-size: 10px;
  }

  .rating-badge {
    padding: 0 5px;
    border: 1px solid var(--muted);
    border-radius: 3px;
    font-weight: 600;
    font-size: 10px;
  }

  .monitor-pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    border-radius: 100px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    font-size: 11px;
    font-weight: 700;
    cursor: pointer;
    transition: all 0.15s;
  }

  .monitor-pill:hover {
    background: rgba(139, 92, 246, 0.15);
    border-color: var(--monitored);
    color: var(--monitored);
  }

  .monitor-pill.active {
    background: rgba(139, 92, 246, 0.15);
    border-color: var(--monitored);
    color: var(--monitored);
  }

  .monitor-pill :global(.material-icons-outlined) {
    font-size: 14px;
  }

  .auto-want-section {
    padding: 12px 0;
  }

  .auto-want-row {
    display: flex;
    align-items: flex-end;
    gap: 8px;
  }

  .auto-want-select {
    flex: 1;
    min-width: 0;
  }

  .icon-btn.small {
    width: 28px;
    height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    flex-shrink: 0;
  }

  .icon-btn.small:hover {
    background: var(--bg);
    color: var(--ink);
  }

  .icon-btn.small.watched {
    color: var(--accent);
    border-color: var(--accent-dim);
  }

  .icon-btn.small .material-icons-outlined {
    font-size: 16px;
  }

  .hero-genres {
    margin-top: 6px;
  }

  /* --- Seasons --- */
  .seasons-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .season-item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px;
    border: none;
    background: transparent;
    cursor: pointer;
    text-align: left;
    border-radius: 8px;
    transition: background 0.15s;
    color: inherit;
    font: inherit;
    width: 100%;
  }

  .season-item:hover {
    background: var(--bg-2);
  }

  .season-poster {
    width: 40px;
    height: 56px;
    border-radius: 4px;
    object-fit: cover;
    flex-shrink: 0;
  }

  .season-poster-placeholder {
    width: 40px;
    height: 56px;
    border-radius: 4px;
    background: var(--bg-2);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 12px;
    font-weight: 700;
    color: var(--muted);
    flex-shrink: 0;
  }

  .season-info {
    display: flex;
    flex-direction: column;
    gap: 1px;
    flex: 1;
    min-width: 0;
  }

  .season-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
  }

  .season-meta {
    font-size: 12px;
    color: var(--muted);
  }

  .chevron {
    font-size: 18px;
    color: var(--muted);
  }
</style>
