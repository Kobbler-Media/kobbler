<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { mediaTypeChip } from '$lib/utils/chip-mappings';
  import { getProviderName, getProviderColor, formatDuration } from '$lib/utils/format';
  import IconChip from '$lib/components/shared/IconChip.svelte';
  import MediaCard from '$lib/components/shared/MediaCard.svelte';
  import ScrollRow from '$lib/components/shared/ScrollRow.svelte';
  import { updateUrlParams } from '$lib/utils/url-state';
  import { discoverLibraryRoute } from '$lib/utils/entity-routes';
  import { onDebounced, off, onReconnect, offReconnect } from '$lib/services/eventstream.svelte';
  import {
    searchMovies,
    searchTvShows,
    searchBooks,
    createMovie,
    createTvShow,
    createBook,
    getDiscoverLists,
    dismissForYou,
    undismissForYou,
    getVideoMetadataProviders,
    getMetadataProviders,
    type MetadataResult,
    type DiscoverResponse,
    type ForYouItem,
    type StremioContinueItem
  } from '$lib/api/client';
  import type { MovieSearchResult, TvSearchResult, WatchNextItem, RecentItem } from '$lib/types';
  import DiscoverRow from '$lib/components/discover/DiscoverRow.svelte';
  import DiscoverCard from '$lib/components/discover/DiscoverCard.svelte';
  import { watchStateVersion } from '$lib/stores/watchState.svelte';

  type TypeFilter = 'all' | 'movies' | 'tv' | 'books';

  type DiscoverResult =
    | { kind: 'movie'; data: MovieSearchResult }
    | { kind: 'tv'; data: TvSearchResult }
    | { kind: 'book'; data: MetadataResult };

  type DiscoverProvider = {
    id: string;
    displayName: string;
    domain: 'video' | 'book' | 'all';
    supportedTypes: TypeFilter[];
  };

  const ALL_PROVIDERS: DiscoverProvider = {
    id: '__all__',
    displayName: 'All',
    domain: 'all',
    supportedTypes: ['movies', 'tv', 'books']
  };

  let searchInput = $state('');
  let typeFilter = $state<TypeFilter>('all');
  let results = $state<DiscoverResult[]>([]);
  let loading = $state(false);
  let searched = $state(false);
  let addingIds = $state<Set<string>>(new Set());
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;
  let inputRef: HTMLInputElement | undefined = $state();

  // Provider state
  let providers = $state<DiscoverProvider[]>([ALL_PROVIDERS]);
  let selectedProvider = $state<DiscoverProvider>(ALL_PROVIDERS);
  let defaultProvider = $state<DiscoverProvider>(ALL_PROVIDERS);
  let providersLoaded = $state(false);

  // Curated discover lists
  let discover = $state<DiscoverResponse | null>(null);
  let discoverLoading = $state(true);

  // Library rows
  let watchNext = $state<WatchNextItem[]>([]);
  let recentlyAdded = $state<RecentItem[]>([]);
  let stremioContinue = $state<StremioContinueItem[]>([]);
  let forYou = $state<ForYouItem[]>([]);

  const hasDiscoverContent = $derived(discover && (discover.sections?.length ?? 0) > 0);

  // SSE callbacks
  let libraryCallback: ((data: any) => void) | null = null;
  let tvCallback: ((data: any) => void) | null = null;
  let movieCallback: ((data: any) => void) | null = null;
  let reconnectHandler: (() => void) | null = null;

  function applyDiscover(data: DiscoverResponse) {
    discover = data;
    watchNext = data.watch_next ?? [];
    recentlyAdded = data.recently_added ?? [];
    stremioContinue = data.stremio_continue ?? [];
    forYou = data.for_you ?? [];
  }

  function refreshDiscoverData() {
    getDiscoverLists()
      .then(applyDiscover)
      .catch((err) => {
        console.error('Failed to load discover lists:', err);
      });
  }

  let lastWatchVersion = -1;
  $effect(() => {
    const v = watchStateVersion();
    if (lastWatchVersion !== -1 && v !== lastWatchVersion) refreshDiscoverData();
    lastWatchVersion = v;
  });

  onMount(() => {
    getDiscoverLists()
      .then(applyDiscover)
      .catch((err) => {
        console.error('Failed to load discover lists:', err);
        discover = null;
      })
      .finally(() => {
        discoverLoading = false;
      });

    // Fetch providers to build the provider selector
    Promise.all([
      getVideoMetadataProviders().catch(() => []),
      getMetadataProviders().catch(() => [])
    ]).then(([videoProv, bookProv]) => {
      const unified: DiscoverProvider[] = [];

      for (const vp of videoProv) {
        if (!vp.enabled) continue;
        const types: TypeFilter[] = [];
        if (vp.capabilities.includes('movie_search')) types.push('movies');
        if (vp.capabilities.includes('tv_search')) types.push('tv');
        if (types.length === 0) continue;
        unified.push({
          id: vp.provider_type,
          displayName: vp.display_name,
          domain: 'video',
          supportedTypes: types
        });
      }

      for (const bp of bookProv) {
        if (!bp.enabled) continue;
        if (!bp.capabilities.includes('title_search')) continue;
        unified.push({
          id: bp.provider_type,
          displayName: bp.display_name,
          domain: 'book',
          supportedTypes: ['books']
        });
      }

      // Already sorted by priority from API
      providers = [ALL_PROVIDERS, ...unified];

      // Determine the default provider (highest-priority video with both capabilities)
      const bestVideo = unified.find(
        (p) =>
          p.domain === 'video' &&
          p.supportedTypes.includes('movies') &&
          p.supportedTypes.includes('tv')
      );
      defaultProvider = bestVideo ?? ALL_PROVIDERS;

      // Use URL provider if specified, otherwise use default
      const urlProv = $page.url.searchParams.get('provider');
      if (urlProv) {
        const found = providers.find((p) => p.id === urlProv);
        selectedProvider = found ?? defaultProvider;
      } else {
        selectedProvider = defaultProvider;
      }
      providersLoaded = true;
    });

    // SSE listeners for live updates
    reconnectHandler = onReconnect(() => {
      refreshDiscoverData();
    });
    libraryCallback = onDebounced('library_updated', () => refreshDiscoverData(), 300);
    tvCallback = onDebounced('tv_show_updated', () => refreshDiscoverData(), 300);
    movieCallback = onDebounced('movie_updated', () => refreshDiscoverData(), 300);
  });

  onDestroy(() => {
    if (libraryCallback) off('library_updated', libraryCallback);
    if (tvCallback) off('tv_show_updated', tvCallback);
    if (movieCallback) off('movie_updated', movieCallback);
    if (reconnectHandler) offReconnect(reconnectHandler);
  });

  // Read initial query and type from URL
  const urlQuery = $derived($page.url.searchParams.get('q') ?? '');
  const urlType = $derived(($page.url.searchParams.get('type') as TypeFilter) ?? 'all');

  const allFilterOptions: { value: TypeFilter; label: string; icon: string }[] = [
    {
      value: 'all',
      label: 'All',
      icon: 'M3 3h8v8H3zm10 0h8v8h-8zM3 13h8v8H3zm10 0h8v8h-8z'
    },
    {
      value: 'movies',
      label: 'Movies',
      icon: 'M18 4l2 4h-3l-2-4h-2l2 4h-3l-2-4H8l2 4H7L5 4H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V4h-4z'
    },
    {
      value: 'tv',
      label: 'TV Shows',
      icon: 'M21 3H3c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h5v2h8v-2h5c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 14H3V5h18v12z'
    },
    {
      value: 'books',
      label: 'Books',
      icon: 'M6 4h12a2 2 0 0 1 2 2v12h-2V6H6V4zm0 4h10a2 2 0 0 1 2 2v10H6a2 2 0 0 1-2-2V10a2 2 0 0 1 2-2zm0 2v8h10v-8H6z'
    }
  ];

  // Filter options based on selected provider's supported types
  const filterOptions = $derived(
    allFilterOptions.filter(
      (f) => f.value === 'all' || selectedProvider.supportedTypes.includes(f.value)
    )
  );

  // Dynamic placeholder based on provider
  const searchPlaceholder = $derived.by(() => {
    const types = selectedProvider.supportedTypes;
    if (types.length === 3 || selectedProvider.id === '__all__')
      return 'Search movies, TV shows, books...';
    const labels: string[] = [];
    if (types.includes('movies')) labels.push('movies');
    if (types.includes('tv')) labels.push('TV shows');
    if (types.includes('books')) labels.push('books');
    return `Search ${labels.join(' and ')}...`;
  });

  function setTypeFilter(f: TypeFilter) {
    typeFilter = f;
    updateUrlParams({ type: f === 'all' ? null : f });
    if (searchInput.trim()) doSearch(searchInput);
  }

  function selectProvider(p: DiscoverProvider) {
    selectedProvider = p;
    // Only write provider to URL if it differs from the default
    const isDefault = p.id === defaultProvider.id;
    updateUrlParams({ provider: isDefault ? null : p.id });
    // Reset type filter if current selection isn't supported
    if (typeFilter !== 'all' && !p.supportedTypes.includes(typeFilter)) {
      typeFilter = 'all';
      updateUrlParams({ type: null });
    }
    if (searchInput.trim()) doSearch(searchInput);
  }

  // Initialize search input from URL on first load
  // Wait for providers to load so we search with the correct default provider
  let initialized = false;
  $effect(() => {
    if (initialized || !providersLoaded) return;
    if (['all', 'movies', 'tv', 'books'].includes(urlType)) {
      typeFilter = urlType;
    }
    if (urlQuery) {
      searchInput = urlQuery;
      doSearch(urlQuery);
    }
    initialized = true;
  });

  function getResultId(r: DiscoverResult): string {
    switch (r.kind) {
      case 'movie':
        return `movie-${r.data.provider}-${r.data.external_id}`;
      case 'tv':
        return `tv-${r.data.provider}-${r.data.external_id}`;
      case 'book':
        return `book-${r.data.provider}-${r.data.external_id ?? `title-${r.data.title}`}`;
    }
  }

  function providerLabel(provider: string): string {
    return getProviderName(provider);
  }

  const filteredResults = $derived(results);

  async function doSearch(query: string) {
    if (!query.trim()) {
      results = [];
      searched = false;
      return;
    }

    loading = true;
    searched = true;

    try {
      const isAll = selectedProvider.id === '__all__';
      const providerParam = isAll ? undefined : selectedProvider.id;
      const domain = selectedProvider.domain;
      const searchTypes = typeFilter === 'all' ? selectedProvider.supportedTypes : [typeFilter];

      // Kick off all three in parallel, then deterministically merge in the
      // order TV → movies → books so TV ties surface first (TMDB search is
      // typically better-populated for shows, and exact-title hits there
      // deserve the top slot).
      const tvPromise =
        searchTypes.includes('tv') && (isAll || domain === 'video')
          ? searchTvShows(query, { provider: providerParam })
              .then((r) => r.map((data) => ({ kind: 'tv' as const, data })))
              .catch((): DiscoverResult[] => [])
          : Promise.resolve([] as DiscoverResult[]);
      const moviesPromise =
        searchTypes.includes('movies') && (isAll || domain === 'video')
          ? searchMovies(query, { provider: providerParam })
              .then((r) => r.map((data) => ({ kind: 'movie' as const, data })))
              .catch((): DiscoverResult[] => [])
          : Promise.resolve([] as DiscoverResult[]);
      const booksPromise =
        searchTypes.includes('books') && (isAll || domain === 'book')
          ? searchBooks(query, { provider: providerParam })
              .then((r) => r.map((data) => ({ kind: 'book' as const, data })))
              .catch((): DiscoverResult[] => [])
          : Promise.resolve([] as DiscoverResult[]);
      const [tvResults, movieResults, bookResults] = await Promise.all([
        tvPromise,
        moviesPromise,
        booksPromise
      ]);
      results = [...tvResults, ...movieResults, ...bookResults];
    } catch {
      results = [];
    } finally {
      loading = false;
    }
  }

  function handleInput(e: Event) {
    const value = (e.target as HTMLInputElement).value;
    searchInput = value;

    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => {
      updateUrlParams({ q: value || null });
      doSearch(value);
    }, 500);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      if (debounceTimer) clearTimeout(debounceTimer);
      updateUrlParams({ q: searchInput || null });
      doSearch(searchInput);
    }
  }

  function resultTitle(r: DiscoverResult): string {
    return r.data.title;
  }

  function resultYear(r: DiscoverResult): number | null {
    switch (r.kind) {
      case 'movie':
      case 'tv':
        return r.data.year;
      case 'book': {
        const d = r.data.publish_date;
        if (!d) return null;
        const y = parseInt(d.slice(0, 4), 10);
        return Number.isNaN(y) ? null : y;
      }
    }
  }

  function resultDescription(r: DiscoverResult): string | null {
    return r.data.description;
  }

  function resultImage(r: DiscoverResult): string | null {
    switch (r.kind) {
      case 'movie':
      case 'tv':
        return r.data.poster_url;
      case 'book':
        return r.data.cover_url;
    }
  }

  function resultLibraryHref(r: DiscoverResult): string | null {
    switch (r.kind) {
      case 'movie':
      case 'tv':
        return discoverLibraryRoute(r.kind, r.data.library_id);
      case 'book':
        return null;
    }
  }

  function resultSubtitle(r: DiscoverResult): string | null {
    switch (r.kind) {
      case 'movie':
        return r.data.genres?.slice(0, 2).join(', ') || null;
      case 'tv':
        return [r.data.network, r.data.status].filter(Boolean).join(' · ') || null;
      case 'book':
        return r.data.authors?.join(', ') || null;
    }
  }

  async function handleAdd(r: DiscoverResult) {
    const id = getResultId(r);
    addingIds = new Set([...addingIds, id]);

    try {
      switch (r.kind) {
        case 'movie': {
          const { id: movieId } = await createMovie({
            title: r.data.title,
            year: r.data.year,
            description: r.data.description,
            poster_url: r.data.poster_url,
            identifiers: r.data.identifiers,
            genres: r.data.genres,
            rating: r.data.rating,
            rating_count: r.data.rating_count
          });
          goto(`/movies/${movieId}`);
          break;
        }
        case 'tv': {
          const { id: tvId } = await createTvShow({
            title: r.data.title,
            year: r.data.year,
            description: r.data.description,
            poster_url: r.data.poster_url,
            identifiers: r.data.identifiers,
            genres: r.data.genres,
            network: r.data.network
          });
          goto(`/tv/${tvId}`);
          break;
        }
        case 'book': {
          const { id: bookId } = await createBook({
            title: r.data.title,
            authors: r.data.authors?.map((a) => ({ name: a })),
            description: r.data.description,
            cover_url: r.data.cover_url,
            identifiers: r.data.identifiers ?? undefined,
            series_name: r.data.series,
            series_index: r.data.series_index,
            rating: r.data.rating,
            rating_count: r.data.rating_count,
            want: true
          });
          goto(`/library/books/${bookId}`);
          break;
        }
      }
    } catch (err) {
      console.error('Failed to add:', err);
      const next = new Set(addingIds);
      next.delete(id);
      addingIds = next;
    }
  }

  const hasQuery = $derived(!!urlQuery);
  let providerDropdownOpen = $state(false);

  function handleClickOutside(e: MouseEvent) {
    const target = e.target as HTMLElement;
    if (!target.closest('.provider-select')) {
      providerDropdownOpen = false;
    }
  }

  // Watch next progress helpers
  function progressPercent(item: WatchNextItem): number {
    if (item.progress_percent != null) return Math.min(100, item.progress_percent);
    if (
      item.position_seconds != null &&
      item.duration_seconds != null &&
      item.duration_seconds > 0
    ) {
      return Math.min(100, (item.position_seconds / item.duration_seconds) * 100);
    }
    return 0;
  }

  function progressLabel(item: WatchNextItem): string {
    if (item.position_seconds != null && item.duration_seconds != null) {
      return `${formatDuration(item.position_seconds)} / ${formatDuration(item.duration_seconds)}`;
    }
    if (item.progress_percent != null) {
      return `${Math.round(item.progress_percent)}%`;
    }
    return '';
  }

  function hasProgress(item: WatchNextItem): boolean {
    return item.progress_percent != null || item.position_seconds != null;
  }

  let dismissed = $state<Set<string>>(new Set());

  const forYouKey = (item: ForYouItem) => `${item.media_type}:${item.provider_id}`;

  function forYouTitle(item: ForYouItem) {
    const tmdbId = item.provider === 'tmdb' ? Number(item.provider_id) : NaN;
    if (!Number.isFinite(tmdbId) || (item.media_type !== 'movie' && item.media_type !== 'tv')) {
      return null;
    }
    return { media_type: item.media_type, tmdb_id: tmdbId } as const;
  }

  async function setDismissed(item: ForYouItem, hide: boolean) {
    const title = forYouTitle(item);
    if (!title) return;
    const next = new Set(dismissed);
    if (hide) next.add(forYouKey(item));
    else next.delete(forYouKey(item));
    dismissed = next;
    try {
      await (hide ? dismissForYou(title) : undismissForYou(title));
    } catch {
      const back = new Set(dismissed);
      if (hide) back.delete(forYouKey(item));
      else back.add(forYouKey(item));
      dismissed = back;
      showMessage(hide ? 'Could not hide that title' : 'Could not restore that title');
    }
  }

  function becauseLabel(because: string[]): string | null {
    if (because.length === 0) return null;
    const more = because.length > 1 ? ` +${because.length - 1}` : '';
    return `Because you watched ${because[0]}${more}`;
  }
