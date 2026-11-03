<script lang="ts">
  import { onMount, onDestroy, untrack } from 'svelte';
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
  import TvShowsPage from '#lib/components/pages/TvShowsPage.svelte';
  import VideoSearchModal from '#lib/components/movies/VideoSearchModal.svelte';
  import TvShowDetailsPanel from '#lib/components/tv/TvShowDetailsPanel.svelte';
  import TvShowForm from '#lib/components/tv/TvShowForm.svelte';
  import type { TvShowFormData } from '#lib/components/tv/TvShowForm.svelte';
  import VideoMetadataSearchModal from '#lib/components/video/VideoMetadataSearchModal.svelte';
  import type { TvSearchResult, MovieSearchResult } from '#lib/types.js';
  import SeasonDetailsPanel from '#lib/components/tv/SeasonDetailsPanel.svelte';
  import SeasonForm from '#lib/components/tv/SeasonForm.svelte';
  import type { SeasonFormData } from '#lib/components/tv/SeasonForm.svelte';
  import EpisodeDetailsPanel from '#lib/components/tv/EpisodeDetailsPanel.svelte';
  import EpisodeForm from '#lib/components/tv/EpisodeForm.svelte';
  import type { EpisodeFormData } from '#lib/components/tv/EpisodeForm.svelte';
  import PersonDetailsPanel from '#lib/components/people/PersonDetailsPanel.svelte';
  import ReleaseSearchModal from '#lib/components/ReleaseSearchModal.svelte';
  import { ExpandableSidebar } from '@koblibs/ui';
  import ErrorBanner from '#lib/components/shared/ErrorBanner.svelte';
  import MoveFileModal from '#lib/components/shared/MoveFileModal.svelte';
  import type { SearchSelectItem } from '@koblibs/ui';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { getStatsState } from '#lib/stores/stats.svelte.js';
  import * as api from '#lib/api/client.js';
  import { formatEpisodeSearchTitle } from '#lib/utils/format.js';
  import { unwantMessage } from '#lib/utils/want-status.js';
  import type { TvShow, TvSeason, TvEpisode, PersonDetail } from '#lib/types.js';
  import {
    pushEntity,
    popEntity,
    clearStack,
    getNavigationState,
    updateCurrentEntityData,
    setLoadingEntity
  } from '#lib/stores/navigation.svelte.js';

  import type { FilterValue, SortValue } from '#lib/components/pages/TvShowsPage.svelte';

  let shows = $state<TvShow[]>([]);
  let loading = $state(true);
  let error = $state('');
  const VALID_FILTERS: FilterValue[] = ['all', 'monitored', 'returning', 'ended'];
  const VALID_SORTS: SortValue[] = ['title', 'next_airing', 'created_at'];

  const filter = $derived(
    VALID_FILTERS.includes(page.url.searchParams.get('filter') as FilterValue)
      ? (page.url.searchParams.get('filter') as FilterValue)
      : 'all'
  );

  const sort = $derived(
    VALID_SORTS.includes(page.url.searchParams.get('sort') as SortValue)
      ? (page.url.searchParams.get('sort') as SortValue)
      : 'title'
  );

  const searchQuery = $derived(page.url.searchParams.get('q') || '');
  const urlAround = $derived(page.url.searchParams.get('around'));
  const urlAroundBefore = $derived(page.url.searchParams.get('around_before'));
  let showAddModal = $state(false);
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let availableLetters = $state<string[]>([]);
  let editMode = $state(false);
  let addingEpisode = $state(false);
  let formRequestSave: (() => void) | undefined = $state();
  let formCanSave: boolean | undefined = $state();
  let showMetadataSearch = $state(false);
  let releaseSearchEpisodeId = $state<string | null>(null);
  let editSearchResults = $state<(TvSearchResult | MovieSearchResult)[]>([]);
  let nextCursor = $state<string | null>(null);
  let prevCursor = $state<string | null>(null);
  let totalCount = $state<number | null>(null);
  let movingFileId = $state<string | null>(null);
  let siblingEpisodes = $state<SearchSelectItem[]>([]);
  let loadGeneration = 0;

  const nav = getNavigationState();
  const statsState = getStatsState();

  const releaseSearchTitle = $derived.by(() => {
    if (!releaseSearchEpisodeId) return 'Episode';
    const show = nav.stack.findLast((item) => item.type === 'tv_show')?.data as TvShow | undefined;
    const seasons: TvSeason[] = [
      ...(show?.seasons ?? []),
      ...nav.stack
        .filter((item) => item.type === 'season' && item.data)
        .map((item) => item.data as TvSeason)
    ];
    for (const season of seasons) {
      const episode = season.episodes?.find((e) => e.id === releaseSearchEpisodeId);
      if (episode) {
        return formatEpisodeSearchTitle(
          show?.title,
          season.season_identifier,
          episode.episode_identifier
        );
      }
    }
    return 'Episode';
  });

  let reconnectHandler: (() => void) | null = null;
  let tvShowHandler: ((data: unknown) => void) | null = null;

  async function loadShows(options?: {
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
      if (searchQuery.trim()) params.search = searchQuery.trim();
      if (filter === 'monitored') params.is_monitored = true;
      else if (filter === 'returning') params.status = 'continuing';
      else if (filter === 'ended') params.status = 'ended';
      if (sort !== 'title') params.sort = sort;
      if (options?.cursor) params.cursor = options.cursor;
      if (options?.direction) params.direction = options.direction;
      if (options?.around) params.around = options.around;
      if (options?.around_before != null) params.around_before = options.around_before;
      if (options?.limit != null) params.limit = options.limit;

      const result = await api.getTvShows(params);
      if (gen !== loadGeneration) return; // stale response, discard
      const mode = options?.mode ?? 'replace';

      if (mode === 'append') {
        const existingIds = new Set(shows.map((s) => s.id));
        const uniqueNew = result.items.filter((s) => !existingIds.has(s.id));
        shows = [...shows, ...uniqueNew];
        nextCursor = result.next_cursor;
      } else if (mode === 'prepend') {
        const existingIds = new Set(shows.map((s) => s.id));
        const uniqueNew = result.items.filter((s) => !existingIds.has(s.id));
        shows = [...uniqueNew, ...shows];
        prevCursor = result.prev_cursor ?? null;
      } else {
        shows = result.items;
        nextCursor = result.next_cursor;
        prevCursor = result.prev_cursor ?? null;
        if (result.total_count != null) totalCount = result.total_count;
      }
      error = '';
    } catch (e) {
      if (gen !== loadGeneration) return; // stale error, discard
      if (options?.around) {
        // The anchor row can vanish between render and request (it was just deleted).
        // Retry without it rather than blanking the list.
        return await loadShows({ ...options, around: undefined, around_before: undefined });
      }
      showMessage(e instanceof Error ? e.message : 'Failed to load TV shows');
      shows = [];
      error = 'Failed to load data. Please check your connection and try again.';
    } finally {
      if (gen === loadGeneration) loading = false;
    }
  }

  /**
   * Reload the current view in-place, keeping the reader where they are.
   *
   * The window has to be re-requested at its current size and centred on the first
   * *visible* show: a default-limit reload centred on shows[0] throws away every page
   * that was scrolled into view, the grid collapses to 50 cards, and the browser clamps
   * scrollTop to the new (much shorter) content — which is why a background update used
   * to fling the page somewhere random.
   */
  function reloadInPlace(): Promise<void> {
    // `around` windows are always title-ordered server-side (cursor pagination only
    // exists for that sort), so anchoring under any other sort would silently re-sort
    // the page out from under the reader. Those sorts are a single page anyway.
    if (shows.length === 0 || sort !== 'title') {
      return loadShows();
    }
    const anchor = captureScrollAnchor();
    const anchorIndex = anchor ? shows.findIndex((s) => s.id === anchor.id) : -1;
    const index = anchorIndex >= 0 ? anchorIndex : 0;
    const limit = Math.min(Math.max(shows.length, 50), 200);
    const before = Math.min(index, Math.floor(limit / 2));
    return loadShows({ around: shows[index].id, around_before: before, limit }).then(() =>
      restoreScrollAnchor(anchor)
    );
  }

  async function loadAvailableLetters() {
    try {
      availableLetters = await api.getTvShowAvailableLetters();
    } catch (e) {
      console.error('Failed to load available letters:', e);
      availableLetters = [];
    }
  }

  // Reload when URL-derived filter/sort/search changes
  $effect(() => {
    // Read these to establish dependencies
    filter;
    sort;
    searchQuery;
    // Skip if there's an around param — the around effect handles the load
    if (!untrack(() => urlAround)) {
      // A new filter/sort/search is a new list: start it at the top, since the scroll
      // container keeps its offset across the swap.
      loadShows().then(scrollContainerToTop);
    }
  });

  // Handle around parameter changes (letter jump navigation)
  let lastAround = $state<string | null>(null);
  $effect(() => {
    const around = urlAround;
    if (around && around !== lastAround) {
      lastAround = around;
      const ab = urlAroundBefore ? parseInt(urlAroundBefore, 10) : 0;
      loadShows({ around, around_before: ab }).then(() => {
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

  onMount(() => {
    loadAvailableLetters();

    reconnectHandler = onReconnect(() => {
      reloadInPlace();
      loadAvailableLetters();
    });

    tvShowHandler = onDebounced('tv_show_updated', () => {
      reloadInPlace();
      loadAvailableLetters();
    });

    return () => clearTimeout(searchTimer);
  });

  onDestroy(() => {
    if (reconnectHandler) offReconnect(reconnectHandler);
    if (tvShowHandler) off('tv_show_updated', tvShowHandler);
  });

  async function handleJumpToLetter(letter: string | null) {
    if (!letter) {
      // Back to the start of the list. A goto() here is usually a no-op (the URL is
      // already bare by the time the previous jump cleared `around`), so reload directly.
      updateUrlParams({ around: null, around_before: null });
      await loadShows();
      scrollContainerToTop();
      return;
    }

    loading = true;
    try {
      const result = await api.getTvShowFirstByLetter(letter);
      if (result) {
        const params = new URLSearchParams();
        params.set('around', result.id);
        params.set('around_before', '0');
        if (filter !== 'all') params.set('filter', filter);
        if (sort !== 'title') params.set('sort', sort);
        goto(`/tv?${params.toString()}`);
      } else {
        showMessage(`No shows starting with "${letter}"`);
        loading = false;
      }
    } catch {
      showMessage(`Failed to find shows for "${letter}"`);
      loading = false;
    }
  }

  function loadMoreShows() {
    if (!nextCursor || loading) return;
    loadShows({ cursor: nextCursor, direction: 'forward', mode: 'append' });
  }

  function loadPreviousShows() {
    if (!prevCursor || loading) return;
    loadShows({ cursor: prevCursor, direction: 'backward', mode: 'prepend' });
  }

  function handleFilterChange(newFilter: FilterValue) {
    updateUrlParams({ filter: newFilter === 'all' ? null : newFilter, q: null });
  }

  function handleSortChange(newSort: SortValue) {
    updateUrlParams({ sort: newSort === 'title' ? null : newSort });
  }

  async function handleBulkUpdate(
    ids: string[],
    updates: { auto_want_mode?: string; quality_profile_id?: string | null }
  ) {
    try {
      await api.bulkUpdateTvShows(ids, updates);
      showMessage(`Updated ${ids.length} shows`);
      await reloadInPlace();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to bulk update shows');
    }
  }

  function handleSearch(query: string) {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      updateUrlParams({ q: query || null });
    }, 300);
  }

  function handleSelectShow(show: TvShow) {
    if (window.matchMedia('(max-width: 768px)').matches) {
      goto(`/tv/${show.id}`);
    } else {
      pushEntity('tv_show', show.id);
    }
  }

  async function handleAddComplete(id: string) {
    showAddModal = false;
    await loadShows();
    try {
      const show = await api.getTvShow(id);
      pushEntity('tv_show', id, show);
    } catch {
      // Show was added, just refresh the list
    }
  }

  function handleCloseSidebar() {
    if (addingEpisode) {
      addingEpisode = false;
      return;
    }
    if (editMode) {
      editMode = false;
    } else {
      clearStack();
    }
  }

  function handleExpandShow() {
    const entity = nav.current;
    if (entity?.type === 'tv_show') {
      const wasEditing = editMode;
      editMode = false;
      clearStack();
      goto(`/tv/${entity.id}`, wasEditing ? { state: { editing: true } } : undefined);
    }
  }

  function handleBack() {
    editMode = false;
    popEntity();
  }

  function handleToggleEdit() {
    editMode = !editMode;
    if (!editMode) {
      editSearchResults = [];
      showMetadataSearch = false;
    }
  }

  function handleEditCancel() {
    if (addingEpisode) {
      addingEpisode = false;
      return;
    }
    editMode = false;
    addingEpisode = false;
    editSearchResults = [];
    showMetadataSearch = false;
  }

  async function handleDeleteShow() {
    const entity = nav.current;
    if (entity?.type !== 'tv_show') return;
    if (!confirm('Delete this show? This cannot be undone.')) return;
    try {
      await api.deleteTvShow(entity.id);
      showMessage('Show deleted');
      clearStack();
      await reloadInPlace();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete show');
    }
  }

  async function handleShowEditSave(data: TvShowFormData) {
    const entity = nav.current;
    if (entity?.type !== 'tv_show' || !entity.data) return;
    loading = true;
    try {
      const { cast, crew, ...metadata } = data;
      await api.updateTvShow(entity.id, metadata as unknown as Record<string, unknown>);
      if (cast) await api.replaceTvShowCast(entity.id, cast);
      if (crew) await api.replaceTvShowCrew(entity.id, crew);
      showMessage('Show updated');
      editMode = false;
      const show = await api.getTvShow(entity.id);
      updateCurrentEntityData(show);
      await reloadInPlace();
    } catch {
      showMessage('Failed to update show');
    } finally {
      loading = false;
    }
  }

  async function handleSeasonEditSave(data: SeasonFormData, posterFile?: File) {
    const entity = nav.current;
    if (entity?.type !== 'season' || !entity.data) return;
    loading = true;
    try {
      await api.updateSeason(entity.id, data as unknown as Record<string, unknown>);
      if (posterFile) {
        await api.uploadSeasonPoster(entity.id, posterFile);
      }
      showMessage('Season updated');
      editMode = false;
      const season = await api.getSeason(entity.id);
      updateCurrentEntityData(season);
    } catch {
      showMessage('Failed to update season');
    } finally {
      loading = false;
    }
  }

  async function handleEpisodeEditSave(data: EpisodeFormData) {
    const entity = nav.current;
    if (entity?.type !== 'episode' || !entity.data) return;
    loading = true;
    try {
      await api.updateEpisode(entity.id, data as unknown as Record<string, unknown>);
      showMessage('Episode updated');
      editMode = false;
      const episode = await api.getEpisode(entity.id);
      updateCurrentEntityData(episode);
    } catch {
      showMessage('Failed to update episode');
    } finally {
      loading = false;
    }
  }

  // Load entity data when nav stack changes
  $effect(() => {
    const entity = nav.current;
    if (!entity) return;
    if (entity.data) return;

    if (entity.type === 'tv_show') {
      loadNavShowData(entity.id);
    } else if (entity.type === 'season') {
      loadNavSeasonData(entity.id);
    } else if (entity.type === 'episode') {
      loadNavEpisodeData(entity.id);
    } else if (entity.type === 'person') {
      loadNavPersonData(entity.id);
    }
  });

  async function loadNavShowData(showId: string) {
    setLoadingEntity(true);
    try {
      const show = await api.getTvShow(showId);
      updateCurrentEntityData(show);
    } catch (e) {
      console.error('Failed to load show:', e);
    } finally {
      setLoadingEntity(false);
    }
  }

  async function loadNavSeasonData(seasonId: string) {
    setLoadingEntity(true);
    try {
      const season = await api.getSeason(seasonId);
      updateCurrentEntityData(season);
    } catch (e) {
      console.error('Failed to load season:', e);
    } finally {
      setLoadingEntity(false);
    }
  }

  async function loadNavEpisodeData(episodeId: string) {
    setLoadingEntity(true);
    try {
      const episode = await api.getEpisode(episodeId);
      updateCurrentEntityData(episode);
    } catch (e) {
      console.error('Failed to load episode:', e);
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

  async function handleMonitorToggle(monitored: boolean, mode?: string) {
    const entity = nav.current;
    if (entity?.type !== 'tv_show' || !entity.data) return;
    const show = entity.data as TvShow;
    try {
      await api.setTvShowMonitored(entity.id, monitored, mode ?? show.auto_want_mode ?? 'none');
      showMessage(monitored ? 'Show monitored' : 'Show unmonitored');
      const updated = await api.getTvShow(entity.id);
      updateCurrentEntityData(updated);
      await reloadInPlace();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update monitor');
    }
  }

  async function handleAutoWantChange(mode: string) {
    const entity = nav.current;
    if (entity?.type !== 'tv_show' || !entity.data) return;
    const show = entity.data as TvShow;
    try {
      // Auto-enable monitoring if switching to non-'none' while unmonitored
      const monitored = mode !== 'none' ? true : show.is_monitored;
      await api.setTvShowMonitored(entity.id, monitored, mode);
      if (mode !== 'none' && !show.is_monitored) {
        showMessage('Show monitored with auto-want');
      }
      const updated = await api.getTvShow(entity.id);
      updateCurrentEntityData(updated);
      await reloadInPlace();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update auto-want');
    }
  }

  async function handleWantAllMissing() {
    const entity = nav.current;
    if (entity?.type !== 'tv_show') return;
    try {
      await api.wantAllMissing(entity.id);
      showMessage('Wanting all missing episodes');
      const updated = await api.getTvShow(entity.id);
      updateCurrentEntityData(updated);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want missing episodes');
    }
  }

  async function handleUnwantAll() {
    const entity = nav.current;
    if (entity?.type !== 'tv_show') return;
    const show = entity.data as TvShow | undefined;
    if (
      !confirm(
        `Unwant every outstanding episode of "${show?.title ?? 'this show'}"? Episodes already in your library are unaffected.`
      )
    )
      return;
    try {
      const result = await api.unwantAll(entity.id);
      showMessage(unwantMessage(result, 'show'));
      const updated = await api.getTvShow(entity.id);
      updateCurrentEntityData(updated);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant episodes');
    }
  }

  async function handleBulkShowWatch(watched: boolean) {
    const entity = nav.current;
    if (entity?.type !== 'tv_show') return;
    try {
      const result = await api.bulkUpdateShowWatchState(entity.id, watched);
      showMessage(
        `Marked ${result.updated_count} episode${result.updated_count === 1 ? '' : 's'} as ${watched ? 'watched' : 'unwatched'}`
      );
      const updated = await api.getTvShow(entity.id);
      updateCurrentEntityData(updated);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update watch state');
    }
  }

  async function handleBulkSeasonWatch(watched: boolean) {
    const entity = nav.current;
    if (entity?.type !== 'season') return;
    const season = entity.data as TvSeason;
    try {
      const result = await api.bulkUpdateSeasonWatchState(entity.id, watched);
      showMessage(
        `Marked ${result.updated_count} episode${result.updated_count === 1 ? '' : 's'} as ${watched ? 'watched' : 'unwatched'}`
      );
      const updated = await api.getSeason(entity.id);
      updateCurrentEntityData(updated);
      // Also refresh the parent show data if it's in the nav stack
      if (season.show_id) {
        reloadInPlace();
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update watch state');
    }
  }

  async function handleRefreshMetadata() {
    const entity = nav.current;
    if (entity?.type !== 'tv_show') return;
    try {
      await api.refreshTvShowMetadata(entity.id);
      showMessage('Metadata refresh queued');
      setTimeout(async () => {
        const show = await api.getTvShow(entity.id);
        updateCurrentEntityData(show);
      }, 1500);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to refresh metadata');
    }
  }

  function handleSeasonClick(seasonId: string) {
    pushEntity('season', seasonId);
  }

  function handleEpisodeClick(episodeId: string) {
    pushEntity('episode', episodeId);
  }

  async function handleWantSeason() {
    const entity = nav.current;
    if (entity?.type !== 'season') return;
    try {
      await api.wantSeason(entity.id);
      showMessage('Season episodes wanted');
      const season = await api.getSeason(entity.id);
      updateCurrentEntityData(season);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want season');
    }
  }

  async function handleUnwantSeason() {
    const entity = nav.current;
    if (entity?.type !== 'season') return;
    const season = entity.data as TvSeason | undefined;
    const label = season?.is_special
      ? 'Specials'
      : `Season ${season?.season_identifier ?? ''}`.trim();
    if (
      !confirm(
        `Unwant every outstanding episode in ${label}? Episodes already in your library are unaffected.`
      )
    )
      return;
    try {
      const result = await api.unwantSeason(entity.id);
      showMessage(unwantMessage(result, 'season'));
      const updated = await api.getSeason(entity.id);
      updateCurrentEntityData(updated);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant season');
    }
  }

  async function handleSeasonAutoWantChange(mode: string) {
    const entity = nav.current;
    if (entity?.type !== 'season') return;
    try {
      await api.setSeasonAutoWant(entity.id, mode);
      showMessage('Season auto-want updated');
      const season = await api.getSeason(entity.id);
      updateCurrentEntityData(season);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update season auto-want');
    }
  }

  async function handleWantEpisode() {
    const entity = nav.current;
    if (entity?.type !== 'episode' || !entity.data) return;
    try {
      await api.wantEpisode(entity.id);
      showMessage('Episode added to wanted');
      const episode = await api.getEpisode(entity.id);
      updateCurrentEntityData(episode);
      // Check if streaming is available
      api
        .streamSearch({ episode_id: entity.id })
        .then((result) => {
          if (result.options.length > 0) {
            showMessage('This is also available for streaming!');
          }
        })
        .catch(() => {
          /* non-critical */
        });
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want episode');
    }
  }

  async function handleUnwantEpisode() {
    const entity = nav.current;
    if (entity?.type !== 'episode' || !entity.data) return;
    try {
      await api.unwantEpisode(entity.id);
      showMessage('Episode removed from wanted');
      const episode = await api.getEpisode(entity.id);
      updateCurrentEntityData(episode);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant episode');
    }
  }

  function handleAddEpisode() {
    addingEpisode = true;
  }

  async function handleAddEpisodeSave(data: EpisodeFormData) {
    const entity = nav.current;
    if (entity?.type !== 'season') return;
    loading = true;
    try {
      await api.createEpisode(entity.id, data as Parameters<typeof api.createEpisode>[1]);
      showMessage('Episode created');
      addingEpisode = false;
      const season = await api.getSeason(entity.id);
      updateCurrentEntityData(season);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to create episode');
    } finally {
      loading = false;
    }
  }

  async function handleMoveFile(fileId: string) {
    const entity = nav.current;
    if (entity?.type !== 'episode' || !entity.data) return;
    const episode = entity.data as TvEpisode;
    try {
      const season = await api.getSeason(episode.season_id);
      siblingEpisodes = season.episodes
        .filter((ep) => ep.id !== episode.id)
        .map((ep) => ({
          value: ep.id,
          label: ep.title || `Episode ${ep.episode_sort_order}`
        }));
      movingFileId = fileId;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load season episodes');
    }
  }

  async function doMoveFile(targetEpisodeId: string) {
    const entity = nav.current;
    if (entity?.type !== 'episode' || !entity.data || !movingFileId) return;
    await api.moveEpisodeFile(entity.id, movingFileId, targetEpisodeId);
    movingFileId = null;
    siblingEpisodes = [];
    const episode = await api.getEpisode(entity.id);
    updateCurrentEntityData(episode);
  }

  function handleSearchEpisodeReleases(episodeId?: string) {
    const epId = episodeId ?? (nav.current?.type === 'episode' ? nav.current.id : null);
    if (epId) {
      releaseSearchEpisodeId = epId;
    }
  }

  function handleCastClick(personId: string, _personName: string) {
    pushEntity('person', personId);
  }

  function handleGenreClick(genre: string) {
    handleSearch(genre);
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

  const showEditButton = $derived(
    (nav.current?.type === 'tv_show' ||
      nav.current?.type === 'episode' ||
      nav.current?.type === 'season') &&
      !editMode
  );

  const sidebarTitle = $derived.by(() => {
    if (editMode) {
      if (nav.current?.type === 'tv_show') return 'Edit Show';
      if (nav.current?.type === 'season') return addingEpisode ? 'Add Episode' : 'Edit Season';
      if (nav.current?.type === 'episode') return 'Edit Episode';
    }
    const entity = nav.current;
    if (!entity) return 'Details';
    if (entity.type === 'tv_show' && entity.data) return (entity.data as TvShow).title;
    if (entity.type === 'season' && entity.data) {
      const s = entity.data as TvSeason;
      return s.title || `Season ${s.season_sort_order}`;
    }
    if (entity.type === 'episode' && entity.data) {
      const ep = entity.data as TvEpisode;
      return ep.title || `Episode ${ep.episode_sort_order}`;
    }
    if (entity.type === 'person' && entity.data) return (entity.data as PersonDetail).name;
    return 'Details';
  });
</script>

<svelte:head>
  <title>TV Shows — Kobbler</title>
</svelte:head>

{#if error}
  <div class="error-wrapper">
    <ErrorBanner message={error} onRetry={() => loadShows()} onDismiss={() => (error = '')} />
  </div>
{/if}

<TvShowsPage
  {shows}
  {loading}
  {filter}
  {sort}
  {searchQuery}
  {availableLetters}
  {totalCount}
  hasMore={nextCursor !== null}
  hasPrevious={prevCursor !== null}
  onFilterChange={handleFilterChange}
  onSortChange={handleSortChange}
  onSearch={handleSearch}
  onSelectShow={handleSelectShow}
  onBulkUpdate={handleBulkUpdate}
  onJumpToLetter={handleJumpToLetter}
  onLoadMore={loadMoreShows}
  onLoadPrevious={loadPreviousShows}
  onAdd={() => (showAddModal = true)}
/>

<ExpandableSidebar
  open={nav.isOpen}
  expanded={editMode}
  title={sidebarTitle}
  showEdit={showEditButton}
  showRefresh={nav.current?.type === 'tv_show' && !editMode}
  {editMode}
  onClose={handleCloseSidebar}
  onExpand={nav.current?.type === 'tv_show' ? handleExpandShow : undefined}
  onToggleEdit={handleToggleEdit}
  onRefresh={handleRefreshMetadata}
  onSearch={editMode && nav.current?.type === 'tv_show'
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
    {#if editMode && (nav.current?.type === 'tv_show' || nav.current?.type === 'season' || nav.current?.type === 'episode') && nav.current.data}
      <div class="sidebar-footer-actions">
        {#if nav.current?.type === 'tv_show'}
          <button class="btn-danger-sm" type="button" onclick={handleDeleteShow} disabled={loading}>
            <span class="material-icons-outlined">delete</span>
            Delete
          </button>
          <div class="spacer"></div>
        {/if}

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
  {#if nav.current?.type === 'tv_show' && nav.current.data}
    {#if editMode}
      {#key nav.current.id}
        <TvShowForm
          show={nav.current.data as TvShow}
          searchResults={editSearchResults}
          onSave={handleShowEditSave}
          onCancel={handleEditCancel}
          saving={loading}
          hideFooter
          bind:requestSave={formRequestSave}
          bind:canSave={formCanSave}
        />
      {/key}
    {:else}
      <TvShowDetailsPanel
        show={nav.current.data as TvShow}
        onMonitorToggle={handleMonitorToggle}
        onAutoWantChange={handleAutoWantChange}
        onWantAllMissing={handleWantAllMissing}
        onUnwantAll={handleUnwantAll}
        onBulkWatchToggle={handleBulkShowWatch}
        onSeasonClick={handleSeasonClick}
        onCastClick={handleCastClick}
        onGenreClick={handleGenreClick}
      />
    {/if}
  {:else if nav.current?.type === 'season' && nav.current.data}
    {#if editMode && addingEpisode}
      <EpisodeForm
        createMode
        onSave={handleAddEpisodeSave}
        onCancel={() => (addingEpisode = false)}
        saving={loading}
        hideFooter
        bind:requestSave={formRequestSave}
        bind:canSave={formCanSave}
      />
    {:else if editMode}
      {#key nav.current.id}
        <SeasonForm
          season={nav.current.data as TvSeason}
          onSave={handleSeasonEditSave}
          onCancel={handleEditCancel}
          saving={loading}
          hideFooter
          bind:requestSave={formRequestSave}
          bind:canSave={formCanSave}
        />
      {/key}
      <div class="sidebar-add-episode">
        <button class="btn-sec" type="button" onclick={handleAddEpisode}>
          <span class="material-icons-outlined">add</span>
          Add Episode
        </button>
      </div>
    {:else}
      <SeasonDetailsPanel
        season={nav.current.data as TvSeason}
        onEpisodeClick={handleEpisodeClick}
        onWantSeason={handleWantSeason}
        onUnwantSeason={handleUnwantSeason}
        onBulkWatchToggle={handleBulkSeasonWatch}
        onAutoWantChange={handleSeasonAutoWantChange}
        onEdit={handleToggleEdit}
        onDelete={async () => {
          const season = nav.current?.data as TvSeason;
          if (!season || !confirm('Delete this season? This cannot be undone.')) return;
          try {
            await api.deleteSeason(season.id);
            showMessage('Season deleted');
            popEntity();
            reloadInPlace();
          } catch (e) {
            showMessage(e instanceof Error ? e.message : 'Failed to delete season');
          }
        }}
      />
    {/if}
  {:else if nav.current?.type === 'episode' && nav.current.data}
    {#if editMode}
      {#key nav.current.id}
        <EpisodeForm
          episode={nav.current.data as TvEpisode}
          onSave={handleEpisodeEditSave}
          onCancel={handleEditCancel}
          saving={loading}
          hideFooter
          bind:requestSave={formRequestSave}
          bind:canSave={formCanSave}
        />
      {/key}
    {:else}
      <EpisodeDetailsPanel
        episode={nav.current.data as TvEpisode}
        onWant={handleWantEpisode}
        onUnwant={handleUnwantEpisode}
        onSearchReleases={statsState.stats.kobload_configured
          ? () => handleSearchEpisodeReleases()
          : undefined}
        onMoveFile={handleMoveFile}
        onEdit={handleToggleEdit}
        onDelete={async () => {
          const episode = nav.current?.data as TvEpisode;
          if (!episode || !confirm('Delete this episode? This cannot be undone.')) return;
          try {
            await api.deleteEpisode(episode.id);
            showMessage('Episode deleted');
            popEntity();
            reloadInPlace();
          } catch (e) {
            showMessage(e instanceof Error ? e.message : 'Failed to delete episode');
          }
        }}
      />
    {/if}
  {:else if nav.current?.type === 'person' && nav.current.data}
    <PersonDetailsPanel
      person={nav.current.data as PersonDetail}
      onEdit={handlePersonEdit}
      onDelete={handlePersonDelete}
      onViewFullPage={() => goto(`/people/${(nav.current!.data as PersonDetail).id}`)}
      onCreditClick={(libraryId, mediaType) => {
        if (mediaType === 'movie') goto(`/movies/${libraryId}`);
        else pushEntity('tv_show', libraryId);
      }}
    />
  {:else if nav.isOpen}
    <div class="loading-sidebar">Loading...</div>
  {/if}
</ExpandableSidebar>

{#if showAddModal}
  <VideoSearchModal
    mode="tv"
    onClose={() => (showAddModal = false)}
    onComplete={handleAddComplete}
  />
{/if}

{#if showMetadataSearch}
  <VideoMetadataSearchModal
    mode="tv"
    initialTitle={(nav.current?.data as TvShow)?.title ?? ''}
    onSelect={(results) => {
      editSearchResults = results;
      showMetadataSearch = false;
    }}
    onCancel={() => (showMetadataSearch = false)}
  />
{/if}

{#if movingFileId}
  <MoveFileModal
    title="Move File to Episode"
    items={siblingEpisodes}
    onMove={doMoveFile}
    onClose={() => {
      movingFileId = null;
      siblingEpisodes = [];
    }}
  />
{/if}

{#if releaseSearchEpisodeId}
  <ReleaseSearchModal
    entityTitle={releaseSearchTitle}
    initialQuery={releaseSearchTitle}
    onClose={() => (releaseSearchEpisodeId = null)}
    searchFn={(query, scope) => api.searchEpisodeReleases(releaseSearchEpisodeId!, query, scope)}
    scopeLabel="TV"
    smartSearchFn={() => api.searchEpisodeReleases(releaseSearchEpisodeId!)}
    queueFn={async (release, source) => {
      await api.queueEpisodeRelease(releaseSearchEpisodeId!, release, source);
      const entity = nav.current;
      if (entity?.type === 'episode') {
        const episode = await api.getEpisode(entity.id);
        updateCurrentEntityData(episode);
      }
    }}
    wantFn={async () => {
      await api.wantEpisode(releaseSearchEpisodeId!);
      const entity = nav.current;
      if (entity?.type === 'episode') {
        const episode = await api.getEpisode(entity.id);
        updateCurrentEntityData(episode);
      }
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

  .sidebar-add-episode {
    padding: 16px 0;
    border-top: 1px solid var(--ring);
  }
</style>
