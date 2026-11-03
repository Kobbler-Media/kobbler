<script lang="ts">
  import { VIDEO_RATING_SCALE } from '#lib/utils/rating.js';
  import { onMount } from 'svelte';
  import type {
    Movie,
    MovieFile,
    MovieWatchState,
    SubtitleTrackInfo,
    ExternalSubtitleInfo
  } from '#lib/types.js';
  import {
    getMovieFiles,
    getMovieWatchState,
    updateMovieWatchState,
    deleteMovieFile,
    rejectDownload,
    replaceFile,
    searchMovieReleases,
    queueMovieRelease,
    wantMovie,
    getMovieTracks
  } from '#lib/api/client.js';
  import { getStatsState } from '#lib/stores/stats.svelte.js';
  import { usePlayback } from '#lib/composables/usePlayback.svelte.js';
  import { useStreamPlayback } from '#lib/composables/useStreamPlayback.svelte.js';
  import VideoPlayer from '#lib/components/VideoPlayer/VideoPlayer.svelte';
  import { showMessage } from '#lib/context/app.svelte.js';
  import ContentPageView from '#lib/components/ContentPageView.svelte';
  import CastSection from '#lib/components/shared/CastSection.svelte';
  import PlaybackArea from '#lib/components/shared/PlaybackArea.svelte';
  import FilesList from '#lib/components/shared/FilesList.svelte';
  import GenreChips from '#lib/components/GenreChips.svelte';
  import { isActivelyWanted } from '#lib/utils/want-status.js';
  import DualRatingStars from '#lib/components/DualRatingStars.svelte';
  import ReleaseSearchModal from '#lib/components/ReleaseSearchModal.svelte';
  import SubtitlesList from '#lib/components/shared/SubtitlesList.svelte';
  import SubtitleSearchModal from '#lib/components/shared/SubtitleSearchModal.svelte';
  import StreamPlaybackOverlays from '#lib/components/StreamPlaybackOverlays.svelte';
  import ReleaseAvailability from '#lib/components/shared/ReleaseAvailability.svelte';
  import { libraryStatus, streamingAvailability } from '#lib/utils/releaseAvailability.js';
  import { useTitleKeep } from '#lib/composables/useKeep.svelte.js';
  import EntityHero from '../shared/EntityHero.svelte';
  import HeroActions from '../shared/HeroActions.svelte';
  import MetaLine from '../shared/MetaLine.svelte';
  import KidsAccess from '../shared/KidsAccess.svelte';
  import { getAuthState } from '#lib/stores/auth.svelte.js';

  interface Props {
    movie: Movie;
    syncing?: boolean;
    onWant: () => void;
    onUnwant: () => void;
    onRefresh: () => void;
    onDelete: () => void;
    onClose: () => void;
    onBack?: () => void;
    onEdit?: () => void;
    onCastClick?: (personId: string, personName: string) => void;
    onGenreClick?: (genre: string) => void;
    onMoveFile?: (fileId: string) => void;
    onReanalyze?: (force?: boolean) => void;
    onWatchlistToggle?: (inWatchlist: boolean) => void;
  }

  let {
    movie,
    syncing = false,
    onWant,
    onUnwant,
    onRefresh,
    onDelete,
    onClose,
    onBack,
    onEdit,
    onCastClick,
    onGenreClick,
    onMoveFile,
    onReanalyze,
    onWatchlistToggle
  }: Props = $props();

  const auth = getAuthState();

  let files = $state<MovieFile[]>([]);
  let watchState = $state<MovieWatchState | null>(null);
  let loadingWatch = $state(false);
  let deletingFileId = $state<string | null>(null);
  let rejecting = $state(false);
  let showReleaseSearch = $state(false);
  let subtitleTracks = $state<SubtitleTrackInfo[]>([]);
  let externalSubtitles = $state<ExternalSubtitleInfo[]>([]);
  let showSubtitleSearch = $state(false);

  const statsState = getStatsState();
  const stats = $derived(statsState.stats);

  const playback = usePlayback({
    mediaType: 'movie',
    refreshWatchState: async () => {
      watchState = await getMovieWatchState(movie.id);
    }
  });

  const stream = useStreamPlayback({
    mediaType: 'movie',
    getMediaId: () => movie.id,
    getSearchParams: () => ({ movie_id: movie.id }),
    getResumePosition: () => watchState?.position_seconds ?? undefined,
    getEstimatedDuration: () => (movie.runtime ? movie.runtime * 60 : undefined),
    playback,
    onMessage: showMessage,
    want: async () => onWant(),
    canWant: () => !isActivelyWanted(movie.want_status)
  });

  const titleKeep = useTitleKeep(showMessage);

  const runtime = $derived.by(() => {
    if (!movie.runtime) return null;
    const hours = Math.floor(movie.runtime / 60);
    const mins = movie.runtime % 60;
    if (hours === 0) return `${mins}m`;
    return `${hours}h ${mins}m`;
  });

  const director = $derived.by(() => {
    if (!movie.crew) return null;
    const d = movie.crew.find((c) => c.job === 'Director');
    return d?.person_name ?? null;
  });

  async function loadTracks() {
    try {
      const tracks = await getMovieTracks(movie.id);
      subtitleTracks = tracks.files.flatMap((f) => f.subtitle_tracks);
      externalSubtitles = tracks.files.flatMap((f) => f.external_subtitles);
    } catch {
      // Non-critical
    }
  }

  async function loadDetails() {
    loadingWatch = true;
    try {
      const [f, ws] = await Promise.all([getMovieFiles(movie.id), getMovieWatchState(movie.id)]);
      files = f;
      watchState = ws;
      if (f.length === 0) void titleKeep.refresh(movie.id);
    } catch (e) {
      console.error('Failed to load movie details:', e);
    } finally {
      loadingWatch = false;
    }
    loadTracks();
  }

  /** Pre-warm the right thing: playback session for owned content, stream search for unowned. */
  function preWarm() {
    if (movie.is_in_library && files.length > 0) {
      // Must match the resume logic in the Play button — if watched, start from beginning
      const resumePos = watchState?.watched
        ? undefined
        : (watchState?.position_seconds ?? undefined);
      playback.preWarm(files[0].id, resumePos);
      return;
    }
    stream.preWarmStream();
  }

  onMount(() => {
    loadDetails().then(() => {
      preWarm();
    });
    return () => {
      playback.teardownPreWarm();
      titleKeep.dispose();
    };
  });

  let prevMovieId: string | undefined;
  $effect(() => {
    const id = movie.id;
    if (prevMovieId !== undefined && prevMovieId !== id) {
      loadTracks();
      stream.reset();
      playback.teardownPreWarm();
      loadDetails().then(() => preWarm());
    }
    prevMovieId = id;
  });

  function closePlayer() {
    const streamed = playback.playingFileId?.startsWith('debrid-');
    stream.cancel();
    if (streamed) void titleKeep.refresh(movie.id);
  }

  async function handleRatingChange(rating: number | null) {
    try {
      const result = await updateMovieWatchState(movie.id, { rating });
      watchState = {
        id: result.id,
        movie_id: movie.id,
        user_id: '',
        watched: watchState?.watched ?? false,
        watched_at: watchState?.watched_at ?? null,
        position_seconds: watchState?.position_seconds ?? null,
        rating,
        created_at: watchState?.created_at ?? new Date().toISOString(),
        updated_at: new Date().toISOString()
      };
    } catch {
      showMessage('Failed to update rating');
    }
  }

  async function toggleWatched() {
    loadingWatch = true;
    try {
      const newWatched = !watchState?.watched;
      const result = await updateMovieWatchState(movie.id, {
        watched: newWatched,
        watched_at: newWatched ? new Date().toISOString() : null
      });
      watchState = {
        id: result.id,
        movie_id: movie.id,
        user_id: '',
        watched: newWatched,
        watched_at: newWatched ? new Date().toISOString() : null,
        position_seconds: watchState?.position_seconds ?? null,
        rating: watchState?.rating ?? null,
        created_at: watchState?.created_at ?? new Date().toISOString(),
        updated_at: new Date().toISOString()
      };
      showMessage(newWatched ? 'Marked as watched' : 'Marked as unwatched');
    } catch {
      showMessage('Failed to update watch state');
    } finally {
      loadingWatch = false;
    }
  }

  async function handleDeleteFile(fileId: string) {
    if (!confirm('Delete this file? This cannot be undone.')) return;
    deletingFileId = fileId;
    try {
      await deleteMovieFile(movie.id, fileId);
      files = files.filter((f) => f.id !== fileId);
      showMessage('File deleted');
      onRefresh();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Delete failed');
    } finally {
      deletingFileId = null;
    }
  }

  async function handleReplace(fileId: string) {
    if (!confirm('Replace this file? The current file will be deleted and a new search started.'))
      return;
    try {
      await replaceFile('movie', movie.id, fileId);
      showMessage('Replacement search started');
      await loadDetails();
      onRefresh();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Replace failed');
    }
  }

  async function handleRejectAndResearch() {
    rejecting = true;
    try {
      await rejectDownload('movie', movie.id);
      showMessage('Download rejected — searching for new release');
      showReleaseSearch = true;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Reject failed');
    } finally {
      rejecting = false;
    }
  }