</script>

<svelte:window onclick={handleClickOutside} />

{#snippet providerSelector()}
  {#if providers.length > 1}
    <div class="search-divider"></div>
    <div class="provider-select">
      <button
        class="provider-chip"
        class:active={selectedProvider.id !== '__all__'}
        type="button"
        onclick={(e) => {
          e.stopPropagation();
          providerDropdownOpen = !providerDropdownOpen;
        }}
      >
        {selectedProvider.displayName}
        <svg
          viewBox="0 0 24 24"
          width="12"
          height="12"
          fill="currentColor"
          class="provider-chevron"
          class:open={providerDropdownOpen}
        >
          <path d="M7 10l5 5 5-5z"></path>
        </svg>
      </button>
      {#if providerDropdownOpen}
        <div class="provider-dropdown">
          {#each providers as p (p.id)}
            <button
              class="provider-option"
              class:active={selectedProvider.id === p.id}
              type="button"
              onclick={(e) => {
                e.stopPropagation();
                selectProvider(p);
                providerDropdownOpen = false;
              }}
            >
              {#if p.id !== '__all__'}
                <span class="provider-dot" style="background: {getProviderColor(p.id)}"></span>
              {/if}
              <span class="provider-option-name">{p.displayName}</span>
              <span class="provider-option-types">{p.supportedTypes.join(', ')}</span>
            </button>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
{/snippet}

<svelte:head>
  <title>{hasQuery ? `Search: ${urlQuery}` : 'Home'} — Kobbler</title>
</svelte:head>

<div class="home-page">
  <!-- Compact search bar (always visible) -->
  <div class="home-search">
    <div class="search-bar">
      <svg viewBox="0 0 24 24" width="20" height="20" fill="currentColor" class="search-icon">
        <path
          d="M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z"
        ></path>
      </svg>
      <input
        bind:this={inputRef}
        type="text"
        class="search-input"
        placeholder={searchPlaceholder}
        value={searchInput}
        oninput={handleInput}
        onkeydown={handleKeydown}
        autocomplete="off"
        spellcheck="false"
      />
      <div class="type-filters">
        {#each filterOptions as f (f.value)}
          <button
            class="type-filter-btn"
            class:active={typeFilter === f.value}
            type="button"
            onclick={() => setTypeFilter(f.value)}
            title={f.label}
          >
            <svg viewBox="0 0 24 24" width="18" height="18" fill="currentColor">
              <path d={f.icon}></path>
            </svg>
          </button>
        {/each}
      </div>
      {@render providerSelector()}
      {#if loading}
        <div class="search-spinner"></div>
      {/if}
    </div>
  </div>

  {#if searched}
    <!-- Search results -->
    <div class="page">
      {#if loading}
        <div class="results-grid">
          {#each Array(8) as _, i (i)}
            <div class="skeleton-card">
              <div class="skeleton-poster"></div>
              <div class="skeleton-body">
                <div class="skeleton-line skeleton-title"></div>
                <div class="skeleton-line skeleton-sub"></div>
                <div class="skeleton-line skeleton-desc"></div>
                <div class="skeleton-line skeleton-desc short"></div>
              </div>
            </div>
          {/each}
        </div>
      {:else if filteredResults.length === 0}
        <div class="no-results">
          <p>No results for "{urlQuery}"</p>
          <span>Try a different search term or check your spelling</span>
        </div>
      {:else}
        <div class="results-grid">
          {#each filteredResults as result (getResultId(result))}
            {@const id = getResultId(result)}
            {@const isAdding = addingIds.has(id)}
            {@const libraryHref = resultLibraryHref(result)}
            {@const detailHref =
              libraryHref ??
              `/discover/${result.kind === 'book' ? 'book' : result.kind === 'tv' ? 'tv' : 'movie'}/${result.data.provider}/${result.data.external_id ?? ''}`}
            <a class="result-card" href={detailHref}>
              <div class="card-poster">
                {#if resultImage(result)}
                  <img src={resultImage(result)} alt={resultTitle(result)} loading="lazy" />
                {:else}
                  <div class="poster-fallback">
                    {#if result.kind === 'movie'}
                      <svg viewBox="0 0 24 24" width="28" height="28" fill="currentColor">
                        <path
                          d="M18 4l2 4h-3l-2-4h-2l2 4h-3l-2-4H8l2 4H7L5 4H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V4h-4z"
                        ></path>
                      </svg>
                    {:else if result.kind === 'tv'}
                      <svg viewBox="0 0 24 24" width="28" height="28" fill="currentColor">
                        <path
                          d="M21 3H3c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h5v2h8v-2h5c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 14H3V5h18v12z"
                        ></path>
                      </svg>
                    {:else}
                      <svg viewBox="0 0 24 24" width="28" height="28" fill="currentColor">
                        <path
                          d="M6 4h12a2 2 0 0 1 2 2v12h-2V6H6V4zm0 4h10a2 2 0 0 1 2 2v10H6a2 2 0 0 1-2-2V10a2 2 0 0 1 2-2zm0 2v8h10v-8H6z"
                        ></path>
                      </svg>
                    {/if}
                  </div>
                {/if}
                <div class="card-badges">
                  <IconChip chip={mediaTypeChip(result.kind)} />
                  <span
                    class="badge badge--provider"
                    style="background: {getProviderColor(result.data.provider)}"
                  >
                    {providerLabel(result.data.provider)}
                  </span>
                </div>
              </div>
              <div class="card-body">
                <h3 class="card-title">
                  {resultTitle(result)}
                  {#if resultYear(result)}
                    <span class="card-year">{resultYear(result)}</span>
                  {/if}
                </h3>
                {#if resultSubtitle(result)}
                  <p class="card-subtitle">{resultSubtitle(result)}</p>
                {/if}
                {#if resultDescription(result)}
                  <p class="card-description">{resultDescription(result)}</p>
                {/if}
                <div class="card-actions">
                  {#if libraryHref}
                    <span class="in-library">
                      <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor">
                        <path d="M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41L9 16.17z"></path>
                      </svg>
                      In library
                    </span>
                  {:else}
                    <button
                      class="add-button"
                      type="button"
                      disabled={isAdding}
                      onclick={(e) => {
                        e.preventDefault();
                        e.stopPropagation();
                        handleAdd(result);
                      }}
                    >
                      {#if isAdding}
                        <div class="add-spinner"></div>
                        Adding...
                      {:else}
                        <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor">
                          <path d="M19 13h-6v6h-2v-6H5v-2h6V5h2v6h6v2z"></path>
                        </svg>
                        Add
                      {/if}
                    </button>
                  {/if}
                </div>
              </div>
            </a>
          {/each}
        </div>
      {/if}
    </div>
  {:else}
    <!-- Library rows + discover sections -->
    <div class="content-rows">
      {#if watchNext.length > 0}
        <ScrollRow title="What to Watch">
          {#each watchNext as item (item.media_id + ':' + item.media_type)}
            <MediaCard
              posterUrl={item.image_url}
              placeholderIcon={item.media_type === 'movie' ? 'movie' : 'tv'}
              href={item.link}
            >
              {#snippet posterOverlay()}
                <span class="chip-pos-left">
                  <IconChip chip={mediaTypeChip(item.media_type)} />
                </span>
                {#if hasProgress(item)}
                  <div class="progress-track">
                    <div class="progress-fill" style="width: {progressPercent(item)}%"></div>
                  </div>
                {/if}
              {/snippet}
              <strong class="card-title">{item.title}</strong>
              <div class="card-sub">{item.subtitle ?? progressLabel(item)}</div>
            </MediaCard>
          {/each}
        </ScrollRow>
      {/if}

      {#if recentlyAdded.length > 0}
        <ScrollRow title="Recently Added">
          {#each recentlyAdded as item (item.id + ':' + item.item_type)}
            <MediaCard
              posterUrl={item.image_url}
              placeholderIcon={item.item_type === 'movie'
                ? 'movie'
                : item.item_type === 'episode'
                  ? 'tv'
                  : 'auto_stories'}
              href={item.link}
            >
              {#snippet posterOverlay()}
                <span class="chip-pos-left">
                  <IconChip chip={mediaTypeChip(item.item_type)} />
                </span>
              {/snippet}
              <strong class="card-title">{item.title}</strong>
              <div class="card-sub">{item.subtitle}</div>
            </MediaCard>
          {/each}
        </ScrollRow>
      {/if}

      {#if stremioContinue.length > 0}
        <ScrollRow title="Continue Watching on Stremio" viewAllHref="/settings/stremio-history">
          {#each stremioContinue as item (item.media_type + ':' + item.provider_id)}
            <DiscoverCard
              {item}
              subtitle={item.subtitle}
              progressPercent={item.progress_percent}
              season={item.season}
            />
          {/each}
        </ScrollRow>
      {/if}

      {#if forYou.length > 0}
        <ScrollRow title="For You">
          {#each forYou as item (forYouKey(item))}
            {#if dismissed.has(forYouKey(item))}
              <div class="media-card undo-tile">
                <span class="muted">{item.title} hidden</span>
                <button class="undo-btn" type="button" onclick={() => setDismissed(item, false)}>
                  Undo
                </button>
              </div>
            {:else}
              <DiscoverCard
                {item}
                subtitle={becauseLabel(item.because)}
                ondismiss={forYouTitle(item) ? () => setDismissed(item, true) : undefined}
              />
            {/if}
          {/each}
        </ScrollRow>
      {/if}

      {#if discoverLoading}
        <div class="discover-loading">
          <div class="discover-spinner"></div>
        </div>
      {:else if hasDiscoverContent && discover}
        <div class="discover-rows">
          {#each discover.sections as section (section.section_key)}
            <DiscoverRow title={section.title} items={section.items} />
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .home-page {
    padding-top: 8px;
  }

  /* ── Compact search bar ── */
  .home-search {
    padding: 8px 24px 16px;
  }

  .search-bar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 10px;
    transition:
      border-color 0.15s,
      box-shadow 0.15s;
  }

  .search-bar:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-dim);
  }

  .search-icon {
    flex-shrink: 0;
    color: var(--muted);
  }

  .search-input {
    flex: 1;
    background: transparent;
    border: none;
    color: var(--ink);
    font-size: 15px;
    outline: none;
    min-width: 0;
  }

  .search-input::placeholder {
    color: var(--muted);
  }

  .search-spinner {
    width: 18px;
    height: 18px;
    border: 2px solid var(--ring);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.6s linear infinite;
    flex-shrink: 0;
  }

  /* ── Content rows ── */
  .content-rows {
    padding-top: 4px;
  }

  /* ── Discover rows ── */
  .discover-rows {
    padding: 12px 0 16px;
  }

  .undo-tile {
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 12px;
    text-align: center;
    font-size: 12px;
    border: 1px dashed var(--ring);
    border-radius: 10px;
    aspect-ratio: 2 / 3;
  }

  .undo-btn {
    padding: 6px 14px;
    border-radius: 999px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    font-size: 12px;
    cursor: pointer;
  }

  .discover-loading {
    display: flex;
    justify-content: center;
    padding: 48px 0;
  }

  .discover-spinner {
    width: 24px;
    height: 24px;
    border: 2px solid var(--ring);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.6s linear infinite;
  }

  /* ── Provider selector ── */
  .provider-select {
    position: relative;
    flex-shrink: 0;
  }

  .provider-chip {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 4px 8px;
    border: none;
    border-radius: 14px;
    background: rgba(255, 255, 255, 0.06);
    color: var(--muted);
    font-size: 11px;
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
    transition:
      background 0.15s,
      color 0.15s;
  }

  :global([data-theme='light']) .provider-chip {
    background: rgba(0, 0, 0, 0.04);
  }

  .provider-chip:hover {
    background: rgba(255, 255, 255, 0.1);
    color: var(--ink);
  }

  :global([data-theme='light']) .provider-chip:hover {
    background: rgba(0, 0, 0, 0.08);
  }

  .provider-chip.active {
    background: var(--accent-dim);
    color: var(--accent);
  }

  .provider-chevron {
    flex-shrink: 0;
    opacity: 0.6;
    transition: transform 0.15s;
  }

  .provider-chevron.open {
    transform: rotate(180deg);
  }

  .provider-dropdown {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    min-width: 180px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 8px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.16);
    z-index: 50;
    padding: 4px;
    max-height: 320px;
    overflow-y: auto;
  }

  .provider-option {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 7px 10px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--ink);
    font-size: 12px;
    cursor: pointer;
    text-align: left;
    transition: background 0.1s;
  }

  .provider-option:hover {
    background: rgba(255, 255, 255, 0.06);
  }

  :global([data-theme='light']) .provider-option:hover {
    background: rgba(0, 0, 0, 0.04);
  }

  .provider-option.active {
    background: var(--accent-dim);
    color: var(--accent);
  }

  .provider-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .provider-option-name {
    flex: 1;
    font-weight: 500;
  }

  .provider-option-types {
    font-size: 10px;
    color: var(--muted);
    text-transform: capitalize;
  }

  .search-divider {
    width: 1px;
    height: 18px;
    background: var(--ring);
    flex-shrink: 0;
    opacity: 0.6;
  }

  /* ── Type filter icons (inside search bars) ── */
  .type-filters {
    display: flex;
    gap: 2px;
    margin-left: 8px;
    flex-shrink: 0;
  }

  .type-filter-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 30px;
    height: 30px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    padding: 0;
    transition:
      background 0.15s,
      color 0.15s;
  }

  .type-filter-btn:hover {
    background: rgba(255, 255, 255, 0.08);
    color: var(--ink);
  }

  :global([data-theme='light']) .type-filter-btn:hover {
    background: rgba(0, 0, 0, 0.06);
  }

  .type-filter-btn.active {
    background: var(--accent-dim);
    color: var(--accent);
  }

  /* ── Results page ── */
  .page {
    padding: 0 24px 24px;
  }

  /* ── Results grid ── */
  .results-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
    gap: 12px;
  }

  /* ── Result card ── */
  .result-card {
    display: flex;
    text-decoration: none;
    color: inherit;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 10px;
    overflow: hidden;
    transition:
      border-color 0.15s,
      box-shadow 0.15s;
  }

  .result-card:hover {
    border-color: color-mix(in srgb, var(--accent) 40%, transparent);
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.12);
  }

  :global([data-theme='light']) .result-card:hover {
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.06);
  }

  .card-poster {
    position: relative;
    width: 100px;
    min-height: 150px;
    flex-shrink: 0;
    background: var(--bg-2);
    overflow: hidden;
  }

  .card-poster img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }

  .poster-fallback {
    width: 100%;
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--muted);
    opacity: 0.25;
  }

  .card-badges {
    position: absolute;
    bottom: 5px;
    left: 5px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .badge {
    display: inline-block;
    padding: 2px 6px;
    border-radius: 4px;
    font-size: 9px;
    font-weight: 600;
    letter-spacing: 0.3px;
    text-transform: uppercase;
    line-height: 1.4;
    backdrop-filter: blur(8px);
  }

  .badge--provider {
    color: #fff;
  }

  .card-body {
    flex: 1;
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }

  .card-title {
    font-size: 14px;
    font-weight: 600;
    margin: 0;
    color: var(--ink);
    line-height: 1.3;
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    overflow: hidden;
  }

  .card-year {
    font-weight: 400;
    color: var(--muted);
    font-size: 13px;
    margin-left: 4px;
  }

  .card-subtitle {
    font-size: 12px;
    color: var(--muted);
    margin: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .card-description {
    font-size: 12px;
    color: var(--muted);
    margin: 2px 0 0;
    line-height: 1.45;
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    overflow: hidden;
    opacity: 0.75;
  }

  .card-actions {
    display: flex;
    gap: 6px;
    margin-top: auto;
    align-self: flex-start;
  }

  .add-button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    border: 1px solid var(--accent);
    border-radius: 7px;
    background: transparent;
    color: var(--accent);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition:
      background 0.15s,
      color 0.15s;
  }

  .in-library {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    border: 1px solid var(--success, #22c55e);
    border-radius: 7px;
    color: var(--success, #22c55e);
    font-size: 12px;
    font-weight: 600;
  }

  .add-button:hover:not(:disabled) {
    background: var(--accent);
    color: #fff;
  }

  .add-button:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .add-spinner {
    width: 12px;
    height: 12px;
    border: 2px solid var(--ring);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.6s linear infinite;
  }

  /* ── No results ── */
  .no-results {
    text-align: center;
    padding: 60px 20px;
    color: var(--muted);
  }

  .no-results p {
    font-size: 15px;
    font-weight: 500;
    margin: 0;
    color: var(--ink);
  }

  .no-results span {
    font-size: 13px;
    display: block;
    margin-top: 6px;
  }

  /* ── Skeleton loading ── */
  .skeleton-card {
    display: flex;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 10px;
    overflow: hidden;
  }

  .skeleton-poster {
    width: 100px;
    min-height: 150px;
    background: linear-gradient(
      110deg,
      var(--bg-2) 30%,
      color-mix(in srgb, var(--ring) 50%, transparent) 50%,
      var(--bg-2) 70%
    );
    background-size: 200% 100%;
    animation: shimmer 1.5s ease-in-out infinite;
  }

  .skeleton-body {
    flex: 1;
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .skeleton-line {
    height: 12px;
    border-radius: 6px;
    background: linear-gradient(
      110deg,
      var(--bg-2) 30%,
      color-mix(in srgb, var(--ring) 50%, transparent) 50%,
      var(--bg-2) 70%
    );
    background-size: 200% 100%;
    animation: shimmer 1.5s ease-in-out infinite;
  }

  .skeleton-title {
    width: 70%;
    height: 14px;
  }

  .skeleton-sub {
    width: 50%;
  }

  .skeleton-desc {
    width: 90%;
  }

  .skeleton-desc.short {
    width: 60%;
  }

  @keyframes shimmer {
    0% {
      background-position: 200% 0;
    }
    100% {
      background-position: -200% 0;
    }
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  /* ── Responsive ── */
  @media (max-width: 640px) {
    .home-search {
      padding: 8px 16px 12px;
    }

    .search-bar {
      padding: 10px 14px;
    }

    .search-input {
      font-size: 14px;
    }

    .page {
      padding: 0 16px 16px;
    }

    .results-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
