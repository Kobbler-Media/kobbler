<script lang="ts">
  import { ratingScale } from '#lib/utils/rating.js';
  import { goto } from '$app/navigation';
  import { onMount, untrack } from 'svelte';
  import { getDiscoverDetail, type DiscoverDetailResponse } from '#lib/api/client.js';
  import type { DiscoverPreview } from '#lib/stores/discover-preview.svelte.js';
  import { getStatsState } from '#lib/stores/stats.svelte.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { libraryStatus, streamingAvailability } from '#lib/utils/releaseAvailability.js';
  import { usePlayback } from '#lib/composables/usePlayback.svelte.js';
  import { useStreamPlayback } from '#lib/composables/useStreamPlayback.svelte.js';
  import ContentPageView from '#lib/components/ContentPageView.svelte';
  import EntityHero from '#lib/components/shared/EntityHero.svelte';
  import HeroActions from '#lib/components/shared/HeroActions.svelte';
  import MetaLine from '#lib/components/shared/MetaLine.svelte';
  import PlaybackArea from '#lib/components/shared/PlaybackArea.svelte';
  import DetailSection from '#lib/components/shared/DetailSection.svelte';
  import CastSection from '#lib/components/shared/CastSection.svelte';
  import ReleaseAvailability from '#lib/components/shared/ReleaseAvailability.svelte';
  import GenreChips from '#lib/components/GenreChips.svelte';
  import VideoPlayer from '#lib/components/VideoPlayer/VideoPlayer.svelte';
  import StreamPlaybackOverlays from '#lib/components/StreamPlaybackOverlays.svelte';
  import DiscoverSeasons from './DiscoverSeasons.svelte';
  import { useDiscoverSeasons } from './useDiscoverSeasons.svelte';
  import {
    autoCreateFor,
    episodeCode,
    parseDiscoverItem,
    streamTarget,
    type EpisodeRef
  } from './discoverDetail';
  import { wantDiscoverEpisodeRef, wantDiscoverItem } from './discoverWant';
  import { createDiscoverReturn } from './discoverReturn';

  interface Props {
    mediaType: string;
    provider: string;
    providerId: string;
    preview: DiscoverPreview | null;
    focusSeason: number | null;
  }

  let { mediaType, provider, providerId, preview: initialPreview, focusSeason }: Props = $props();

  const statsState = getStatsState();
  const stats = $derived(statsState.stats);
  let preview = $state(untrack(() => initialPreview));
  let detail = $state<DiscoverDetailResponse | null>(null);
  let error = $state<string | null>(null);
  let wanting = $state(false);
  let wantingEpisode = $state<string | null>(null);

  const item = $derived(parseDiscoverItem(detail, preview));
  const isTv = $derived(mediaType === 'tv');
  const canStream = $derived(!!stats.kobload_configured && mediaType !== 'book');
  const seasons = useDiscoverSeasons(
    untrack(() => provider),
    untrack(() => providerId)
  );
  const target = $derived(isTv ? streamTarget(item.seasons, seasons.episodes, focusSeason) : null);
  const refKey = (ref: EpisodeRef) => `${ref.season}:${ref.episode}`;

  // The episode the stream composable is searching for; null for a movie.
  let streaming = $state<EpisodeRef | null>(null);

  const playback = usePlayback({
    mediaType: untrack(() => (mediaType === 'tv' ? 'episode' : 'movie')),
    refreshWatchState: async () => {}
  });

  const stream = useStreamPlayback({
    mediaType: untrack(() => (mediaType === 'tv' ? 'tv' : 'movie')),
    getMediaId: () => '',
    getSearchParams: () => ({
      title: item.title,
      year: item.year ?? undefined,
      imdb_id: item.identifiers.imdb_id,
      tmdb_id: item.identifiers.tmdb_id ? parseInt(item.identifiers.tmdb_id) : undefined,
      media_type: mediaType,
      ...(streaming ? { season: streaming.season, episode: streaming.episode } : {})
    }),
    getResumePosition: () => undefined,
    getEstimatedDuration: () => (item.runtime ? item.runtime * 60 : undefined),
    playback,
    onMessage: showMessage,
    getAutoCreate: () => autoCreateFor(item, provider, providerId, streaming),
    want: () => (streaming ? wantEpisode(streaming) : want()),
    canWant: () => true
  });

  const libraryReturn = createDiscoverReturn({
    mediaType: untrack(() => mediaType),
    navigate: (route, opts) => goto(route, opts),
    recheck: () => void loadDetail(),
    isPlaying: () => playback.isActive
  });

  async function loadDetail() {
    error = null;
    try {
      detail = await getDiscoverDetail(mediaType, provider, providerId);
      preview = null;
      libraryReturn.redirect(detail.library_id);
    } catch (e) {
      const msg = e instanceof Error ? e.message : 'Failed to load details';
      if (preview) showMessage(msg);
      else error = msg;
    }
  }

  onMount(() => {
    if (!libraryReturn.redirect(preview?.library_id)) void loadDetail();
    const stopWatching = libraryReturn.watch(window, document);
    return () => {
      stopWatching();
      playback.teardownPreWarm();
    };
  });

  function aim(ref: EpisodeRef | null) {
    const same = (streaming && ref && refKey(streaming) === refKey(ref)) || streaming === ref;
    if (same) return;
    stream.reset();
    streaming = ref;
  }

  // Warm the search for whatever Stream will play, so the press is not a cold start.
  $effect(() => {
    if (!detail || !canStream || playback.isActive || stream.showStreamModal) return;
    const next = target;
    if (isTv && !next) return;
    untrack(() => {
      aim(next);
      void stream.preWarmStream();
    });
  });

  function play(ref: EpisodeRef | null) {
    aim(ref);
    void stream.autoPlay();
  }

  function chooseRelease(ref: EpisodeRef | null) {
    aim(ref);
    stream.openModal();
  }

  async function want() {
    if (wanting) return;
    wanting = true;
    try {
      goto(await wantDiscoverItem(mediaType, item));
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want this');
      wanting = false;
    }
  }

  async function wantEpisode(ref: EpisodeRef) {
    wantingEpisode = refKey(ref);
    try {
      goto(await wantDiscoverEpisodeRef(item, provider, providerId, ref));
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want episode');
      wantingEpisode = null;
    }
  }

  function closePlayer() {
    const played = !!playback.session;
    stream.cancel();
    // Streaming minted the library entry; its page carries resume and next-up.
    libraryReturn.streamEnded(played);
  }

  const streamLabel = $derived(
    isTv ? (target ? `Stream ${episodeCode(target.season, target.episode)}` : null) : 'Stream'
  );
  const playingSubtitle = $derived(
    streaming ? episodeCode(streaming.season, streaming.episode) : undefined
  );
  const playingMediaType = $derived(
    playback.session?.media_type === 'episode' || isTv ? 'episode' : 'movie'
  );
