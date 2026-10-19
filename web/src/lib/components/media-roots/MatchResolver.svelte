<script lang="ts">
  import type {
    MediaRootFile,
    MatchCandidate,
    MovieSearchResult,
    TvSearchResult,
    Book,
    Movie,
    TvShow,
    TvSeason
  } from '$lib/types';
  import type { MetadataResult } from '$lib/api/client';
  import {
    resolveMediaRootFileMatch,
    bulkManualResolveMediaRootFiles,
    linkMediaRootFileToEntity,
    bulkLinkMediaRootFilesToEntity,
    searchTvShows,
    searchMovies,
    searchBooks,
    searchSportsLeagues,
    getBooks,
    getMovies,
    getTvShows,
    getTvShow,
    createMovie,
    createTvShow
  } from '$lib/api/client';
  import { showMessage } from '$lib/context/app.svelte';
  import AddEntityModal from '$lib/components/AddEntityModal.svelte';
  import MetaLine from '$lib/components/shared/MetaLine.svelte';

  type Tab = 'provider' | 'library';

  interface Props {
    file: MediaRootFile;
    mediaType: 'tv' | 'movie' | 'book' | 'audiobook' | 'sport';
    open: boolean;
    onClose: () => void;
    onResolved: () => void;
    additionalFileIds?: string[];
  }

  let { file, mediaType, open, onClose, onResolved, additionalFileIds }: Props = $props();

  let activeTab = $state<Tab>('provider');
  let resolving = $state(false);

  // Provider search state — pre-fill from interpretation
  function buildSuggestedQuery(): string {
    const interp = file.interpretation;
    if (!interp) return '';
    if (mediaType === 'book' || mediaType === 'audiobook') {
      const title = (interp.title as string) ?? '';
      const authors = interp.authors as string[] | undefined;
      const firstAuthor = authors?.[0] ?? '';
      if (title && firstAuthor) return `${title} ${firstAuthor}`;
      return title || firstAuthor;
    }
    if (mediaType === 'tv' || mediaType === 'sport') {
      const name = (interp.show_name as string) ?? '';
      const year = interp.show_year ?? interp.year;
      return year ? `${name} ${year}` : name;
    }
    // movie
    const title = (interp.movie_title as string) ?? '';
    const year = interp.movie_year ?? interp.year;
    return year ? `${title} ${year}` : title;
  }
  let searchQuery = $state(buildSuggestedQuery());
  let searching = $state(false);
  let searchResults = $state<(MovieSearchResult | TvSearchResult | MetadataResult)[]>([]);
  let hasSearched = $state(false);
  let searchProvider = $state<string>('');

  // Library search state
  let libQuery = $state('');
  let libSearching = $state(false);
  let libBooks = $state<Book[]>([]);
  let libMovies = $state<Movie[]>([]);
  let libShows = $state<TvShow[]>([]);
  let libHasSearched = $state(false);

  // TV episode picker state
  let selectedShow = $state<TvShow | null>(null);
  let loadingShow = $state(false);

  // Create & Link state
  let showCreateModal = $state(false);
  let createTitle = $state('');
  let createYear = $state('');
  let showInlineCreate = $state(false);

  const interpretation = $derived(file.interpretation);
  const candidates = $derived(file.match_candidates ?? []);
  const totalFileCount = $derived(1 + (additionalFileIds?.length ?? 0));
  const isGroupResolve = $derived(!!additionalFileIds?.length);

  const mediaIcon = $derived(
    mediaType === 'tv'
      ? 'tv'
      : mediaType === 'sport'
        ? 'sports_motorsports'
        : mediaType === 'book' || mediaType === 'audiobook'
          ? 'menu_book'
          : 'movie'
  );

  function groupLabel(): string {
    if (!interpretation) return file.file_name;
    if (mediaType === 'tv' || mediaType === 'sport')
      return (interpretation.show_name as string) ?? file.file_name;
    if (mediaType === 'movie') return (interpretation.movie_title as string) ?? file.file_name;
    return (
      (interpretation.series_name as string) ?? (interpretation.title as string) ?? file.file_name
    );
  }

  function interpretationSummary(): string[] {
    if (!interpretation) return [];
    const parts: string[] = [];
    if (interpretation.authors) {
      const authors = interpretation.authors as string[];
      if (authors.length > 0) parts.push(authors.join(', '));
    }
    if (interpretation.series_name) parts.push(String(interpretation.series_name));
    if (interpretation.title && interpretation.title !== interpretation.series_name) {
      parts.push(String(interpretation.title));
    }
    if (interpretation.year) parts.push(String(interpretation.year));
    if (interpretation.isbn) parts.push(String(interpretation.isbn));
    if (interpretation.season != null)
      parts.push(`S${String(interpretation.season).padStart(2, '0')}`);
    if (interpretation.episode != null)
      parts.push(`E${String(interpretation.episode).padStart(2, '0')}`);
    if (interpretation.resolution) parts.push(String(interpretation.resolution));
    return parts;
  }

  // ---- Provider search ----

  async function resolveBulkRemaining(candidate: {
    provider: string;
    provider_id: string;
    title: string;
    year?: number | string | null;
  }) {
    if (additionalFileIds?.length) {
      try {
        await bulkManualResolveMediaRootFiles(additionalFileIds, candidate);
      } catch (e) {
        console.error('Bulk resolve failed:', e);
        showMessage(`Failed to resolve remaining ${additionalFileIds.length} files`);
      }
    }
  }

  async function handleSelect(candidate: MatchCandidate) {
    resolving = true;
    try {
      const resolveData = {
        provider: candidate.provider,
        provider_id: candidate.provider_id,
        title: candidate.title,
        year: candidate.year
      };
      const resp = await resolveMediaRootFileMatch(file.id, resolveData);
      await resolveBulkRemaining(resolveData);
      if (isGroupResolve) {
        showMessage(`Identified ${totalFileCount} files as "${candidate.title}"`);
      } else if (resp.status === 'confirmed') {
        showMessage(`Identified as "${candidate.title}"`);
      } else {
        showMessage(`Added "${candidate.title}" to library`);
      }
      onResolved();
      onClose();
    } catch (e) {
      showMessage('Failed to resolve match');
      console.error('Resolve failed:', e);
    } finally {
      resolving = false;
    }
  }

  async function handleSelectSearchResult(
    result: MovieSearchResult | TvSearchResult | MetadataResult
  ) {
    resolving = true;
    try {
      const isBook = 'authors' in result && !('genres' in result);
      const year = isBook
        ? ((result as MetadataResult).publish_date?.slice(0, 4) ?? null)
        : (result as MovieSearchResult | TvSearchResult).year;
      const resolveData = {
        provider: result.provider,
        provider_id: result.external_id ?? '',
        title: result.title,
        year
      };
      const resp = await resolveMediaRootFileMatch(file.id, resolveData);
      await resolveBulkRemaining(resolveData);
      if (isGroupResolve) {
        showMessage(`Identified ${totalFileCount} files as "${result.title}"`);
      } else if (resp.status === 'confirmed') {
        showMessage(`Identified as "${result.title}"`);
      } else {
        showMessage(`Added "${result.title}" to library`);
      }
      onResolved();
      onClose();
    } catch (e) {
      showMessage('Failed to resolve match');
      console.error('Resolve failed:', e);
    } finally {
      resolving = false;
    }
  }

  async function handleProviderSearch() {
    const q = searchQuery.trim();
    if (!q) return;
    searching = true;
    hasSearched = true;
    searchResults = [];
    try {
      const providerOpt = searchProvider ? { provider: searchProvider } : undefined;
      if (mediaType === 'tv') {
        searchResults = await searchTvShows(q, providerOpt);
      } else if (mediaType === 'sport') {
        searchResults = await searchSportsLeagues(q);
      } else if (mediaType === 'book' || mediaType === 'audiobook') {
        searchResults = await searchBooks(q, providerOpt);
      } else {
        searchResults = await searchMovies(q, providerOpt);
      }
    } catch (e) {
      showMessage('Search failed');
      console.error('Search failed:', e);
    } finally {
      searching = false;
    }
  }

  function handleProviderKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      handleProviderSearch();
    }
  }

  function resultPoster(
    result: MovieSearchResult | TvSearchResult | MetadataResult
  ): string | null {
    if ('poster_url' in result) return (result as MovieSearchResult).poster_url;
    return (result as MetadataResult).cover_url ?? null;
  }

  function resultYear(result: MovieSearchResult | TvSearchResult | MetadataResult): string | null {
    if ('year' in result && (result as MovieSearchResult).year != null) {
      return String((result as MovieSearchResult).year);
    }
    const pd = (result as MetadataResult).publish_date;
    return pd ? pd.slice(0, 4) : null;
  }

  // ---- Library search ----

  async function handleLibrarySearch() {
    const q = libQuery.trim();
    if (!q) return;
    libSearching = true;
    libHasSearched = true;
    libBooks = [];
    libMovies = [];
    libShows = [];
    selectedShow = null;
    try {
      if (mediaType === 'book' || mediaType === 'audiobook') {
        const resp = await getBooks({ search: q, limit: 20 });
        libBooks = resp.items;
      } else if (mediaType === 'movie') {
        const resp = await getMovies({ search: q, limit: 20 });
        libMovies = resp.items;
      } else {
        const resp = await getTvShows({ search: q, limit: 20 });
        libShows = resp.items;
      }
    } catch (e) {
      showMessage('Library search failed');
      console.error('Library search failed:', e);
    } finally {
      libSearching = false;
    }
  }

  function handleLibKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      handleLibrarySearch();
    }
  }

  async function linkToEntity(entityType: string, entityId: string, title: string) {
    resolving = true;
    try {
      await linkMediaRootFileToEntity(file.id, { entity_type: entityType, entity_id: entityId });
      if (additionalFileIds?.length) {
        await bulkLinkMediaRootFilesToEntity(additionalFileIds, entityType, entityId);
      }
      if (isGroupResolve) {
        showMessage(`Linked ${totalFileCount} files to "${title}"`);
      } else {
        showMessage(`Linked to "${title}"`);
      }
      onResolved();
      onClose();
    } catch (e) {
      showMessage('Failed to link file');
      console.error('Link failed:', e);
    } finally {
      resolving = false;
    }
  }

  function handleSelectBook(book: Book) {
    linkToEntity('book', book.id, book.title);
  }

  function handleSelectMovie(movie: Movie) {
    linkToEntity('movie', movie.id, movie.title);
  }

  async function handleSelectShow(show: TvShow) {
    loadingShow = true;
    try {
      const detail = await getTvShow(show.id);
      selectedShow = detail;
    } catch (e) {
      showMessage('Failed to load show details');
      console.error(e);
    } finally {
      loadingShow = false;
    }
  }

  function handleSelectEpisode(ep: { id: string }, season: TvSeason, show: TvShow) {
    const label = `${show.title} S${season.season_identifier}E${ep.id.slice(0, 8)}`;
    linkToEntity('tv_episode', ep.id, label);
  }

  // ---- Create & Link ----

  async function handleCreateAndLink() {
    if (mediaType === 'book' || mediaType === 'audiobook') {
      showCreateModal = true;
      return;
    }
    showInlineCreate = true;
    createTitle = groupLabel();
    createYear = interpretation?.year ? String(interpretation.year) : '';
  }

  async function handleBookCreated(bookId: string) {
    showCreateModal = false;
    await linkToEntity('book', bookId, 'new book');
  }

  async function handleInlineCreate() {
    const title = createTitle.trim();
    if (!title) return;
    resolving = true;
    try {
      const year = createYear ? parseInt(createYear) : undefined;
      if (mediaType === 'movie') {
        const movie = await createMovie({ title, year });
        showInlineCreate = false;
        await linkToEntity('movie', movie.id, title);
      } else {
        const show = await createTvShow({ title, year });
        showInlineCreate = false;
        // For TV, load the show detail to pick episode
        const detail = await getTvShow(show.id);
        selectedShow = detail;
        resolving = false;
      }
    } catch (e) {
      showMessage('Failed to create entity');
      console.error(e);
      resolving = false;
    }
  }

  function handleBackdropClick() {
    if (!resolving) onClose();
  }

  const providerSearchLabel = $derived(
    mediaType === 'tv'
      ? 'TV shows'
      : mediaType === 'sport'
        ? 'leagues'
        : mediaType === 'book' || mediaType === 'audiobook'
          ? 'books'
          : 'movies'
  );

  const libResultCount = $derived(libBooks.length + libMovies.length + libShows.length);