</script>

<ContentPageView
  noHeader={true}
  noPadding={true}
  pageBackdropUrl={movie.backdrop_url || movie.poster_url}
  {onBack}
  {onClose}
  {onRefresh}
  {onEdit}
  {syncing}
>
  {#snippet headerActions()}
    <button
      type="button"
      class="glass-btn"
      class:syncing
      onclick={onRefresh}
      title={syncing ? 'Syncing metadata...' : 'Refresh'}
    >
      <span class="material-icons-outlined" class:spin={syncing}>refresh</span>
    </button>
    {#if onReanalyze}
      <button
        type="button"
        class="glass-btn"
        onclick={() => onReanalyze?.(false)}
        title="Re-analyze Media"
      >
        <span class="material-icons-outlined">science</span>
      </button>
      <button
        type="button"
        class="glass-btn"
        onclick={() => onReanalyze?.(true)}
        title="Force Re-analyze (rebuild cache from scratch, ignore manifest)"
      >
        <span class="material-icons-outlined">autorenew</span>
      </button>
    {/if}
    {#if onEdit}
      <button type="button" class="glass-btn" onclick={onEdit} title="Edit">
        <span class="material-icons-outlined">edit</span>
      </button>
    {/if}
    <button type="button" class="glass-btn" onclick={onClose} title="Close">
      <span class="material-icons-outlined">close</span>
    </button>
  {/snippet}
  <EntityHero
    title={movie.title}
    coverUrl={movie.poster_url}
    backdropUrl={movie.backdrop_url}
    logoUrl={movie.logo_url}
    description={movie.description}
  >
    {#snippet ratingContent()}
      <DualRatingStars
        communityRating={movie.rating}
        communityScale={VIDEO_RATING_SCALE}
        communityRatingCount={movie.rating_count ?? null}
        userRating={watchState?.rating ?? null}
        size="small"
        editable={true}
        onRatingChange={handleRatingChange}
      />
    {/snippet}

    {#snippet actions()}
      <HeroActions
        actions={[
          {
            label: watchState?.watched ? 'Watched' : 'Not Watched',
            icon: watchState?.watched ? 'visibility' : 'visibility_off',
            active: watchState?.watched ?? false,
            busy: loadingWatch,
            pinned: true,
            onSelect: toggleWatched
          },
          onWatchlistToggle && {
            label: movie.in_watchlist ? 'On My List' : 'Want to Watch',
            icon: movie.in_watchlist ? 'playlist_add_check' : 'playlist_add',
            title: movie.in_watchlist
              ? 'Remove from your watch list'
              : 'Add to your watch list — shows up in Up Next',
            active: movie.in_watchlist,
            pinned: true,
            onSelect: () => onWatchlistToggle(!movie.in_watchlist)
          },
          files.length === 0 && titleKeep.action,
          isActivelyWanted(movie.want_status)
            ? { label: 'Remove from Wanted', icon: 'close', onSelect: onUnwant }
            : !movie.is_in_library && {
                label: 'Want',
                icon: 'bookmark_add',
                tone: 'primary',
                onSelect: onWant
              },
          { label: 'Delete', icon: 'delete', tone: 'danger', iconOnly: true, onSelect: onDelete }
        ]}
      >
        {#snippet primary()}
          {#if files.length > 0}
            <PlaybackArea
              positionSeconds={watchState?.position_seconds ?? null}
              totalSeconds={movie.runtime ? movie.runtime * 60 : null}
              watched={watchState?.watched ?? false}
              loading={playback.startingFileId !== null}
              variant="hero"
              onPlay={() =>
                playback.play(
                  files[0].id,
                  undefined,
                  watchState?.watched ? undefined : (watchState?.position_seconds ?? undefined)
                )}
            />
          {:else if stats.kobload_configured}
            <PlaybackArea
              mode="stream"
              positionSeconds={watchState?.position_seconds ?? null}
              totalSeconds={movie.runtime ? movie.runtime * 60 : null}
              watched={watchState?.watched ?? false}
              loading={stream.busy}
              variant="hero"
              onPlay={stream.autoPlay}
              onPickStream={stream.openModal}
            />
          {/if}
        {/snippet}
      </HeroActions>
    {/snippet}

    {#snippet meta()}
      <MetaLine>
        {#if movie.year}<span>{movie.year}</span>{/if}
        {#if runtime}<span>{runtime}</span>{/if}
        {#if movie.content_rating}<span class="content-rating">{movie.content_rating}</span>{/if}
        {#if director}<span>Directed by <strong>{director}</strong></span>{/if}
      </MetaLine>
      {#if movie.genres && movie.genres.length > 0}
        <div class="hero-genres">
          <GenreChips
            genres={movie.genres}
            size="small"
            clickable={!!onGenreClick}
            {onGenreClick}
          />
        </div>
      {/if}
      {#if auth.isAdmin}
        <div class="hero-kids">
          <KidsAccess
            kind="movie"
            id={movie.id}
            override={movie.kids_override}
            allowed={movie.kids_allowed}
          />
        </div>
      {/if}
    {/snippet}
  </EntityHero>

  <div class="full-page-main-grid page-container">
    <div class="details-column">
      <CastSection
        members={(movie.cast ?? []).map((m) => ({
          key: m.id,
          name: m.person_name,
          role: m.character_name,
          imageUrl: m.person_image_url,
          onSelect: onCastClick ? () => onCastClick(m.person_id, m.person_name) : undefined
        }))}
      />
    </div>

    <div class="actions-column side-first">
      {#if files.length === 0 && !movie.is_in_library}
        <ReleaseAvailability
          releaseDate={movie.release_date ?? null}
          releaseDates={movie.release_dates ?? null}
          year={movie.year}
          streaming={streamingAvailability(
            !!stats.kobload_configured,
            stream.streamLoading,
            stream.searched ? stream.streamOptions : null
          )}
          library={libraryStatus(movie.want_status, movie.is_in_library)}
        />
      {/if}

      <!-- Files -->
      {#if files.length > 0}
        <div class="section card-section">
          <div class="label">Files</div>
          <FilesList
            {files}
            noun="movie"
            {deletingFileId}
            startingPlaybackFileId={playback.startingFileId}
            onPlay={(fileId) => playback.play(fileId)}
            onDelete={handleDeleteFile}
            onMove={onMoveFile}
            onReplace={handleReplace}
            onSearchReleases={stats.kobload_configured
              ? () => (showReleaseSearch = true)
              : undefined}
            onRejectAndResearch={handleRejectAndResearch}
            {rejecting}
          />
        </div>
      {/if}

      {#if files.length > 0}
        <div class="section card-section">
          <SubtitlesList
            {subtitleTracks}
            {externalSubtitles}
            onSearchSubtitles={() => (showSubtitleSearch = true)}
          />
        </div>
      {/if}
    </div>
  </div>
</ContentPageView>

{#if playback.isActive}
  <VideoPlayer
    sessionResponse={playback.session}
    creationParams={playback.session
      ? {
          media_type: 'movie',
          file_id: playback.playingFileId!,
          capabilities: playback.capabilities!
        }
      : undefined}
    mediaId={movie.id}
    mediaInfo={{
      title: movie.title,
      artworkUrl: movie.poster_url ?? undefined,
      backdropUrl: movie.backdrop_url ?? undefined
    }}
    initialPosition={watchState?.watched ? undefined : (watchState?.position_seconds ?? undefined)}
    onClose={closePlayer}
    {...stream.playerProps}
  />
{/if}

{#if showReleaseSearch}
  <ReleaseSearchModal
    entityTitle={movie.title}
    initialQuery={movie.title}
    onClose={() => (showReleaseSearch = false)}
    searchFn={(query) => searchMovieReleases(movie.id, query)}
    smartSearchFn={() => searchMovieReleases(movie.id)}
    queueFn={async (release, source) => {
      await queueMovieRelease(movie.id, release, source);
      await loadDetails();
      onRefresh();
    }}
    wantFn={async () => {
      await wantMovie(movie.id);
      await loadDetails();
      onRefresh();
    }}
  />
{/if}

{#if showSubtitleSearch && files.length > 0}
  <SubtitleSearchModal
    mediaType="movie"
    entityId={movie.id}
    fileId={files[0].id}
    onClose={() => (showSubtitleSearch = false)}
    onDownloaded={() => {
      showSubtitleSearch = false;
      loadTracks();
    }}
  />
{/if}

<StreamPlaybackOverlays {stream} />

<style>
  .card-section {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 24px;
  }

  .content-rating {
    padding: 2px 8px;
    border: 1px solid var(--muted);
    border-radius: 6px;
    font-weight: 700;
    font-size: 12px;
    text-transform: uppercase;
  }

  .hero-genres {
    margin-top: 6px;
  }

  .hero-kids {
    margin-top: 10px;
  }

  .label {
    font-size: 11px;
    font-weight: 800;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.1em;
    margin-bottom: 12px;
  }

  @media (max-width: 768px) {
    .hero-kids :global(.kids-access) {
      justify-content: center;
    }
  }
</style>
