<script lang="ts">
  import { onMount } from 'svelte';
  import type {
    Movie,
    MovieFile,
    MovieWatchState,
    SubtitleTrackInfo,
    ExternalSubtitleInfo
  } from '$lib/types';
  import {
    getMovieFiles,
    getMovieWatchState,
    updateMovieWatchState,
    deleteMovieFile,
    rejectDownload,
    getMovieTracks
  } from '$lib/api/client';
  import StreamPlaybackOverlays from '$lib/components/StreamPlaybackOverlays.svelte';
  import { isActivelyWanted } from '$lib/utils/want-status';
  import { showMessage } from '$lib/context/app.svelte';
  import { usePlayback } from '$lib/composables/usePlayback.svelte';
  import { useStreamPlayback } from '$lib/composables/useStreamPlayback.svelte';
  import PersonCard from '$lib/components/shared/PersonCard.svelte';
  import GenreChips from '$lib/components/GenreChips.svelte';
  import DualRatingStars from '$lib/components/DualRatingStars.svelte';
  import EntityHero from '../shared/EntityHero.svelte';
  import PlaybackArea from '$lib/components/shared/PlaybackArea.svelte';
  import FilesList from '$lib/components/shared/FilesList.svelte';
  import VideoPlayer from '$lib/components/VideoPlayer/VideoPlayer.svelte';
  import SubtitlesList from '$lib/components/shared/SubtitlesList.svelte';
  import SubtitleSearchModal from '$lib/components/shared/SubtitleSearchModal.svelte';
  import { getStatsState } from '$lib/stores/stats.svelte';

  interface Props {
    movie: Movie;
    onWant: () => void;
    onUnwant: () => void;
    onSearchReleases?: () => void;
    onCastClick?: (personId: string, personName: string) => void;
    onGenreClick?: (genre: string) => void;
    onExpand?: () => void;
    onMoveFile?: (fileId: string) => void;
    onReplace?: ((fileId: string) => void) | undefined;
  }

  let {
    movie,
    onWant,
    onUnwant,
    onSearchReleases,
    onCastClick,
    onGenreClick,
    onExpand: _onExpand,
    onMoveFile,
    onReplace
  }: Props = $props();

  const statsState = getStatsState();
  const stats = $derived(statsState.stats);

  let files = $state<MovieFile[]>([]);
  let watchState = $state<MovieWatchState | null>(null);
  let loadingWatch = $state(false);
  let descriptionExpanded = $state(false);
  let deletingFileId = $state<string | null>(null);
  let rejecting = $state(false);
  let subtitleTracks = $state<SubtitleTrackInfo[]>([]);
  let externalSubtitles = $state<ExternalSubtitleInfo[]>([]);
  let showSubtitleSearch = $state(false);

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

  const descriptionNeedsExpand = $derived(!!movie.description && movie.description.length > 200);

  async function loadDetails() {
    loadingWatch = true;
    try {
      const [f, ws] = await Promise.all([getMovieFiles(movie.id), getMovieWatchState(movie.id)]);
      files = f;
      watchState = ws;
      loadTracks();
    } catch (e) {
      console.error('Failed to load movie details:', e);
    } finally {
      loadingWatch = false;
    }
  }

  async function loadTracks() {
    try {
      const tracks = await getMovieTracks(movie.id);
      subtitleTracks = tracks.files.flatMap((f) => f.subtitle_tracks);
      externalSubtitles = tracks.files.flatMap((f) => f.external_subtitles);
    } catch {
      // Non-critical
    }
  }

  /** Pre-warm the right thing: playback session for owned content, stream search for unowned. */
  async function preWarm() {
    if (movie.is_in_library) {
      const f = await getMovieFiles(movie.id);
      if (f.length > 0) {
        playback.preWarm(f[0].id, watchState?.position_seconds ?? undefined);
        return;
      }
    }
    stream.preWarmStream();
  }

  onMount(() => {
    loadDetails().then(() => preWarm());
    return () => playback.teardownPreWarm();
  });

  // Re-load when movie changes (but not on initial mount — onMount handles that)
  let prevMovieId: string | undefined;
  $effect(() => {
    const id = movie.id;
    if (prevMovieId !== undefined && prevMovieId !== id) {
      stream.reset();
      playback.teardownPreWarm();
      loadDetails().then(() => preWarm());
    }
    prevMovieId = id;
  });

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
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Delete failed');
    } finally {
      deletingFileId = null;
    }
  }

  async function handleRejectAndResearch() {
    rejecting = true;
    try {
      await rejectDownload('movie', movie.id);
      showMessage('Download rejected — searching for new release');
      onSearchReleases?.();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Reject failed');
    } finally {
      rejecting = false;
    }
  }
</script>

