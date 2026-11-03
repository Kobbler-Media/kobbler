<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import MovieDetailPage from '#lib/components/movies/MovieDetailPage.svelte';
  import MovieForm from '#lib/components/movies/MovieForm.svelte';
  import type { MovieFormData } from '#lib/components/movies/MovieForm.svelte';
  import VideoMetadataSearchModal from '#lib/components/video/VideoMetadataSearchModal.svelte';
  import MoveFileModal from '#lib/components/shared/MoveFileModal.svelte';
  import type { SearchSelectItem } from '@koblibs/ui';
  import ContentPageView from '#lib/components/ContentPageView.svelte';
  import { showMessage } from '#lib/context/app.svelte.js';
  import * as api from '#lib/api/client.js';
  import { on, off } from '#lib/services/eventstream.svelte.js';
  import { getJobsState } from '#lib/stores/jobs.svelte.js';
  import { watchStateVersion } from '#lib/stores/watchState.svelte.js';
  import type { Movie, MovieSearchResult, TvSearchResult } from '#lib/types.js';

  let movie = $state<Movie | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let editing = $state(false);
  let saving = $state(false);
  let deleting = $state(false);
  let showMetadataSearch = $state(false);
  let editSearchResults = $state<(MovieSearchResult | TvSearchResult)[]>([]);
  let triggerSave = $state<(() => void) | undefined>(undefined);
  let formValid = $state(false);
  let movingFileId = $state<string | null>(null);

  const handleReanalyze = async (force = false) => {
    if (!movie) return;
    try {
      const result = await api.runMediaAnalyzeMovie(movie.id, force);
      if (result.enqueued > 0) {
        const label = force ? 'Queued force re-analyze' : 'Queued re-analyze';
        showMessage(`${label} for ${result.enqueued} file${result.enqueued === 1 ? '' : 's'}`);
      } else {
        showMessage('No files to analyze');
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to run media analysis');
    }
  };

  const movieId = $derived(page.params.id);
  const pageState = $derived(page.state as { editing?: boolean });
  const jobsState = getJobsState();
  const syncing = $derived(
    jobsState.jobs.some(
      (j) =>
        j.entity_type === 'movie' &&
        j.entity_id === movieId &&
        j.job_type === 'movie_enrich' &&
        (j.status === 'pending' || j.status === 'running')
    )
  );

  const loadMovie = async (id: string) => {
    loading = true;
    error = null;
    try {
      movie = await api.getMovie(id);
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load movie';
      showMessage(error);
    } finally {
      loading = false;
    }
  };

  let lastWatchVersion = -1;
  $effect(() => {
    const v = watchStateVersion();
    if (lastWatchVersion !== -1 && v !== lastWatchVersion && movie) loadMovie(movie.id);
    lastWatchVersion = v;
  });

  const handleWatchlistToggle = async (inWatchlist: boolean) => {
    if (!movie) return;
    try {
      if (inWatchlist) {
        await api.addToWatchlist('movie', movie.id);
        showMessage('Added to your watch list');
      } else {
        await api.removeFromWatchlist('movie', movie.id);
        showMessage('Removed from your watch list');
      }
      await loadMovie(movie.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update your watch list');
    }
  };

  const handleWant = async () => {
    if (!movie) return;
    try {
      await api.wantMovie(movie.id);
      showMessage('Movie added to wanted');
      await loadMovie(movie.id);
      // Check if streaming is available
      api
        .streamSearch({ movie_id: movie.id })
        .then((result) => {
          if (result.options.length > 0) {
            showMessage('This is also available for streaming!');
          }
        })
        .catch(() => {
          /* non-critical */
        });
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want movie');
    }
  };

  const handleUnwant = async () => {
    if (!movie) return;
    try {
      await api.unwantMovie(movie.id);
      showMessage('Movie removed from wanted');
      await loadMovie(movie.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant movie');
    }
  };

  const handleRefresh = async () => {
    if (!movie) return;
    try {
      await api.refreshMovieMetadata(movie.id);
      showMessage('Metadata refresh queued');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to refresh metadata');
    }
  };

  const handleDelete = async () => {
    if (!movie) return;
    if (!confirm(`Delete "${movie.title}"? This cannot be undone.`)) return;
    try {
      await api.deleteMovie(movie.id);
      showMessage('Movie deleted');
      goto('/movies');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete movie');
    }
  };

  const handleEdit = () => {
    editing = true;
    editSearchResults = [];
  };

  const handleEditCancel = () => {
    editing = false;
    editSearchResults = [];
  };

  const handleClose = () => {
    goto('/movies');
  };

  const handleSave = async (
    data: MovieFormData,
    posterFile?: File,
    backdropFile?: File,
    logoFile?: File
  ) => {
    if (!movie) return;
    saving = true;
    try {
      const { cast, crew, ...metadata } = data;
      await api.updateMovie(movie.id, metadata as unknown as Record<string, unknown>);
      if (cast) await api.replaceMovieCast(movie.id, cast);
      if (crew) await api.replaceMovieCrew(movie.id, crew);
      if (posterFile) {
        await api.uploadMoviePoster(movie.id, posterFile);
      }
      if (backdropFile) {
        await api.uploadMovieBackdrop(movie.id, backdropFile);
      }
      if (logoFile) {
        await api.uploadMovieLogo(movie.id, logoFile);
      }
      showMessage('Movie updated');
      editing = false;
      editSearchResults = [];
      await loadMovie(movie.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update movie');
    } finally {
      saving = false;
    }
  };

  const handleMetadataSearchSelect = (results: (MovieSearchResult | TvSearchResult)[]) => {
    editSearchResults = results;
    showMetadataSearch = false;
  };

  const handleEditDelete = async () => {
    if (!movie) return;
    if (!confirm(`Delete "${movie.title}"? This cannot be undone.`)) return;
    deleting = true;
    try {
      await api.deleteMovie(movie.id);
      showMessage('Movie deleted');
      goto('/movies');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete movie');
    } finally {
      deleting = false;
    }
  };

  const handleMoveFile = (fileId: string) => {
    movingFileId = fileId;
  };

  const doMoveFile = async (targetMovieId: string) => {
    if (!movie || !movingFileId) return;
    await api.moveMovieFile(movie.id, movingFileId, targetMovieId);
    await loadMovie(movie.id);
  };

  const searchMoviesForMove = async (query: string): Promise<SearchSelectItem[]> => {
    const resp = await api.getMovies({ search: query, limit: 20 });
    return resp.items
      .filter((m) => m.id !== movie?.id)
      .map((m) => ({
        value: m.id,
        label: m.year ? `${m.title} (${m.year})` : m.title
      }));
  };

  const handleCastClick = (personId: string, _personName: string) => {
    goto(`/people/${personId}`);
  };

  // SSE: auto-refresh when enrichment completes
  const movieUpdatedCb = on('movie_updated', (data: { movie_id: string }) => {
    if (data.movie_id === movieId) {
      loadMovie(movieId);
    }
  });

  onMount(() => {
    if (pageState?.editing) {
      editing = true;
    }
    if (movieId) {
      loadMovie(movieId);
    }
    return () => {
      off('movie_updated', movieUpdatedCb);
    };
  });

  $effect(() => {
    if (movieId && untrack(() => !movie || movie.id !== movieId)) {
      loadMovie(movieId);
    }
  });
</script>

<svelte:head>
  <title>{movie?.title ?? 'Movie'} — Kobbler</title>
</svelte:head>

{#if loading && !movie}
  <div class="loading-state page-container">
    <p>Loading movie...</p>
  </div>
{:else if error && !movie}
  <div class="error-state page-container">
    <p>{error}</p>
    <button type="button" class="btn-back" onclick={handleClose}>Back to Movies</button>
  </div>
{:else if movie && editing}
  <ContentPageView
    noHeader={true}
    noPadding={true}
    pageBackdropUrl={movie.backdrop_url || movie.poster_url}
    onClose={handleClose}
  >
    {#snippet headerActions()}
      <button
        type="button"
        class="glass-btn"
        onclick={() => (showMetadataSearch = true)}
        title="Search Metadata"
      >
        <span class="material-icons-outlined">search</span>
      </button>
      <button
        type="button"
        class="glass-btn active"
        onclick={handleEditCancel}
        title="Editing (click to cancel)"
      >
        <span class="material-icons-outlined">edit</span>
      </button>
      <button type="button" class="glass-btn" onclick={handleClose} title="Close">
        <span class="material-icons-outlined">close</span>
      </button>
    {/snippet}

    <div class="edit-hero">
      <div class="edit-hero-content">
        <h2 class="edit-hero-title">Editing: {movie.title}</h2>
      </div>
    </div>

    <div class="page-container">
      <MovieForm
        {movie}
        searchResults={editSearchResults}
        hideFooter
        bind:requestSave={triggerSave}
        bind:canSave={formValid}
        onSave={handleSave}
        onCancel={handleEditCancel}
        onDelete={handleEditDelete}
        {saving}
        {deleting}
      />
    </div>

    {#snippet footer()}
      <div class="edit-footer">
        <button
          type="button"
          class="btn btn-danger"
          onclick={handleEditDelete}
          disabled={deleting || saving}
        >
          <span class="material-icons-outlined">delete</span>
          {deleting ? 'Deleting...' : 'Delete'}
        </button>
        <div class="spacer"></div>

        <button type="button" class="btn btn-sec" onclick={handleEditCancel} disabled={saving}
          >Cancel</button
        >

        <button
          type="button"
          class="btn btn-primary"
          onclick={() => triggerSave?.()}
          disabled={!formValid || saving}
        >
          <span class="material-icons-outlined">save</span>
          {saving ? 'Saving...' : 'Save Changes'}
        </button>
      </div>
    {/snippet}
  </ContentPageView>
  {#if showMetadataSearch}
    <VideoMetadataSearchModal
      mode="movie"
      initialTitle={movie.title}
      onSelect={handleMetadataSearchSelect}
      onCancel={() => (showMetadataSearch = false)}
    />
  {/if}
{:else if movie}
  <MovieDetailPage
    {movie}
    {syncing}
    onWant={handleWant}
    onUnwant={handleUnwant}
    onWatchlistToggle={handleWatchlistToggle}
    onRefresh={handleRefresh}
    onDelete={handleDelete}
    onEdit={handleEdit}
    onClose={handleClose}
    onCastClick={handleCastClick}
    onGenreClick={(genre) => goto(`/movies?q=${encodeURIComponent(genre)}`)}
    onMoveFile={handleMoveFile}
    onReanalyze={handleReanalyze}
  />
  {#if movingFileId}
    <MoveFileModal
      title="Move File to Movie"
      searchFn={searchMoviesForMove}
      onMove={doMoveFile}
      onClose={() => (movingFileId = null)}
    />
  {/if}
{/if}

<style>
  .loading-state,
  .error-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    min-height: 200px;
    color: var(--muted);
    gap: 16px;
  }

  .error-state p {
    color: var(--error, #ef4444);
  }

  .btn-back {
    padding: 8px 16px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--card);
    color: var(--ink);
    cursor: pointer;
    font-size: 13px;
  }

  .btn-back:hover {
    background: var(--bg-2);
  }

  .edit-hero {
    position: relative;
    z-index: 1;
    height: 56px;
  }

  .edit-hero-content {
    display: flex;
    align-items: center;
    height: 100%;
    padding: 0 clamp(24px, 5vw, 80px);
    max-width: 1600px;
    margin: 0 auto;
  }

  .edit-hero-title {
    font-size: 15px;
    font-weight: 600;
    color: var(--ink);
    margin: 0;
    letter-spacing: -0.01em;
  }

  .page-container {
    position: relative;
    z-index: 1;
  }

  .page-container :global(.form-inner) {
    padding-top: 20px;
  }

  /* Footer */
  .edit-footer {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
  }

  .spacer {
    flex: 1;
  }
</style>
