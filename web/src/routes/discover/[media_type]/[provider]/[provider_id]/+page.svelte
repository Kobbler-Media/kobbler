<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { onMount, untrack } from 'svelte';
  import {
    getDiscoverDetail,
    getDiscoverSeason,
    createMovie,
    createTvShow,
    createBook,
    wantMovie,
    setTvShowMonitored,
    wantDiscoverEpisode,
    getMovieWatchState,
    type DiscoverDetailResponse,
    type DiscoverEpisode
  } from '$lib/api/client';
  import type { MovieWatchState, PlaybackSessionResponse } from '$lib/types';
  import { formatCalendarDate, formatDuration } from '$lib/utils/format';
  import StreamOptionsModal from '$lib/components/StreamOptionsModal.svelte';
  import StreamCachingOverlay from '$lib/components/StreamCachingOverlay.svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import { useStreamPlayback } from '$lib/composables/useStreamPlayback.svelte';
  import { getStatsState } from '$lib/stores/stats.svelte';
  import { on, off } from '$lib/services/eventstream.svelte';
  import { consumeDiscoverPreview } from '$lib/stores/discover-preview.svelte';
  import { discoverLibraryRoute } from '$lib/utils/entity-routes';

  const statsState = getStatsState();
  const stats = $derived(statsState.stats);

  let detail = $state<DiscoverDetailResponse | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);

  // Preview data from the card click — shown instantly while full detail loads
  let preview = $state(
    consumeDiscoverPreview(
      $page.params.media_type ?? '',
      $page.params.provider ?? '',
      $page.params.provider_id ?? ''
    )
  );
  let wanting = $state(false);
  // A title reached from search may already be in the library with a part-watched
  // stream behind it. Without this the Stream button restarts it from zero.
  let watchState = $state<MovieWatchState | null>(null);

  // Episode context for TV episode streaming (set by handleEpisodeStream)
  let streamingEpisodeContext = $state<{
    season_number: number;
    episode_number: number;
  } | null>(null);

  async function handleStreamResolved(session: PlaybackSessionResponse) {
    let mediaId = session.media_id;
    let resolvedMediaType = session.media_type;
    // Resolve may return without media_id (dedup/cache hit). Create the entity ourselves.
    if (!mediaId) {
      if (mediaType === 'movie') {
        const { id } = await createMovie({
          title,
          year,
          description,
          poster_url: posterUrl,
          backdrop_url: backdropUrl,
          identifiers,
          genres,
          rating,
          rating_count: ratingCount,
          runtime,
          content_rating: null
        });
        mediaId = id;
        resolvedMediaType = 'movie';
      } else if (mediaType === 'tv') {
        const { id } = await createTvShow({
          title,
          year,
          description,
          poster_url: posterUrl,
          identifiers,
          genres,
          network
        });
        mediaId = id;
        resolvedMediaType = 'tv';
      }
    }
    if (mediaId) {
      const navState = { playbackSession: session };
      if (resolvedMediaType === 'movie') {
        goto(`/movies/${mediaId}`, { state: navState });
      } else if (resolvedMediaType === 'episode') {
        goto(`/tv/episodes/${mediaId}`, { state: navState });
      } else if (resolvedMediaType === 'tv') {
        goto(`/tv/${mediaId}`, { state: navState });
      }
    }
  }

  const stream = useStreamPlayback({
    get mediaType() {
      return streamingEpisodeContext ? ('tv' as const) : ('movie' as const);
    },
    getMediaId: () => '',
    getSearchParams: () => {
      const params: Record<string, unknown> = {
        title,
        year: year ?? undefined,
        imdb_id: identifiers.imdb_id,
        tmdb_id: identifiers.tmdb_id ? parseInt(identifiers.tmdb_id) : undefined,
        media_type: mediaType
      };
      if (streamingEpisodeContext) {
        params.season = streamingEpisodeContext.season_number;
        params.episode = streamingEpisodeContext.episode_number;
      }
      return params;
    },
    getResumePosition: () =>
      watchState?.watched ? undefined : (watchState?.position_seconds ?? undefined),
    getEstimatedDuration: () => (runtime ? runtime * 60 : undefined),
    onMessage: showMessage,
    getAutoCreate: () => {
      const epCtx = streamingEpisodeContext;
      return {
        title,
        year: year ?? undefined,
        identifiers,
        description: description ?? undefined,
        poster_url: posterUrl ?? undefined,
        backdrop_url: backdropUrl ?? undefined,
        genres: genres.length > 0 ? genres : undefined,
        rating: rating ?? undefined,
        rating_count: ratingCount ?? undefined,
        runtime: runtime ?? undefined,
        network: network ?? undefined,
        ...(epCtx
          ? {
              provider,
              provider_id: providerId,
              season_number: epCtx.season_number,
              episode_number: epCtx.episode_number
            }
          : {})
      };
    },
    onResolved: handleStreamResolved
  });

  const mediaType = $derived($page.params.media_type ?? '');
  const provider = $derived($page.params.provider ?? '');
  const providerId = $derived($page.params.provider_id ?? '');

  const isInLibrary = $derived((detail?.is_in_library ?? preview?.is_in_library) === true);
  const isWanted = $derived(
    (detail?.want_status ?? preview?.want_status) != null &&
      (detail?.want_status ?? preview?.want_status) !== 'none'
  );

  const d = $derived(detail?.details ?? {});
  const title = $derived((d.title as string) ?? preview?.title ?? '');
  const year = $derived((d.year as number) ?? preview?.year ?? null);
  const description = $derived(
    (d.description as string) ?? (d.overview as string) ?? preview?.description ?? null
  );
  const posterUrl = $derived((d.poster_url as string) ?? preview?.poster_url ?? null);
  const backdropUrl = $derived((d.backdrop_url as string) ?? preview?.backdrop_url ?? null);
  const genres = $derived((d.genres as string[]) ?? preview?.genres ?? []);
  const rating = $derived((d.rating as number) ?? preview?.rating ?? null);
  const ratingCount = $derived((d.rating_count as number) ?? preview?.rating_count ?? null);
  const runtime = $derived((d.runtime as number) ?? null);
  const status = $derived((d.status as string) ?? null);
  const tagline = $derived((d.tagline as string) ?? null);
  const network = $derived((d.network as string) ?? null);

  // Book-specific
  const authors = $derived((d.authors as string[]) ?? []);
  const publisher = $derived((d.publisher as string) ?? null);
  const pageCount = $derived((d.page_count as number) ?? null);
  const isbn = $derived((d.isbn as string) ?? null);

  const resumeSeconds = $derived(
    watchState && !watchState.watched && (watchState.position_seconds ?? 0) > 0
      ? watchState.position_seconds!
      : null
  );
  const resumeRemaining = $derived.by(() => {
    const total = runtime ? runtime * 60 : null;
    if (resumeSeconds == null || total == null || total - resumeSeconds <= 30) return null;
    return total - resumeSeconds;
  });

  // Movie/TV cast
  const cast = $derived(
    (d.cast as Array<{ name: string; character: string; profile_url?: string }>) ?? []
  );

  const identifiers = $derived(
    (d.identifiers as Record<string, string>) ?? preview?.identifiers ?? {}
  );

  // Seasons (TV)
  const seasons = $derived(
    (d.seasons as Array<{
      season_number: number;
      episode_count?: number;
      air_date?: string;
      title?: string;
    }>) ?? []
  );

  let expandedSeason = $state<number | null>(null);
  let seasonEpisodes = $state<Map<number, DiscoverEpisode[]>>(new Map());
  let seasonLoading = $state<number | null>(null);
  let wantingEpisodes = $state<Set<string>>(new Set());

  // Listen for debrid cache warming completions — mark the option cached and
  // promote it, so auto-play picks a verified stream instead of walking options
  // that warming has since proven blocked.
  $effect(() => {
    const cb = on(
      'stream_warm_ready',
      (data: { info_hash: string; file_id?: number | null; debrid_provider_id: string }) => {
        stream.markCached(data.info_hash, data.file_id ?? null, data.debrid_provider_id);
      }
    );
    return () => off('stream_warm_ready', cb);
  });

  onMount(async () => {
    await loadDetail();
    const season = Number($page.url.searchParams.get('season'));
    if (mediaType === 'tv' && season > 0 && detail) toggleSeason(season);
  });

  async function loadDetail() {
    loading = true;
    error = null;
    try {
      detail = await getDiscoverDetail(mediaType, provider, providerId);
      preview = null;
      if (mediaType === 'movie' && detail.library_id) {
        watchState = await getMovieWatchState(detail.library_id).catch(() => null);
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : 'Failed to load details';
      if (preview) {
        // Show error as toast but keep preview visible
        showMessage(msg);
      } else {
        error = msg;
      }
    } finally {
      loading = false;
    }
  }

  async function handleWant() {
    if ((!detail && !preview) || wanting) return;
    wanting = true;

    try {
      switch (mediaType) {
        case 'movie': {
          const { id } = await createMovie({
            title,
            year,
            description,
            poster_url: posterUrl,
            backdrop_url: backdropUrl,
            identifiers,
            genres,
            rating,
            rating_count: ratingCount,
            runtime,
            content_rating: null
          });
          await wantMovie(id);
          goto(`/movies/${id}`);
          break;
        }
        case 'tv': {
          const { id } = await createTvShow({
            title,
            year,
            description,
            poster_url: posterUrl,
            identifiers,
            genres,
            network
          });
          await setTvShowMonitored(id, true, 'all');
          goto(`/tv/${id}`);
          break;
        }
        case 'book': {
          const { id } = await createBook({
            title,
            authors: authors.map((a: string) => ({ name: a })),
            description,
            cover_url: posterUrl,
            identifiers,
            rating,
            rating_count: ratingCount,
            want: true
          });
          goto(`/library/books/${id}`);
          break;
        }
      }
    } catch (e) {
      console.error('Failed to want:', e);
      wanting = false;
    }
  }

  function goToLibrary() {
    const target = discoverLibraryRoute(mediaType, detail?.library_id ?? preview?.library_id);
    if (target) goto(target);
  }

  // --- Seasons ---

  async function toggleSeason(seasonNumber: number) {
    if (expandedSeason === seasonNumber) {
      expandedSeason = null;
      return;
    }
    expandedSeason = seasonNumber;
    if (!seasonEpisodes.has(seasonNumber)) {
      seasonLoading = seasonNumber;
      try {
        const detail = await getDiscoverSeason(provider, providerId, seasonNumber);
        seasonEpisodes = new Map(seasonEpisodes).set(seasonNumber, detail.episodes);
      } catch (e) {
        console.error('Failed to load season:', e);
      } finally {
        seasonLoading = null;
      }
    }
  }

  function handleEpisodeStream(seasonNumber: number, episode: DiscoverEpisode) {
    streamingEpisodeContext = {
      season_number: seasonNumber,
      episode_number: episode.episode_number
    };
    // Reset pre-warmed state since episode-specific search params differ from show-level
    stream.reset();
    stream.autoPlay();
  }

  function handleEpisodeStreamPick(seasonNumber: number, episode: DiscoverEpisode) {
    streamingEpisodeContext = {
      season_number: seasonNumber,
      episode_number: episode.episode_number
    };
    stream.reset();
    stream.openModal();
  }

  async function handleEpisodeWant(seasonNumber: number, episode: DiscoverEpisode) {
    const key = `${seasonNumber}-${episode.episode_number}`;
    wantingEpisodes = new Set([...wantingEpisodes, key]);
    try {
      const result = await wantDiscoverEpisode({
        provider,
        provider_id: providerId,
        season_number: seasonNumber,
        episode_number: episode.episode_number,
        title,
        year: year ?? undefined,
        identifiers,
        description: description ?? undefined,
        poster_url: posterUrl ?? undefined,
        backdrop_url: backdropUrl ?? undefined,
        genres: genres.length > 0 ? genres : undefined,
        rating: rating ?? undefined,
        rating_count: ratingCount ?? undefined,
        runtime: runtime ?? undefined,
        network: network ?? undefined
      });
      goto(`/tv/${result.show_id}`);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want episode');
      const next = new Set(wantingEpisodes);
      next.delete(key);
      wantingEpisodes = next;
    }
  }

  // Pre-warm stream search when detail loads.
  //   movie -> search the whole movie (imdb_id is sufficient).
  //   tv    -> search a specific episode. No watch history here, so default
  //            to S01E01. User-driven episode clicks below overwrite this.
  //   book  -> skip.
  $effect(() => {
    const d = detail;
    const mt = mediaType;
    if (!d) return;
    if (mt === 'movie') {
      untrack(() => stream.preWarmStream());
    } else if (mt === 'tv') {
      untrack(() => {
        if (!streamingEpisodeContext) {
          streamingEpisodeContext = { season_number: 1, episode_number: 1 };
        }
        stream.preWarmStream();
      });
    }
  });
</script>

<svelte:head>
  <title>{title ? `${title} — Discover` : 'Discover'} — Kobbler</title>
</svelte:head>

{#if loading && !preview}
  <div class="loading-page">
    <div class="spinner"></div>
  </div>
{:else if error && !preview}
  <div class="error-page">
    <p>{error}</p>
    <button type="button" onclick={() => goto('/')}>Back to Home</button>
  </div>
{:else if detail || preview}
  <div class="detail-page">
    <!-- Backdrop -->
    {#if backdropUrl}
      <div class="backdrop" style="background-image: url({backdropUrl})"></div>
    {/if}

    <div class="content">
      <button class="back-btn" type="button" onclick={() => history.back()}>
        <svg viewBox="0 0 24 24" width="18" height="18" fill="currentColor">
          <path d="M20 11H7.83l5.59-5.59L12 4l-8 8 8 8 1.41-1.41L7.83 13H20v-2z"></path>
        </svg>
        Back
      </button>

      <div class="detail-layout">
        <!-- Poster -->
        <div class="poster-col">
          {#if posterUrl}
            <img class="poster" src={posterUrl} alt={title} />
          {:else}
            <div class="poster poster-empty">
              <svg viewBox="0 0 24 24" width="48" height="48" fill="currentColor">
                <path
                  d="M18 4l2 4h-3l-2-4h-2l2 4h-3l-2-4H8l2 4H7L5 4H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V4h-4z"
                ></path>
              </svg>
            </div>
          {/if}
        </div>

        <!-- Info -->
        <div class="info-col">
          <h1 class="title">
            {title}
            {#if year}
              <span class="year">({year})</span>
            {/if}
          </h1>

          {#if tagline}
            <p class="tagline">"{tagline}"</p>
          {/if}

          <div class="meta-row">
            {#if mediaType === 'book' && authors.length > 0}
              <span class="meta-item">{authors.join(', ')}</span>
            {/if}
            {#if genres.length > 0}
              <span class="meta-item">{genres.slice(0, 4).join(', ')}</span>
            {/if}
            {#if runtime}
              <span class="meta-item">{runtime} min</span>
            {/if}
            {#if network}
              <span class="meta-item">{network}</span>
            {/if}
            {#if status}
              <span class="meta-item meta-status">{status}</span>
            {/if}
            {#if pageCount}
              <span class="meta-item">{pageCount} pages</span>
            {/if}
            {#if publisher}
              <span class="meta-item">{publisher}</span>
            {/if}
          </div>

          {#if rating != null}
            <div class="rating-row">
              <svg viewBox="0 0 24 24" width="16" height="16" fill="#fbbf24">
                <path
                  d="M12 17.27L18.18 21l-1.64-7.03L22 9.24l-7.19-.61L12 2 9.19 8.63 2 9.24l5.46 4.73L5.82 21z"
                ></path>
              </svg>
              <span class="rating-value">{rating.toFixed(1)}</span>
              {#if ratingCount != null}
                <span class="rating-count">({ratingCount.toLocaleString()} ratings)</span>
              {/if}
            </div>
          {/if}

          {#if description}
            <p class="description">{description}</p>
          {/if}

          <!-- Actions -->
          <div class="actions">
            {#if isInLibrary}
              <button class="action-btn action-library" type="button" onclick={goToLibrary}>
                <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor">
                  <path d="M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41L9 16.17z"></path>
                </svg>
                In Library — View
              </button>
            {:else if isWanted}
              <button class="action-btn action-wanted" type="button" onclick={goToLibrary}>
                <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor">
                  <path
                    d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm0 18c-4.42 0-8-3.58-8-8s3.58-8 8-8 8 3.58 8 8-3.58 8-8 8zm-1-13h2v6h-2zm0 8h2v2h-2z"
                  ></path>
                </svg>
                Already Wanted — View
              </button>
            {:else}
              <button
                class="action-btn action-want"
                type="button"
                disabled={wanting}
                onclick={handleWant}
              >
                {#if wanting}
                  <div class="btn-spinner"></div>
                  Adding...
                {:else}
                  <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor">
                    <path d="M19 13h-6v6h-2v-6H5v-2h6V5h2v6h6v2z"></path>
                  </svg>
                  Want
                {/if}
              </button>
            {/if}
            {#if mediaType === 'movie' && stats.kobload_configured}
              <div class="action-stream-split">
                <button
                  class="action-btn action-stream"
                  type="button"
                  onclick={stream.autoPlay}
                  disabled={stream.streamLoading}
                >
                  {#if stream.streamLoading}
                    <div class="btn-spinner stream-spinner"></div>
                    Searching...
                  {:else}
                    <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor">
                      <path d="M8 5v14l11-7z"></path>
                    </svg>
                    {resumeSeconds != null ? 'Resume' : 'Stream'}
                  {/if}
                </button>
                <button
                  class="action-btn action-stream action-stream-pick"
                  type="button"
                  onclick={stream.openModal}
                  title="Pick a specific stream"
                  aria-label="Pick a specific stream"
                >
                  <span class="material-icons-outlined">menu</span>
                </button>
              </div>
            {/if}
          </div>

          {#if resumeSeconds != null && mediaType === 'movie' && stats.kobload_configured}
            <div class="resume-progress">
              {#if runtime}
                <div class="resume-track">
                  <div
                    class="resume-fill"
                    style="width: {Math.min(100, (resumeSeconds / (runtime * 60)) * 100)}%"
                  ></div>
                </div>
              {/if}
              <div class="resume-line">
                <span>
                  {formatDuration(resumeSeconds)}{runtime
                    ? ` / ${formatDuration(runtime * 60)}`
                    : ''}
                </span>
                {#if resumeRemaining != null}
                  <span>{formatDuration(resumeRemaining)} left</span>
                {/if}
              </div>
            </div>
          {/if}

          <!-- Cast (movies/TV) -->
          {#if cast.length > 0}
            <div class="cast-section">
              <h3 class="section-title">Cast</h3>
              <div class="cast-grid">
                {#each cast.slice(0, 8) as member (member.name)}
                  <div class="cast-member">
                    {#if member.profile_url}
                      <img
                        class="cast-photo"
                        src={member.profile_url}
                        alt={member.name}
                        loading="lazy"
                      />
                    {:else}
                      <div class="cast-photo cast-photo-empty">
                        <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor">
                          <path
                            d="M12 12c2.21 0 4-1.79 4-4s-1.79-4-4-4-4 1.79-4 4 1.79 4 4 4zm0 2c-2.67 0-8 1.34-8 4v2h16v-2c0-2.66-5.33-4-8-4z"
                          ></path>
                        </svg>
                      </div>
                    {/if}
                    <div class="cast-info">
                      <span class="cast-name">{member.name}</span>
                      {#if member.character}
                        <span class="cast-role">{member.character}</span>
                      {/if}
                    </div>
                  </div>
                {/each}
              </div>
            </div>
          {/if}

          <!-- Seasons (TV) -->
          {#if mediaType === 'tv' && seasons.length > 0}
            <div class="seasons-section">
              <h3 class="section-title">Seasons</h3>
              {#each seasons.filter((s) => s.season_number > 0) as season (season.season_number)}
                <div class="season-group">
                  <button
                    class="season-header"
                    class:expanded={expandedSeason === season.season_number}
                    onclick={() => toggleSeason(season.season_number)}
                  >
                    <span class="season-title">
                      {season.title ?? `Season ${season.season_number}`}
                    </span>
                    {#if season.episode_count}
                      <span class="season-meta">{season.episode_count} episodes</span>
                    {/if}
                    {#if seasonLoading === season.season_number}
                      <div class="season-spinner"></div>
                    {:else}
                      <span class="material-icons-outlined season-chevron">
                        {expandedSeason === season.season_number ? 'expand_less' : 'expand_more'}
                      </span>
                    {/if}
                  </button>
                  {#if expandedSeason === season.season_number && seasonEpisodes.has(season.season_number)}
                    <div class="episodes-list">
                      {#each seasonEpisodes.get(season.season_number) ?? [] as ep (ep.episode_number)}
                        {@const epKey = `${season.season_number}-${ep.episode_number}`}
                        <div class="episode-row">
                          <span class="ep-number">{ep.episode_number}</span>
                          <div class="ep-info">
                            <span class="ep-title"
                              >{ep.title ?? `Episode ${ep.episode_number}`}</span
                            >
                            {#if ep.air_date}
                              <span class="ep-date">{formatCalendarDate(ep.air_date, 'short')}</span
                              >
                            {/if}
                          </div>
                          <div class="ep-actions">
                            <button
                              class="ep-want-btn"
                              disabled={wantingEpisodes.has(epKey)}
                              onclick={() => handleEpisodeWant(season.season_number, ep)}
                            >
                              {#if wantingEpisodes.has(epKey)}
                                <div class="ep-spinner"></div>
                              {:else}
                                <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor">
                                  <path d="M17 3H7c-1.1 0-2 .9-2 2v16l7-3 7 3V5c0-1.1-.9-2-2-2z"
                                  ></path>
                                </svg>
                              {/if}
                              Want
                            </button>
                            {#if stats.kobload_configured}
                              <div class="ep-stream-split">
                                <button
                                  class="ep-stream-btn"
                                  onclick={() => handleEpisodeStream(season.season_number, ep)}
                                >
                                  <svg
                                    viewBox="0 0 24 24"
                                    width="14"
                                    height="14"
                                    fill="currentColor"
                                  >
                                    <path d="M8 5v14l11-7z"></path>
                                  </svg>
                                  Stream
                                </button>
                                <button
                                  class="ep-stream-btn ep-stream-pick"
                                  onclick={() => handleEpisodeStreamPick(season.season_number, ep)}
                                  title="Pick a specific stream"
                                  aria-label="Pick a specific stream"
                                >
                                  <span class="material-icons-outlined">menu</span>
                                </button>
                              </div>
                            {/if}
                          </div>
                        </div>
                      {/each}
                    </div>
                  {/if}
                </div>
              {/each}
            </div>
          {/if}

          <!-- ISBN (books) -->
          {#if isbn}
            <div class="isbn-row">
              <span class="isbn-label">ISBN</span>
              <span class="isbn-value">{isbn}</span>
            </div>
          {/if}
        </div>
      </div>
    </div>
  </div>
{/if}

{#if stream.showStreamModal}
  <StreamOptionsModal
    options={stream.streamOptions}
    loading={stream.streamLoading}
    errors={stream.streamErrors}
    onClose={stream.closeModal}
    onPlay={stream.playOption}
  />
{/if}

{#if stream.isCaching}
  <StreamCachingOverlay
    progress={stream.cachingProgress ?? 0}
    error={stream.cachingError}
    onCancel={stream.closeCachingOverlay}
    onRetry={stream.retryCaching}
    onPickStream={() => {
      stream.closeCachingOverlay();
      stream.openModal();
    }}
  />
{/if}

<style>
  .loading-page,
  .error-page {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    min-height: 300px;
    gap: 16px;
    color: var(--muted);
  }

  .error-page button {
    padding: 8px 16px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--card);
    color: var(--ink);
    cursor: pointer;
    font-size: 14px;
  }

  .spinner {
    width: 28px;
    height: 28px;
    border: 2px solid var(--ring);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.6s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .detail-page {
    position: relative;
  }

  .backdrop {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 320px;
    background-size: cover;
    background-position: center top;
    opacity: 0.15;
    mask-image: linear-gradient(to bottom, black 0%, transparent 100%);
    -webkit-mask-image: linear-gradient(to bottom, black 0%, transparent 100%);
    pointer-events: none;
  }

  .content {
    position: relative;
    z-index: 1;
    padding: 24px;
    max-width: 960px;
  }

  .back-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--card);
    color: var(--muted);
    font-size: 13px;
    cursor: pointer;
    margin-bottom: 20px;
    transition:
      color 0.15s,
      border-color 0.15s;
  }

  .back-btn:hover {
    color: var(--ink);
    border-color: var(--ink);
  }

  .detail-layout {
    display: flex;
    gap: 28px;
  }

  .poster-col {
    flex-shrink: 0;
  }

  .poster {
    width: 220px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    display: block;
  }

  .poster-empty {
    width: 220px;
    aspect-ratio: 2 / 3;
    background: var(--bg-2);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--muted);
    opacity: 0.3;
  }

  .info-col {
    flex: 1;
    min-width: 0;
  }

  .title {
    font-size: 26px;
    font-weight: 800;
    color: var(--ink);
    margin: 0 0 4px;
    letter-spacing: -0.5px;
    line-height: 1.2;
  }

  .year {
    font-weight: 400;
    color: var(--muted);
    font-size: 22px;
  }

  .tagline {
    font-size: 14px;
    color: var(--muted);
    font-style: italic;
    margin: 0 0 12px;
  }

  .meta-row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 12px;
  }

  .meta-item {
    font-size: 12px;
    color: var(--muted);
    padding: 3px 8px;
    border: 1px solid var(--ring);
    border-radius: 6px;
    white-space: nowrap;
  }

  .meta-status {
    text-transform: capitalize;
  }

  .rating-row {
    display: flex;
    align-items: center;
    gap: 5px;
    margin-bottom: 14px;
  }

  .rating-value {
    font-size: 15px;
    font-weight: 700;
    color: var(--ink);
    font-variant-numeric: tabular-nums;
  }

  .rating-count {
    font-size: 12px;
    color: var(--muted);
  }

  .description {
    font-size: 14px;
    color: var(--ink);
    line-height: 1.6;
    margin: 0 0 20px;
    opacity: 0.85;
  }

  /* Actions */
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    margin-bottom: 24px;
  }

  .action-btn {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 10px 20px;
    border-radius: 8px;
    font-size: 14px;
    font-weight: 600;
    cursor: pointer;
    transition:
      background 0.15s,
      color 0.15s,
      transform 0.1s;
    border: none;
  }

  .action-btn:active:not(:disabled) {
    transform: scale(0.97);
  }

  .action-want {
    background: var(--accent);
    color: #fff;
  }

  .action-want:hover:not(:disabled) {
    filter: brightness(1.1);
  }

  .action-want:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .action-library {
    background: rgba(34, 197, 94, 0.15);
    color: rgb(34, 197, 94);
    border: 1px solid rgba(34, 197, 94, 0.3);
  }

  .action-library:hover {
    background: rgba(34, 197, 94, 0.25);
  }

  .action-wanted {
    background: rgba(59, 130, 246, 0.15);
    color: rgb(59, 130, 246);
    border: 1px solid rgba(59, 130, 246, 0.3);
  }

  .action-wanted:hover {
    background: rgba(59, 130, 246, 0.25);
  }

  .action-stream {
    background: rgba(168, 85, 247, 0.15);
    color: rgb(168, 85, 247);
    border: 1px solid rgba(168, 85, 247, 0.3);
  }

  .action-stream:hover:not(:disabled) {
    background: rgba(168, 85, 247, 0.25);
  }

  .action-stream:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .action-stream-split {
    display: inline-flex;
    align-items: stretch;
    gap: 0;
  }

  .action-stream-split .action-stream:first-child {
    border-top-right-radius: 0;
    border-bottom-right-radius: 0;
    border-right: none;
  }

  .action-stream-pick {
    padding: 0 10px;
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
    border-left: 1px solid rgba(168, 85, 247, 0.45);
  }

  .action-stream-pick :global(.material-icons-outlined) {
    font-size: 18px;
  }

  .stream-spinner {
    border-color: rgba(168, 85, 247, 0.3);
    border-top-color: rgb(168, 85, 247);
  }

  .btn-spinner {
    width: 14px;
    height: 14px;
    border: 2px solid rgba(255, 255, 255, 0.3);
    border-top-color: #fff;
    border-radius: 50%;
    animation: spin 0.6s linear infinite;
  }

  .resume-progress {
    max-width: 360px;
    margin-top: 12px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .resume-track {
    height: 3px;
    background: var(--ring);
    border-radius: 2px;
    overflow: hidden;
  }

  .resume-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 2px;
  }

  .resume-line {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-size: 12px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  /* Cast */
  .section-title {
    font-size: 14px;
    font-weight: 700;
    color: var(--ink);
    margin: 0 0 10px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    font-size: 11px;
  }

  .cast-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 8px;
    margin-bottom: 20px;
  }

  .cast-member {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .cast-photo {
    width: 36px;
    height: 36px;
    border-radius: 50%;
    object-fit: cover;
    border: 1px solid var(--ring);
    flex-shrink: 0;
  }

  .cast-photo-empty {
    background: var(--bg-2);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--muted);
  }

  .cast-info {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .cast-name {
    font-size: 12px;
    font-weight: 600;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .cast-role {
    font-size: 11px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .isbn-row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
  }

  .isbn-label {
    color: var(--muted);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    font-size: 10px;
  }

  .isbn-value {
    color: var(--ink);
    font-variant-numeric: tabular-nums;
  }

  /* Seasons */
  .seasons-section {
    margin-top: 32px;
  }

  .season-group {
    border: 1px solid var(--ring);
    border-radius: 10px;
    overflow: hidden;
    margin-bottom: 8px;
  }

  .season-header {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 12px 16px;
    background: var(--card);
    border: none;
    color: var(--ink);
    font-size: 14px;
    cursor: pointer;
    transition: background 0.15s;
  }

  .season-header:hover {
    background: color-mix(in srgb, var(--card) 90%, var(--accent));
  }

  .season-title {
    font-weight: 600;
  }

  .season-meta {
    color: var(--muted);
    font-size: 12px;
    margin-left: auto;
  }

  .season-chevron {
    font-size: 20px;
    color: var(--muted);
  }

  .season-spinner {
    width: 16px;
    height: 16px;
    border: 2px solid var(--ring);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.6s linear infinite;
    margin-left: auto;
  }

  .episodes-list {
    border-top: 1px solid var(--ring);
  }

  .episode-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--ring);
  }

  .episode-row:last-child {
    border-bottom: none;
  }

  .ep-number {
    width: 24px;
    text-align: center;
    font-size: 13px;
    font-weight: 600;
    color: var(--muted);
    flex-shrink: 0;
  }

  .ep-info {
    flex: 1;
    min-width: 0;
  }

  .ep-title {
    font-size: 13px;
    color: var(--ink);
  }

  .ep-date {
    font-size: 11px;
    color: var(--muted);
    margin-left: 8px;
  }

  .ep-stream-btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 4px 10px;
    border: 1px solid var(--accent);
    border-radius: 6px;
    background: transparent;
    color: var(--accent);
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
    flex-shrink: 0;
    transition:
      background 0.15s,
      color 0.15s;
  }

  .ep-stream-btn:hover {
    background: var(--accent);
    color: #fff;
  }

  .ep-stream-split {
    display: inline-flex;
    align-items: stretch;
    gap: 0;
  }

  .ep-stream-split .ep-stream-btn:first-child {
    border-top-right-radius: 0;
    border-bottom-right-radius: 0;
    border-right: none;
  }

  .ep-stream-pick {
    padding: 0 8px;
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
    border-left: 1px solid var(--accent);
  }

  .ep-stream-pick :global(.material-icons-outlined) {
    font-size: 14px;
  }

  .ep-actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
  }

  .ep-want-btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 4px 10px;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
    transition:
      background 0.15s,
      color 0.15s,
      border-color 0.15s;
  }

  .ep-want-btn:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }

  .ep-want-btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .ep-spinner {
    width: 12px;
    height: 12px;
    border: 2px solid var(--ring);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.6s linear infinite;
  }

  /* Responsive */
  @media (max-width: 640px) {
    .content {
      padding: 16px;
    }

    .detail-layout {
      flex-direction: column;
      gap: 20px;
    }

    .poster-col {
      display: flex;
      justify-content: center;
    }

    .poster {
      width: 180px;
    }

    .poster-empty {
      width: 180px;
    }

    .title {
      font-size: 22px;
    }

    .cast-grid {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