</script>

{#if showCreateModal}
  <AddEntityModal
    mode="book"
    onComplete={handleBookCreated}
    onCancel={() => (showCreateModal = false)}
  />
{/if}

{#if open}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="backdrop"
    onclick={handleBackdropClick}
    onkeydown={(e) => e.key === 'Escape' && !resolving && onClose()}
  >
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="dialog" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
      <div class="dialog-header">
        <h3>{isGroupResolve ? `Resolve ${totalFileCount} files` : 'Resolve Match'}</h3>
        <button class="close-btn" type="button" onclick={onClose} disabled={resolving}>
          <span class="material-icons-outlined">close</span>
        </button>
      </div>

      <div class="file-info">
        {#if isGroupResolve}
          <div class="file-name">{groupLabel()}</div>
          <div class="parsed">{totalFileCount} files in this group</div>
        {:else}
          <div class="file-name">{file.file_name}</div>
          {#if interpretationSummary().length > 0}
            <div class="parsed"><MetaLine items={interpretationSummary()} /></div>
          {/if}
        {/if}
      </div>

      <!-- Tab switcher -->
      <div class="tabs">
        <button
          class="tab"
          class:active={activeTab === 'provider'}
          type="button"
          onclick={() => (activeTab = 'provider')}
        >
          Provider Search
        </button>
        <button
          class="tab"
          class:active={activeTab === 'library'}
          type="button"
          onclick={() => (activeTab = 'library')}
        >
          Library
        </button>
      </div>

      {#if activeTab === 'provider'}
        <!-- Provider Search Tab (existing behavior) -->
        {#if candidates.length > 0}
          <div class="section-label">Candidates ({candidates.length})</div>
          <div class="candidates">
            {#each candidates as candidate, i (candidate.provider + ':' + (candidate.provider_id ?? '') + ':' + i)}
              <button
                class="candidate-card"
                type="button"
                onclick={() => handleSelect(candidate)}
                disabled={resolving}
              >
                {#if candidate.poster_url || candidate.cover_url}
                  <img class="poster" src={candidate.poster_url ?? candidate.cover_url} alt="" />
                {:else}
                  <div class="poster placeholder">
                    <span class="material-icons-outlined">{mediaIcon}</span>
                  </div>
                {/if}
                <div class="candidate-info">
                  <div class="candidate-title">{candidate.title}</div>
                  {#if candidate.authors?.length}
                    <div class="candidate-authors">{candidate.authors.join(', ')}</div>
                  {/if}
                  <div class="candidate-meta">
                    <span class="provider-tag">{candidate.provider}</span>
                    {#if candidate.confidence != null}
                      <span class="confidence">{Math.round(candidate.confidence * 100)}%</span>
                    {/if}
                    {#if candidate.year}
                      <span>{candidate.year}</span>
                    {/if}
                  </div>
                </div>
              </button>
            {/each}
          </div>
        {/if}

        <div class="section-label">Manual Search</div>
        <div class="search-row">
          <input
            type="text"
            class="search-input"
            placeholder="Search {providerSearchLabel}..."
            bind:value={searchQuery}
            onkeydown={handleProviderKeydown}
          />
          <select class="search-input provider-select" bind:value={searchProvider}>
            <option value="">All Providers</option>
            {#if mediaType === 'book' || mediaType === 'audiobook'}
              <option value="hardcover">Hardcover</option>
              <option value="googlebooks">Google Books</option>
              <option value="openlibrary">Open Library</option>
            {:else}
              <option value="tmdb">TMDB</option>
            {/if}
          </select>
          <button
            class="btn-search"
            type="button"
            onclick={handleProviderSearch}
            disabled={searching || !searchQuery.trim()}
          >
            {searching ? 'Searching...' : 'Search'}
          </button>
        </div>

        {#if searching}
          <div class="status-msg">Searching...</div>
        {:else if hasSearched && searchResults.length === 0}
          <div class="status-msg">No results found</div>
        {:else if searchResults.length > 0}
          <div class="candidates">
            {#each searchResults as result, i (result.provider + ':' + (result.external_id ?? '') + ':' + i)}
              <button
                class="candidate-card"
                type="button"
                onclick={() => handleSelectSearchResult(result)}
                disabled={resolving}
              >
                {#if resultPoster(result)}
                  <img class="poster" src={resultPoster(result)} alt="" />
                {:else}
                  <div class="poster placeholder">
                    <span class="material-icons-outlined">{mediaIcon}</span>
                  </div>
                {/if}
                <div class="candidate-info">
                  <div class="candidate-title">{result.title}</div>
                  {#if 'authors' in result && result.authors?.length}
                    <div class="candidate-authors">{result.authors.join(', ')}</div>
                  {/if}
                  <div class="candidate-meta">
                    <span class="provider-tag">{result.provider}</span>
                    {#if resultYear(result)}
                      <span>{resultYear(result)}</span>
                    {/if}
                  </div>
                  {#if result.description}
                    <div class="candidate-desc">
                      {result.description.slice(0, 100)}{result.description.length > 100
                        ? '...'
                        : ''}
                    </div>
                  {/if}
                </div>
              </button>
            {/each}
          </div>
        {/if}
      {:else}
        <!-- Library Tab -->
        {#if selectedShow}
          <!-- TV Episode Picker -->
          <div class="section-label">
            <button class="back-link" type="button" onclick={() => (selectedShow = null)}>
              <span class="material-icons-outlined" style="font-size: 14px">arrow_back</span>
            </button>
            Pick episode from "{selectedShow.title}"
          </div>
          {#if selectedShow.seasons?.length}
            <div class="candidates">
              {#each selectedShow.seasons as season (season.id)}
                <div class="season-group">
                  <div class="season-header">
                    {season.title ?? `Season ${season.season_identifier}`}
                  </div>
                  {#each season.episodes as ep (ep.id)}
                    <button
                      class="candidate-card episode-card"
                      type="button"
                      onclick={() => handleSelectEpisode(ep, season, selectedShow!)}
                      disabled={resolving}
                    >
                      <div class="candidate-info">
                        <div class="candidate-title">
                          E{ep.episode_identifier} - {ep.title ?? 'Untitled'}
                        </div>
                        {#if ep.air_date}
                          <div class="candidate-meta">
                            <span>{ep.air_date}</span>
                          </div>
                        {/if}
                      </div>
                    </button>
                  {/each}
                </div>
              {/each}
            </div>
          {:else}
            <div class="status-msg">No seasons/episodes found. Create episodes first.</div>
          {/if}
        {:else}
          <!-- Library Search -->
          <div class="search-row">
            <input
              type="text"
              class="search-input"
              placeholder="Search your library..."
              bind:value={libQuery}
              onkeydown={handleLibKeydown}
            />
            <button
              class="btn-search"
              type="button"
              onclick={handleLibrarySearch}
              disabled={libSearching || !libQuery.trim()}
            >
              {libSearching ? 'Searching...' : 'Search'}
            </button>
          </div>

          {#if libSearching}
            <div class="status-msg">Searching library...</div>
          {:else if libHasSearched && libResultCount === 0}
            <div class="status-msg">No matching entities in your library</div>
          {:else}
            {#if libBooks.length > 0}
              <div class="candidates">
                {#each libBooks as book (book.id)}
                  <button
                    class="candidate-card"
                    type="button"
                    onclick={() => handleSelectBook(book)}
                    disabled={resolving}
                  >
                    {#if book.cover_url}
                      <img class="poster" src={book.cover_url} alt="" />
                    {:else}
                      <div class="poster placeholder">
                        <span class="material-icons-outlined">menu_book</span>
                      </div>
                    {/if}
                    <div class="candidate-info">
                      <div class="candidate-title">{book.title}</div>
                      {#if book.authors?.length}
                        <div class="candidate-authors">
                          {book.authors.map((a) => a.name).join(', ')}
                        </div>
                      {/if}
                      <div class="candidate-meta">
                        <span class="provider-tag library-tag">library</span>
                      </div>
                    </div>
                  </button>
                {/each}
              </div>
            {/if}
            {#if libMovies.length > 0}
              <div class="candidates">
                {#each libMovies as movie (movie.id)}
                  <button
                    class="candidate-card"
                    type="button"
                    onclick={() => handleSelectMovie(movie)}
                    disabled={resolving}
                  >
                    {#if movie.poster_url}
                      <img class="poster" src={movie.poster_url} alt="" />
                    {:else}
                      <div class="poster placeholder">
                        <span class="material-icons-outlined">movie</span>
                      </div>
                    {/if}
                    <div class="candidate-info">
                      <div class="candidate-title">{movie.title}</div>
                      <div class="candidate-meta">
                        <span class="provider-tag library-tag">library</span>
                        {#if movie.year}<span>{movie.year}</span>{/if}
                      </div>
                    </div>
                  </button>
                {/each}
              </div>
            {/if}
            {#if libShows.length > 0}
              <div class="candidates">
                {#each libShows as show (show.id)}
                  <button
                    class="candidate-card"
                    type="button"
                    onclick={() => handleSelectShow(show)}
                    disabled={resolving || loadingShow}
                  >
                    {#if show.poster_url}
                      <img class="poster" src={show.poster_url} alt="" />
                    {:else}
                      <div class="poster placeholder">
                        <span class="material-icons-outlined">tv</span>
                      </div>
                    {/if}
                    <div class="candidate-info">
                      <div class="candidate-title">{show.title}</div>
                      <div class="candidate-meta">
                        <span class="provider-tag library-tag">library</span>
                        {#if show.year}<span>{show.year}</span>{/if}
                      </div>
                    </div>
                  </button>
                {/each}
              </div>
            {/if}
          {/if}

          <!-- Create & Link section -->
          {#if showInlineCreate}
            <div class="section-label">Create New</div>
            <div class="inline-create">
              <input
                type="text"
                class="search-input"
                placeholder="Title"
                bind:value={createTitle}
              />
              <input
                type="text"
                class="search-input year-input"
                placeholder="Year"
                bind:value={createYear}
              />
              <button
                class="btn-search"
                type="button"
                onclick={handleInlineCreate}
                disabled={resolving || !createTitle.trim()}
              >
                Create
              </button>
            </div>
          {:else}
            <button class="btn-create" type="button" onclick={handleCreateAndLink}>
              <span class="material-icons-outlined" style="font-size: 16px">add</span>
              Create new &amp; link
            </button>
          {/if}
        {/if}
      {/if}

      {#if resolving}
        <div class="resolving-overlay">
          <span class="material-icons-outlined spin">sync</span>
          {isGroupResolve ? `Resolving ${totalFileCount} files...` : 'Resolving...'}
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    z-index: 200;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .dialog {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    max-width: 500px;
    width: 90vw;
    max-height: 85vh;
    overflow-y: auto;
    padding: 20px;
    position: relative;
  }

  .dialog-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 16px;
  }

  .dialog-header h3 {
    margin: 0;
    font-size: 16px;
    font-weight: 700;
    color: var(--ink);
  }

  .close-btn {
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

  .close-btn:hover {
    background: var(--bg);
    color: var(--ink);
  }

  .file-info {
    padding: 10px 12px;
    background: var(--bg);
    border: 1px solid var(--ring);
    border-radius: 10px;
    margin-bottom: 16px;
  }

  .file-name {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
    word-break: break-all;
  }

  .parsed {
    font-size: 11px;
    color: var(--muted);
    margin-top: 4px;
  }

  .tabs {
    display: flex;
    gap: 4px;
    margin-bottom: 16px;
    background: var(--bg);
    border-radius: 8px;
    padding: 3px;
  }

  .tab {
    flex: 1;
    padding: 7px 12px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition:
      background 0.15s,
      color 0.15s;
  }

  .tab.active {
    background: var(--card);
    color: var(--ink);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.15);
  }

  .tab:hover:not(.active) {
    color: var(--ink);
  }

  .section-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    margin-bottom: 8px;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .candidates {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 16px;
    max-height: 240px;
    overflow-y: auto;
  }

  .candidate-card {
    display: flex;
    gap: 10px;
    padding: 10px;
    border: 1px solid var(--ring);
    border-radius: 10px;
    background: var(--bg);
    cursor: pointer;
    text-align: left;
    color: inherit;
    font: inherit;
    transition:
      border-color 0.15s,
      background 0.15s;
    appearance: none;
  }

  .candidate-card:hover:not(:disabled) {
    border-color: var(--accent);
    background: var(--card);
  }

  .candidate-card:disabled {
    opacity: 0.5;
    cursor: wait;
  }

  .episode-card {
    padding: 8px 10px;
    margin-left: 12px;
  }

  .poster {
    width: 44px;
    height: 64px;
    border-radius: 6px;
    object-fit: cover;
    flex-shrink: 0;
  }

  .poster.placeholder {
    background: linear-gradient(140deg, #2e323a, #1a1d23);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--muted);
    font-size: 20px;
    opacity: 0.4;
  }

  .candidate-info {
    flex: 1;
    min-width: 0;
  }

  .candidate-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
  }

  .candidate-authors {
    font-size: 11px;
    color: var(--muted);
    font-style: italic;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .candidate-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: var(--muted);
    margin-top: 2px;
  }

  .provider-tag {
    font-size: 9px;
    font-weight: 700;
    padding: 1px 5px;
    border-radius: 3px;
    background: var(--accent);
    color: #1d130c;
    text-transform: uppercase;
  }

  .library-tag {
    background: var(--muted);
    color: var(--bg);
  }

  .confidence {
    font-size: 10px;
    font-weight: 600;
    color: var(--accent);
  }

  .candidate-desc {
    font-size: 11px;
    color: var(--muted);
    margin-top: 4px;
    line-height: 1.3;
  }

  .search-row {
    display: flex;
    gap: 8px;
    margin-bottom: 12px;
  }

  .search-input {
    flex: 1;
    padding: 10px 12px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--bg);
    color: var(--ink);
    font-size: 14px;
    outline: none;
  }

  .search-input:focus {
    border-color: var(--accent);
  }

  .provider-select {
    flex: 0 0 140px;
    cursor: pointer;
  }

  .btn-search {
    padding: 10px 16px;
    border-radius: 8px;
    border: none;
    background: var(--accent);
    color: #1d130c;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
  }

  .btn-search:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
  }

  .btn-search:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-create {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 10px 14px;
    border: 1px dashed var(--ring);
    border-radius: 10px;
    background: transparent;
    color: var(--muted);
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition:
      border-color 0.15s,
      color 0.15s;
    margin-top: 4px;
  }

  .btn-create:hover {
    border-color: var(--accent);
    color: var(--ink);
  }

  .inline-create {
    display: flex;
    gap: 8px;
    margin-bottom: 12px;
  }

  .year-input {
    flex: 0 0 70px;
  }

  .season-group {
    margin-bottom: 8px;
  }

  .season-header {
    font-size: 11px;
    font-weight: 700;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    padding: 4px 0;
  }

  .back-link {
    background: none;
    border: none;
    color: var(--accent);
    cursor: pointer;
    padding: 0;
    display: inline-flex;
    align-items: center;
  }

  .back-link:hover {
    color: var(--ink);
  }

  .status-msg {
    text-align: center;
    color: var(--muted);
    padding: 20px;
    font-size: 13px;
  }

  .resolving-overlay {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    border-radius: 16px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    color: var(--ink);
    font-size: 14px;
    font-weight: 600;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  .spin {
    animation: spin 1s linear infinite;
  }
</style>