<div class="movie-detail">
  <!-- Unified Hero Section -->
  <EntityHero
    title={movie.title}
    coverUrl={movie.poster_url}
    backdropUrl={movie.backdrop_url}
    logoUrl={movie.logo_url}
    embedded={true}
  >
    {#snippet ratingContent()}
      <DualRatingStars
        communityRating={movie.rating}
        communityRatingCount={movie.rating_count ?? null}
        userRating={watchState?.rating ?? null}
        size="small"
        editable={true}
        onRatingChange={handleRatingChange}
      />
    {/snippet}

    {#snippet subtitle()}
      <div class="panel-meta-row">
        {#if movie.year}
          <span class="panel-meta-item">{movie.year}</span>
        {/if}
        {#if runtime}
          <span class="panel-meta-item">{runtime}</span>
        {/if}
        {#if movie.content_rating}
          <span class="meta-item rating-badge">{movie.content_rating}</span>
        {/if}
      </div>
      {#if director}
        <div class="director">Directed by <strong>{director}</strong></div>
      {/if}
      {#if movie.genres?.length > 0}
        <div class="hero-genres">
          <GenreChips
            genres={movie.genres}
            size="small"
            clickable={!!onGenreClick}
            {onGenreClick}
          />
        </div>
      {/if}
    {/snippet}

    {#snippet actions()}
      <button
        class="panel-pill"
        class:active={watchState?.watched}
        type="button"
        onclick={toggleWatched}
        disabled={loadingWatch}
      >
        <span class="material-icons-outlined">
          {watchState?.watched ? 'visibility' : 'visibility_off'}
        </span>
        {watchState?.watched ? 'Watched' : 'Unwatched'}
      </button>
      {#if isActivelyWanted(movie.want_status)}
        <button class="panel-pill active" type="button" onclick={onUnwant}>
          <span class="material-icons-outlined">bookmark</span>
          Wanted
        </button>
      {:else if !movie.is_in_library}
        <button class="panel-pill" type="button" onclick={onWant}>
          <span class="material-icons-outlined">bookmark_add</span>
          Want
        </button>
      {/if}
    {/snippet}
  </EntityHero>

  <!-- Playback -->
  {#if files.length > 0}
    <div class="panel-section">
      <PlaybackArea
        positionSeconds={watchState?.position_seconds ?? null}
        totalSeconds={movie.runtime ? movie.runtime * 60 : null}
        watched={watchState?.watched ?? false}
        loading={playback.startingFileId !== null}
        variant="panel"
        onPlay={() => {
          if (files.length > 0)
            playback.play(
              files[0].id,
              undefined,
              watchState?.watched ? undefined : (watchState?.position_seconds ?? undefined)
            );
        }}
      />
    </div>
  {:else if stats.kobload_configured}
    <div class="panel-section">
      <PlaybackArea
        mode="stream"
        positionSeconds={watchState?.position_seconds ?? null}
        totalSeconds={movie.runtime ? movie.runtime * 60 : null}
        watched={watchState?.watched ?? false}
        loading={stream.busy}
        variant="panel"
        onPlay={stream.autoPlay}
        onPickStream={stream.openModal}
      />
    </div>
  {/if}

  <!-- Overview -->
  {#if movie.description}
    <div class="panel-section">
      <h4 class="panel-section-title">Overview</h4>
      <p class="panel-overview" class:collapsed={!descriptionExpanded && descriptionNeedsExpand}>
        {movie.description}
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

  <!-- Cast -->
  {#if movie.cast && movie.cast.length > 0}
    <div class="panel-section">
      <h4 class="panel-section-title">Cast</h4>
      <div class="panel-cast-list">
        {#each movie.cast.slice(0, 10) as member (member.id)}
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
        {#if movie.cast.length > 10}
          <div class="panel-more-count">+{movie.cast.length - 10} more</div>
        {/if}
      </div>
    </div>
  {/if}

  <!-- Files -->
  <div class="panel-section">
    <h4 class="panel-section-title">File</h4>
    {#if files.length > 0}
      <FilesList
        {files}
        noun="movie"
        {deletingFileId}
        startingPlaybackFileId={playback.startingFileId}
        onPlay={(fileId) => playback.play(fileId)}
        onDelete={handleDeleteFile}
        onMove={onMoveFile}
        {onReplace}
        onSearchReleases={onSearchReleases ? () => onSearchReleases?.() : undefined}
        onRejectAndResearch={handleRejectAndResearch}
        {rejecting}
      />
    {:else}
      <div class="panel-file-empty">
        <span class="muted">No file</span>
        {#if onSearchReleases}
          <button
            class="panel-icon-btn"
            type="button"
            title="Search for releases"
            onclick={onSearchReleases}
          >
            <span class="material-icons-outlined">search</span>
          </button>
        {/if}
      </div>
    {/if}
  </div>

  <!-- Subtitles -->
  {#if files.length > 0}
    <div class="panel-section">
      <SubtitlesList
        {subtitleTracks}
        {externalSubtitles}
        onSearchSubtitles={() => (showSubtitleSearch = true)}
      />
    </div>
  {/if}
</div>

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
    onClose={stream.cancel}
    {...stream.playerProps}
  />
{/if}

<style>
  .movie-detail {
    display: flex;
    flex-direction: column;
    gap: 0;
  }

  .rating-badge {
    padding: 0 5px;
    border: 1px solid var(--muted);
    border-radius: 3px;
    font-weight: 600;
    font-size: 10px;
  }

  .director {
    font-size: 12px;
    color: var(--muted);
    margin-top: 4px;
  }

  .director strong {
    color: var(--ink);
  }

  .hero-genres {
    margin-top: 6px;
  }
</style>
