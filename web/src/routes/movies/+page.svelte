<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { onReconnect, offReconnect, onDebounced, off } from '#lib/services/eventstream.svelte.js';
  import { updateUrlParams } from '#lib/utils/url-state.js';
  import {
    captureScrollAnchor,
    restoreScrollAnchor,
    scrollContainerToTop,
    scrollItemToTop
  } from '#lib/utils/scroll-anchor.js';
  import MoviesPage from '#lib/components/pages/MoviesPage.svelte';
  import type { MovieSortValue } from '#lib/components/pages/MoviesPage.svelte';
  import VideoSearchModal from '#lib/components/movies/VideoSearchModal.svelte';
  import MovieDetailSidebar from '#lib/components/movies/MovieDetailSidebar.svelte';
  import MovieForm from '#lib/components/movies/MovieForm.svelte';
  import type { MovieFormData } from '#lib/components/movies/MovieForm.svelte';
  import VideoMetadataSearchModal from '#lib/components/video/VideoMetadataSearchModal.svelte';
  import type { MovieSearchResult, TvSearchResult } from '#lib/types.js';
  import PersonDetailsPanel from '#lib/components/people/PersonDetailsPanel.svelte';
  import ReleaseSearchModal from '#lib/components/ReleaseSearchModal.svelte';
  import { ExpandableSidebar } from '@koblibs/ui';
  import ErrorBanner from '#lib/components/shared/ErrorBanner.svelte';
  import MoveFileModal from '#lib/components/shared/MoveFileModal.svelte';
  import { showMessage } from '#lib/context/app.svelte.js';
  import * as api from '#lib/api/client.js';
  import { getStatsState } from '#lib/stores/stats.svelte.js';
  import type { Movie, PersonDetail } from '#lib/types.js';
  import {
    pushEntity,
    popEntity,
    clearStack,
    getNavigationState,
    updateCurrentEntityData,
    setLoadingEntity
  } from '#lib/stores/navigation.svelte.js';

  type MovieFilter = 'all' | 'wanted' | 'in_library';

  let movies = $state<Movie[]>([]);
  let loading = $state(false);
  let error = $state('');
  let filter = $state<MovieFilter>('all');
  let sort = $state<MovieSortValue>('title');
  let searchQuery = $state('');
  let showAddModal = $state(false);
  let availableLetters = $state<string[]>([]);
  let editMode = $state(false);
  let formRequestSave: (() => void) | undefined = $state();
  let formCanSave: boolean | undefined = $state();
  let showMetadataSearch = $state(false);
  let showReleaseSearch = $state(false);
  let editSearchResults = $state<(MovieSearchResult | TvSearchResult)[]>([]);
  let nextCursor = $state<string | null>(null);
  let prevCursor = $state<string | null>(null);
  let totalCount = $state<number | null>(null);
  let movingFileId = $state<string | null>(null);
  let loadGeneration = 0;
  const urlAround = $derived(page.url.searchParams.get('around'));
  const urlAroundBefore = $derived(page.url.searchParams.get('around_before'));

  const nav = getNavigationState();
  const statsState = getStatsState();
  const stats = $derived(statsState.stats);

  let reconnectHandler: (() => void) | null = null;
  let sseHandler: ((data: unknown) => void) | null = null;

  async function loadMovies(options?: {
    cursor?: string;
    direction?: string;
    around?: string;
    around_before?: number;
    limit?: number;
    mode?: 'replace' | 'append' | 'prepend';
  }): Promise<void> {
    const gen = ++loadGeneration;
    loading = true;
    try {
      const params: Record<string, string | number | boolean | undefined> = {};
      if (searchQuery.trim()) {
        params.search = searchQuery.trim();
      }
      if (filter === 'wanted') {
        params.want_status = 'wanted';
      } else if (filter === 'in_library') {
        params.is_in_library = true;
      }
      if (sort !== 'title') params.sort = sort;
      if (options?.cursor) params.cursor = options.cursor;
      if (options?.direction) params.direction = options.direction;
      if (options?.around) params.around = options.around;
      if (options?.around_before != null) params.around_before = options.around_before;
      if (options?.limit != null) params.limit = options.limit;

      const result = await api.getMovies(params);
      if (gen !== loadGeneration) return;
      const mode = options?.mode ?? 'replace';

      if (mode === 'append') {
        const existingIds = new Set(movies.map((m) => m.id));
        const uniqueNew = result.items.filter((m) => !existingIds.has(m.id));
        movies = [...movies, ...uniqueNew];
        nextCursor = result.next_cursor;
      } else if (mode === 'prepend') {
        const existingIds = new Set(movies.map((m) => m.id));
        const uniqueNew = result.items.filter((m) => !existingIds.has(m.id));
        movies = [...uniqueNew, ...movies];
        prevCursor = result.prev_cursor ?? null;
      } else {
        movies = result.items;
        nextCursor = result.next_cursor;
        prevCursor = result.prev_cursor ?? null;
        if (result.total_count != null) totalCount = result.total_count;
      }
      error = '';
    } catch (e) {
      if (gen !== loadGeneration) return;
      if (options?.around) {
        // The anchor row can vanish between render and request (it was just deleted).
        // Retry without it rather than blanking the list.
        return await loadMovies({ ...options, around: undefined, around_before: undefined });
      }
      showMessage('Failed to load movies');
      console.error('Failed to load movies:', e);
      error = 'Failed to load data. Please check your connection and try again.';
    } finally {
      if (gen === loadGeneration) loading = false;
    }
  }

  async function loadAvailableLetters() {
    try {
      availableLetters = await api.getMovieAvailableLetters();
    } catch (e) {
      console.error('Failed to load available letters:', e);
      availableLetters = [];
    }
  }

  /**
   * Reload the current view in-place, keeping the reader where they are.
   *
   * The window has to be re-requested at its current size and centred on the first
   * *visible* card: a default-limit reload centred on movies[0] throws away every page
   * that was scrolled into view, the grid collapses to 50 cards, and the browser clamps
   * scrollTop to the new (much shorter) content.
   */
  function reloadInPlace(): Promise<void> {
    // `around` windows are always title-ordered server-side (cursor pagination only
    // exists for that sort), so anchoring under any other sort would silently re-sort
    // the page out from under the reader. Those sorts are a single page anyway.
    if (movies.length === 0 || sort !== 'title') {
      return loadMovies();
    }
    const anchor = captureScrollAnchor();
    const anchorIndex = anchor ? movies.findIndex((m) => m.id === anchor.id) : -1;
    const index = anchorIndex >= 0 ? anchorIndex : 0;
    const limit = Math.min(Math.max(movies.length, 50), 200);
    const before = Math.min(index, Math.floor(limit / 2));
    return loadMovies({ around: movies[index].id, around_before: before, limit }).then(() =>
      restoreScrollAnchor(anchor)
    );
  }

  onMount(() => {
    lastAround = urlAround;
    const initialAround = urlAround;
    if (initialAround) {
      const ab = urlAroundBefore ? parseInt(urlAroundBefore, 10) : 0;
      loadMovies({ around: initialAround, around_before: ab }).then(() => {
        if (ab > 0) scrollItemToTop(initialAround);
        else scrollContainerToTop();
        updateUrlParams({ around: null, around_before: null });
      });
    } else {
      loadMovies();
    }
    loadAvailableLetters();

    reconnectHandler = onReconnect(() => {
      reloadInPlace();
      loadAvailableLetters();
    });

    sseHandler = onDebounced('movie_updated', () => {
      reloadInPlace();
      loadAvailableLetters();
    });
  });

  onDestroy(() => {
    if (reconnectHandler) offReconnect(reconnectHandler);
    if (sseHandler) off('movie_updated', sseHandler);
  });

  // Handle around parameter changes (e.g., letter jump while already on page)
  let lastAround = $state<string | null>(null);
  $effect(() => {
    const around = urlAround;
    if (around && around !== lastAround) {
      lastAround = around;
      const ab = urlAroundBefore ? parseInt(urlAroundBefore, 10) : 0;
      loadMovies({ around, around_before: ab }).then(() => {
        // The fresh window renders underneath wherever the reader was already scrolled,
        // so without this a jump to "A" lands however many pixels down the alphabet.
        if (ab > 0) scrollItemToTop(around);
        else scrollContainerToTop();
        updateUrlParams({ around: null, around_before: null });
      });
    } else if (!around) {
      lastAround = null;
    }
  });

  function handleSelectMovie(movie: Movie) {
    if (window.matchMedia('(max-width: 768px)').matches) {
      goto(`/movies/${movie.id}`);
    } else {
      pushEntity('movie', movie.id);
    }
  }

  async function handleJumpToLetter(letter: string | null) {
    if (!letter) {
      // Back to the start of the list. A goto() here is usually a no-op (the URL is
      // already bare by the time the previous jump cleared `around`), so reload directly.
      updateUrlParams({ around: null, around_before: null });
      await loadMovies();
      scrollContainerToTop();
      return;
    }

    loading = true;
    try {
      const result = await api.getMovieFirstByLetter(letter);
      if (result) {
        goto(`/movies?around=${result.id}&around_before=0`);
      } else {
        showMessage(`No movies starting with "${letter}"`);
        loading = false;
      }
    } catch {
      showMessage(`Failed to find movies for "${letter}"`);
      loading = false;
    }
  }

  function loadMoreMovies() {
    if (!nextCursor || loading) return;
    loadMovies({ cursor: nextCursor, direction: 'forward', mode: 'append' });
  }

  function loadPreviousMovies() {
    if (!prevCursor || loading) return;
    loadMovies({ cursor: prevCursor, direction: 'backward', mode: 'prepend' });
  }

  function handleFilterChange(newFilter: MovieFilter) {
    filter = newFilter;
    nextCursor = null;
    prevCursor = null;
    totalCount = null;
    // A new filter/search is a new list: start it at the top, since the scroll
    // container keeps its offset across the swap.
    loadMovies().then(scrollContainerToTop);
  }

  function handleSortChange(newSort: MovieSortValue) {
    sort = newSort;
    nextCursor = null;
    prevCursor = null;
    totalCount = null;
    // A new sort is a new list: start it at the top, since the scroll
    // container keeps its offset across the swap.
    loadMovies().then(scrollContainerToTop);
  }

  function handleSearch(query: string) {
    searchQuery = query;
    nextCursor = null;
    prevCursor = null;
    totalCount = null;
    // A new filter/search is a new list: start it at the top, since the scroll
    // container keeps its offset across the swap.
    loadMovies().then(scrollContainerToTop);
  }

  async function handleAddComplete(id: string) {
    showAddModal = false;
    await loadMovies();
    // Open the newly added movie in sidebar
    try {
      const movie = await api.getMovie(id);
      pushEntity('movie', id, movie);
    } catch {
      // Movie was added, just refresh the list
    }
  }

  function handleCloseSidebar() {
    if (editMode) {
      editMode = false;
    } else {
      clearStack();
    }
  }

  function handleExpandMovie() {
    const entity = nav.current;
    if (entity?.type === 'movie') {
      const wasEditing = editMode;
      editMode = false;
      clearStack();
      goto(`/movies/${entity.id}`, wasEditing ? { state: { editing: true } } : undefined);
    }
  }

  function handleBack() {
    editMode = false;
    popEntity();
  }

  // Load entity data when nav stack changes
  $effect(() => {
    const entity = nav.current;
    if (!entity) return;
    if (entity.data) return;

    if (entity.type === 'movie') {
      loadNavMovieData(entity.id);
    } else if (entity.type === 'person') {
      loadNavPersonData(entity.id);
    }
  });

  async function loadNavMovieData(movieId: string) {
    setLoadingEntity(true);
    try {
      const movie = await api.getMovie(movieId);
      updateCurrentEntityData(movie);
    } catch (e) {
      console.error('Failed to load movie:', e);
    } finally {
      setLoadingEntity(false);
    }
  }

  async function loadNavPersonData(personId: string) {
    setLoadingEntity(true);
    try {
      const person = await api.getPerson(personId);
      updateCurrentEntityData(person);
    } catch (e) {
      console.error('Failed to load person:', e);
    } finally {
      setLoadingEntity(false);
    }
  }

  async function handleWant() {
    const entity = nav.current;
    if (entity?.type !== 'movie' || !entity.data) return;
    try {
      await api.wantMovie(entity.id);
      showMessage('Movie added to wanted');
      const movie = await api.getMovie(entity.id);
      updateCurrentEntityData(movie);
      await reloadInPlace();
      // Check if streaming is available
      api
        .streamSearch({ movie_id: entity.id })
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
  }

  async function handleUnwant() {
    const entity = nav.current;
    if (entity?.type !== 'movie' || !entity.data) return;
    try {
      await api.unwantMovie(entity.id);
      showMessage('Movie removed from wanted');
      const movie = await api.getMovie(entity.id);
      updateCurrentEntityData(movie);
      await reloadInPlace();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant movie');
    }
  }

  async function handleRefreshMetadata() {
    const entity = nav.current;
    if (entity?.type !== 'movie') return;
    try {
      await api.refreshMovieMetadata(entity.id);
      showMessage('Metadata refresh queued');
      setTimeout(async () => {
        const movie = await api.getMovie(entity.id);
        updateCurrentEntityData(movie);
      }, 1500);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to refresh metadata');
    }
  }

  async function handleDeleteMovie() {
    const entity = nav.current;
    if (entity?.type !== 'movie') return;
    if (!confirm('Delete this movie? This cannot be undone.')) return;
    try {
      await api.deleteMovie(entity.id);
      showMessage('Movie deleted');
      clearStack();
      await reloadInPlace();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete movie');
    }
  }

  function handleToggleEdit() {
    editMode = !editMode;
    if (!editMode) {
      editSearchResults = [];
      showMetadataSearch = false;
    }
  }

  async function handleEditSave(data: MovieFormData) {
    const entity = nav.current;
    if (entity?.type !== 'movie' || !entity.data) return;
    loading = true;
    try {
      const { cast, crew, ...metadata } = data;
      await api.updateMovie(entity.id, metadata as unknown as Record<string, unknown>);
      if (cast) await api.replaceMovieCast(entity.id, cast);
      if (crew) await api.replaceMovieCrew(entity.id, crew);
      showMessage('Movie updated');
      editMode = false;
      const movie = await api.getMovie(entity.id);
      updateCurrentEntityData(movie);
      await reloadInPlace();
    } catch {
      showMessage('Failed to update movie');
    } finally {
      loading = false;
    }
  }

  function handleEditCancel() {
    editMode = false;
    editSearchResults = [];
    showMetadataSearch = false;
  }

  function handleCastClick(personId: string, _personName: string) {
    pushEntity('person', personId);
  }

  function handleGenreClick(genre: string) {
    handleSearch(genre);
  }

  function handleSearchReleases() {
    showReleaseSearch = true;
  }

  function handlePersonEdit() {
    const entity = nav.current;
    if (entity?.type === 'person') {
      goto(`/people/${entity.id}`);
    }
  }

  async function handlePersonDelete() {
    const entity = nav.current;
    if (entity?.type !== 'person') return;
    try {
      await api.deletePerson(entity.id);
      showMessage('Person deleted');
      popEntity();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete person');
    }
  }

  async function searchMoviesForMove(query: string) {
    const result = await api.getMovies({ search: query });
    return result.items.map((m) => ({
      value: m.id,
      label: m.title + (m.year ? ` (${m.year})` : '')
    }));
  }

  async function handleReplace(fileId: string) {
    if (!confirm('Replace this file? The current file will be deleted and a new search started.'))
      return;
    const movie = nav.current?.data as Movie;
    if (!movie) return;
    try {
      await api.replaceFile('movie', movie.id, fileId);
      showMessage('Replacement search started');
      const updated = await api.getMovie(movie.id);
      updateCurrentEntityData(updated);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Replace failed');
    }
  }

  function handleMoveFile(fileId: string) {
    movingFileId = fileId;
  }

  async function doMoveFile(targetMovieId: string) {
    const movie = nav.current?.data as Movie;
    if (!movie || !movingFileId) return;
    await api.moveMovieFile(movie.id, movingFileId, targetMovieId);
    movingFileId = null;
    const updated = await api.getMovie(movie.id);
    updateCurrentEntityData(updated);
  }

  const selectedMovieId = $derived(nav.current?.type === 'movie' ? nav.current.id : null);
  const showEditButton = $derived(nav.current?.type === 'movie' && !editMode);

  const sidebarTitle = $derived.by(() => {
    const entity = nav.current;
    if (!entity) return 'Details';
    if (entity.type === 'person' && entity.data) {
      return (entity.data as PersonDetail).name;
    }
    if (entity.type === 'movie' && entity.data) {
      return (entity.data as Movie).title;
    }
    return 'Details';
  });
</script>

<svelte:head>
  <title>Movies — Kobbler</title>
</svelte:head>

{#if error}
  <div class="error-wrapper">
    <ErrorBanner message={error} onRetry={() => loadMovies()} onDismiss={() => (error = '')} />
  </div>
{/if}

<MoviesPage
  {movies}
  {loading}
  {filter}
  {sort}
  {searchQuery}
  {selectedMovieId}
  {availableLetters}
  {totalCount}
  hasMore={nextCursor !== null}
  hasPrevious={prevCursor !== null}
  onFilterChange={handleFilterChange}
  onSortChange={handleSortChange}
  onSearch={handleSearch}
  onSelectMovie={handleSelectMovie}
  onJumpToLetter={handleJumpToLetter}
  onLoadMore={loadMoreMovies}
  onLoadPrevious={loadPreviousMovies}
  onAdd={() => (showAddModal = true)}
/>

<ExpandableSidebar
  open={nav.isOpen}
  expanded={editMode}
  title={sidebarTitle}
  showEdit={showEditButton}
  showRefresh={nav.current?.type === 'movie' && !editMode}
  {editMode}
  onClose={handleCloseSidebar}
  onExpand={nav.current?.type === 'movie' ? handleExpandMovie : undefined}
  onToggleEdit={nav.current?.type === 'movie' ? handleToggleEdit : undefined}
  onRefresh={handleRefreshMetadata}
  onSearch={editMode && nav.current?.type === 'movie'
    ? () => (showMetadataSearch = true)
    : undefined}
>
  {#snippet headerLeft()}
    {#if nav.canGoBack && !editMode}
      <button class="icon-btn small" type="button" title="Back" onclick={handleBack}>
        <span class="material-icons-outlined">arrow_back</span>
      </button>
    {/if}
  {/snippet}
  {#snippet footer()}
    {#if editMode && nav.current?.type === 'movie' && nav.current.data}
      <div class="sidebar-footer-actions">
        <button class="btn-danger-sm" type="button" onclick={handleDeleteMovie} disabled={loading}>
          <span class="material-icons-outlined">delete</span>
          Delete
        </button>
        <div class="spacer"></div>

        <button class="btn-sec" type="button" onclick={handleEditCancel} disabled={loading}
          >Cancel</button
        >

        <button
          class="btn-primary"
          type="button"
          onclick={() => formRequestSave?.()}
          disabled={!formCanSave || loading}
        >
          {loading ? 'Saving...' : 'Save Changes'}
        </button>
      </div>
    {/if}
  {/snippet}
  {#if nav.current?.type === 'movie' && nav.current.data}
    {#if editMode}
      {#key nav.current.id}
        <MovieForm
          movie={nav.current.data as Movie}
          searchResults={editSearchResults}
          onSave={handleEditSave}
          onCancel={handleEditCancel}
          saving={loading}
          hideFooter
          bind:requestSave={formRequestSave}
          bind:canSave={formCanSave}
        />
      {/key}
    {:else}
      <MovieDetailSidebar
        movie={nav.current.data as Movie}
        onWant={handleWant}
        onUnwant={handleUnwant}
        onSearchReleases={stats.kobload_configured ? handleSearchReleases : undefined}
        onCastClick={handleCastClick}
        onGenreClick={handleGenreClick}
        onExpand={handleExpandMovie}
        onMoveFile={handleMoveFile}
        onReplace={handleReplace}
      />
    {/if}
  {:else if nav.current?.type === 'person' && nav.current.data}
    <PersonDetailsPanel
      person={nav.current.data as PersonDetail}
      onEdit={handlePersonEdit}
      onDelete={handlePersonDelete}
      onViewFullPage={() => goto(`/people/${(nav.current!.data as PersonDetail).id}`)}
      onCreditClick={(libraryId, mediaType) => {
        if (mediaType === 'movie') pushEntity('movie', libraryId);
        else goto(`/tv/${libraryId}`);
      }}
    />
  {:else if nav.isOpen}
    <div class="loading-sidebar">Loading...</div>
  {/if}
</ExpandableSidebar>

{#if showAddModal}
  <VideoSearchModal
    mode="movie"
    onComplete={handleAddComplete}
    onClose={() => (showAddModal = false)}
  />
{/if}

{#if showMetadataSearch}
  <VideoMetadataSearchModal
    mode="movie"
    initialTitle={(nav.current?.data as Movie)?.title ?? ''}
    onSelect={(results) => {
      editSearchResults = results;
      showMetadataSearch = false;
    }}
    onCancel={() => (showMetadataSearch = false)}
  />
{/if}

{#if showReleaseSearch && nav.current?.type === 'movie' && nav.current.data}
  {@const movieData = nav.current.data as Movie}
  <ReleaseSearchModal
    entityTitle={movieData.title}
    initialQuery={movieData.title}
    onClose={() => (showReleaseSearch = false)}
    searchFn={(query, scope) => api.searchMovieReleases(movieData.id, query, scope)}
    scopeLabel="movie"
    smartSearchFn={() => api.searchMovieReleases(movieData.id)}
    queueFn={async (release, source) => {
      await api.queueMovieRelease(movieData.id, release, source);
      const movie = await api.getMovie(movieData.id);
      updateCurrentEntityData(movie);
      await reloadInPlace();
    }}
    wantFn={async () => {
      await api.wantMovie(movieData.id);
      const movie = await api.getMovie(movieData.id);
      updateCurrentEntityData(movie);
      await reloadInPlace();
    }}
  />
{/if}

{#if movingFileId}
  <MoveFileModal
    title="Move to movie"
    searchFn={searchMoviesForMove}
    onMove={doMoveFile}
    onClose={() => {
      movingFileId = null;
    }}
  />
{/if}

<style>
  .error-wrapper {
    padding: 12px 24px 0;
  }

  .icon-btn {
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
  }

  .icon-btn:hover {
    background: var(--bg);
    color: var(--ink);
  }

  .loading-sidebar {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 40px;
    color: var(--muted);
    font-size: 13px;
  }

  .sidebar-footer-actions {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .spacer {
    flex: 1;
  }

  .btn-danger-sm {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 6px 10px;
    border: 1px solid var(--error, #ef4444);
    border-radius: 6px;
    background: transparent;
    color: var(--error, #ef4444);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
  }

  .btn-danger-sm:hover {
    background: rgba(239, 68, 68, 0.1);
  }

  .btn-danger-sm:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-danger-sm :global(.material-icons-outlined) {
    font-size: 14px;
  }
</style>