</script>

<svelte:head>
  <title>{item.title ? `${item.title} — Discover` : 'Discover'} — Kobbler</title>
</svelte:head>

{#if error && !preview && !detail}
  <div class="error-page">
    <p>{error}</p>
    <button type="button" onclick={() => goto('/')}>Back to Home</button>
  </div>
{:else if !detail && !preview}
  <div class="loading-page"><div class="spinner"></div></div>
{:else}
  <ContentPageView
    noHeader
    noPadding
    pageBackdropUrl={item.backdropUrl || item.posterUrl}
    onBack={() => history.back()}
    onClose={() => history.back()}
  >
    <EntityHero
      title={item.title}
      coverUrl={item.posterUrl}
      backdropUrl={item.backdropUrl}
      logoUrl={item.logoUrl}
      description={item.description}
      rating={item.rating}
      ratingScale={ratingScale(mediaType)}
      ratingCount={item.ratingCount}
      subtitle={mediaType === 'book' && item.authors.length > 0
        ? item.authors.join(', ')
        : (item.tagline ?? undefined)}
    >
      {#snippet actions()}
        <HeroActions
          actions={[
            (canStream ? !!streamLabel : false) && {
              label: wanting ? 'Adding…' : 'Want',
              icon: 'bookmark_add',
              title: 'Add to your library and download it',
              busy: wanting,
              pinned: true,
              onSelect: want
            }
          ]}
        >
          {#snippet primary()}
            {#if canStream && streamLabel}
              <PlaybackArea
                mode="stream"
                label={streamLabel}
                positionSeconds={null}
                totalSeconds={null}
                watched={false}
                loading={stream.busy}
                variant="hero"
                onPlay={() => play(target)}
                onPickStream={() => chooseRelease(target)}
              />
            {:else}
              <button class="want-primary" type="button" disabled={wanting} onclick={want}>
                <span class="material-icons-outlined">bookmark_add</span>
                {wanting ? 'Adding…' : 'Want'}
              </button>
            {/if}
          {/snippet}
        </HeroActions>
      {/snippet}

      {#snippet meta()}
        <MetaLine>
          {#if item.year}<span>{item.year}</span>{/if}
          {#if item.network}<span>{item.network}</span>{/if}
          {#if item.status}<span class="status-pill">{item.status}</span>{/if}
          {#if item.contentRating}<span class="content-rating">{item.contentRating}</span>{/if}
          {#if item.runtime}<span>{item.runtime} min</span>{/if}
          {#if item.pageCount}<span>{item.pageCount} pages</span>{/if}
          {#if item.publisher}<span>{item.publisher}</span>{/if}
          {#if item.isbn}<span>ISBN {item.isbn}</span>{/if}
        </MetaLine>
        {#if item.genres.length > 0}
          <div class="hero-genres">
            <GenreChips genres={item.genres} size="small" />
          </div>
        {/if}
      {/snippet}
    </EntityHero>

    <div class="full-page-main-grid page-container">
      <div class="details-column">
        {#if isTv}
          <DetailSection label="Seasons">
            <DiscoverSeasons
              seasons={item.seasons}
              loader={seasons}
              initiallyExpanded={target?.season ?? null}
              {canStream}
              isWanting={(ref) => wantingEpisode === refKey(ref)}
              onStream={play}
              onChooseRelease={chooseRelease}
              onWant={wantEpisode}
            />
          </DetailSection>
        {/if}
      </div>
      <div class="actions-column">
        {#if mediaType === 'movie'}
          <ReleaseAvailability
            releaseDate={item.releaseDate}
            releaseDates={item.releaseDates}
            year={item.year}
            streaming={streamingAvailability(canStream, stream.streamLoading, stream.knownOptions)}
            library={libraryStatus(
              detail?.want_status ?? preview?.want_status,
              detail?.is_in_library ?? preview?.is_in_library
            )}
          />
        {/if}
        <CastSection
          members={item.cast.map((m, i) => ({
            key: `${i}-${m.name}`,
            name: m.name,
            role: m.character ?? null,
            imageUrl: m.profile_url ?? null
          }))}
        />
      </div>
    </div>
  </ContentPageView>
{/if}

{#if playback.isActive}
  <VideoPlayer
    sessionResponse={playback.session}
    creationParams={playback.session
      ? {
          media_type: playingMediaType,
          file_id: playback.playingFileId!,
          capabilities: playback.capabilities!
        }
      : undefined}
    mediaInfo={{
      title: item.title,
      subtitle: playingSubtitle,
      artworkUrl: item.posterUrl ?? undefined,
      backdropUrl: item.backdropUrl ?? undefined
    }}
    onClose={closePlayer}
    {...stream.playerProps}
  />
{/if}

<StreamPlaybackOverlays {stream} />

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

  .hero-genres {
    margin-top: 6px;
  }

  .status-pill {
    padding: 2px 8px;
    border-radius: 6px;
    font-weight: 700;
    text-transform: capitalize;
    background: var(--bg-2);
    font-size: 12px;
  }

  .content-rating {
    padding: 2px 8px;
    border: 1px solid var(--muted);
    border-radius: 6px;
    font-weight: 700;
    font-size: 12px;
    text-transform: uppercase;
  }

  .want-primary {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 12px 28px;
    border: none;
    border-radius: 100px;
    background: var(--accent);
    color: var(--bg);
    font-size: 14px;
    font-weight: 700;
    cursor: pointer;
  }

  .want-primary:disabled {
    opacity: 0.6;
    cursor: default;
  }
</style>
