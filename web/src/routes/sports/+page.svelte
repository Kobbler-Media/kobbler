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
  import { fade } from 'svelte/transition';
  import SportsSearchModal from '#lib/components/SportsSearchModal.svelte';
  import SportsLeagueDetailsPanel from '#lib/components/sports/SportsLeagueDetailsPanel.svelte';
  import SportsSeasonDetailsPanel from '#lib/components/sports/SportsSeasonDetailsPanel.svelte';
  import SportsEventDetailsPanel from '#lib/components/sports/SportsEventDetailsPanel.svelte';
  import MoveFileModal from '#lib/components/shared/MoveFileModal.svelte';
  import TvShowForm from '#lib/components/tv/TvShowForm.svelte';
  import type { TvShowFormData } from '#lib/components/tv/TvShowForm.svelte';
  import SeasonForm from '#lib/components/tv/SeasonForm.svelte';
  import type { SeasonFormData } from '#lib/components/tv/SeasonForm.svelte';
  import EpisodeForm from '#lib/components/tv/EpisodeForm.svelte';
  import type { EpisodeFormData } from '#lib/components/tv/EpisodeForm.svelte';
  import ReleaseSearchModal from '#lib/components/ReleaseSearchModal.svelte';
  import { ExpandableSidebar } from '@koblibs/ui';
  import ErrorBanner from '#lib/components/shared/ErrorBanner.svelte';
  import BidirectionalScroll from '#lib/components/BidirectionalScroll.svelte';
  import VideoPlayer from '#lib/components/VideoPlayer/VideoPlayer.svelte';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { usePlayback } from '#lib/composables/usePlayback.svelte.js';
  import { getStatsState } from '#lib/stores/stats.svelte.js';
  import * as api from '#lib/api/client.js';
  import { unwantMessage } from '#lib/utils/want-status.js';
  import type { TvShow, TvSeason, TvEpisode } from '#lib/types.js';
  import {
    pushEntity,
    popEntity,
    clearStack,
    getNavigationState,
    updateCurrentEntityData,
    setLoadingEntity
  } from '#lib/stores/navigation.svelte.js';

  type FilterValue = 'all' | 'monitored' | 'ended';

  let leagues = $state<TvShow[]>([]);
  let loading = $state(true);
  let error = $state('');
  let filter = $state<FilterValue>('all');
  let sort = $state<'title' | 'created_at'>('title');
  let searchQuery = $state('');
  let showAddModal = $state(false);
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let availableLetters = $state<string[]>([]);
  let editMode = $state(false);
  let formRequestSave: (() => void) | undefined = $state();
  let formCanSave: boolean | undefined = $state();
  let releaseSearchEventId = $state<string | null>(null);
  let nextCursor = $state<string | null>(null);
  let prevCursor = $state<string | null>(null);
  let totalCount = $state<number | null>(null);
  let movingFileId = $state<string | null>(null);
  let moveTargetItems = $state<Array<{ value: string; label: string }>>([]);

  let localSearchQuery = $state('');
  let loadGeneration = 0;
  const urlAround = $derived(page.url.searchParams.get('around'));
  const urlAroundBefore = $derived(page.url.searchParams.get('around_before'));

  const nav = getNavigationState();
  const stats = getStatsState();

  let reconnectHandler: (() => void) | null = null;
  let sseHandler: ((data: unknown) => void) | null = null;

  const playback = usePlayback({
    mediaType: 'episode',
    refreshWatchState: async () => {}
  });

  async function handlePlayEvent(eventId: string) {
    try {
      const files = await api.getEpisodeFiles(eventId);
      if (files.length === 0) {
        showMessage('No files available for this event');
        return;
      }
      await playback.play(files[0].id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to start playback');
    }
  }

  async function loadLeagues(options?: {
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
      else if (filter === 'ended') params.status = 'ended';
      params.sort = sort;
      if (options?.cursor) params.cursor = options.cursor;
      if (options?.direction) params.direction = options.direction;
      if (options?.around) params.around = options.around;
      if (options?.around_before != null) params.around_before = options.around_before;
      if (options?.limit != null) params.limit = options.limit;

      const result = await api.getSportsLeagues(params);
      if (gen !== loadGeneration) return;
      const mode = options?.mode ?? 'replace';

      if (mode === 'append') {
        const existingIds = new Set(leagues.map((l) => l.id));
        const uniqueNew = result.items.filter((l) => !existingIds.has(l.id));
        leagues = [...leagues, ...uniqueNew];
        nextCursor = result.next_cursor;
      } else if (mode === 'prepend') {
        const existingIds = new Set(leagues.map((l) => l.id));
        const uniqueNew = result.items.filter((l) => !existingIds.has(l.id));
        leagues = [...uniqueNew, ...leagues];
        prevCursor = result.prev_cursor ?? null;
      } else {
        leagues = result.items;
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
        return await loadLeagues({ ...options, around: undefined, around_before: undefined });
      }
      showMessage(e instanceof Error ? e.message : 'Failed to load leagues');
      leagues = [];
      error = 'Failed to load data. Please check your connection and try again.';
    } finally {
      if (gen === loadGeneration) loading = false;
    }
  }

  async function loadAvailableLetters() {
    try {
      availableLetters = await api.getSportsLeagueAvailableLetters();
    } catch (e) {
      console.error('Failed to load available letters:', e);
      availableLetters = [];
    }
  }

  /**
   * Reload the current view in-place, keeping the reader where they are.
   *
   * The window has to be re-requested at its current size and centred on the first
   * *visible* card: a default-limit reload centred on leagues[0] throws away every page
   * that was scrolled into view, the grid collapses to 50 cards, and the browser clamps
   * scrollTop to the new (much shorter) content.
   */
  function reloadInPlace(): Promise<void> {
    // The around/anchor mechanism is name-cursor based and only works under title
    // sort; under recently-added the server returns the same page for any cursor.
    if (sort !== 'title' || leagues.length === 0) {
      return loadLeagues();
    }
    const anchor = captureScrollAnchor();
    const anchorIndex = anchor ? leagues.findIndex((l) => l.id === anchor.id) : -1;
    const index = anchorIndex >= 0 ? anchorIndex : 0;
    const limit = Math.min(Math.max(leagues.length, 50), 200);
    const before = Math.min(index, Math.floor(limit / 2));
    return loadLeagues({ around: leagues[index].id, around_before: before, limit }).then(() =>
      restoreScrollAnchor(anchor)
    );
  }

  onMount(() => {
    lastAround = urlAround;
    const initialAround = urlAround;
    if (initialAround) {
      const ab = urlAroundBefore ? parseInt(urlAroundBefore, 10) : 0;
      loadLeagues({ around: initialAround, around_before: ab }).then(() => {
        if (ab > 0) scrollItemToTop(initialAround);
        else scrollContainerToTop();
        updateUrlParams({ around: null, around_before: null });
      });
    } else {
      loadLeagues();
    }
    loadAvailableLetters();

    reconnectHandler = onReconnect(() => {
      reloadInPlace();
      loadAvailableLetters();
    });

    sseHandler = onDebounced('tv_show_updated', () => {
      reloadInPlace();
      loadAvailableLetters();
    });

    return () => clearTimeout(searchTimer);
  });

  onDestroy(() => {
    if (reconnectHandler) offReconnect(reconnectHandler);
    if (sseHandler) off('tv_show_updated', sseHandler);
  });

  // Handle around parameter changes (e.g., letter jump while already on page)
  let lastAround = $state<string | null>(null);
  $effect(() => {
    const around = urlAround;
    if (around && around !== lastAround) {
      lastAround = around;
      const ab = urlAroundBefore ? parseInt(urlAroundBefore, 10) : 0;
      loadLeagues({ around, around_before: ab }).then(() => {
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

  async function handleJumpToLetter(letter: string | null) {
    if (!letter) {
      // Back to the start of the list. A goto() here is usually a no-op (the URL is
      // already bare by the time the previous jump cleared `around`), so reload directly.
      updateUrlParams({ around: null, around_before: null });
      await loadLeagues();
      scrollContainerToTop();
      return;
    }

    loading = true;
    try {
      const result = await api.getSportsLeagueFirstByLetter(letter);
      if (result) {
        goto(`/sports?around=${result.id}&around_before=0`);
      } else {
        showMessage(`No leagues starting with "${letter}"`);
        loading = false;
      }
    } catch {
      showMessage(`Failed to find leagues for "${letter}"`);
      loading = false;
    }
  }

  function loadMoreLeagues() {
    if (!nextCursor || loading) return;
    loadLeagues({ cursor: nextCursor, direction: 'forward', mode: 'append' });
  }

  function loadPreviousLeagues() {
    if (!prevCursor || loading) return;
    loadLeagues({ cursor: prevCursor, direction: 'backward', mode: 'prepend' });
  }

  function handleFilterChange(newFilter: FilterValue) {
    filter = newFilter;
    nextCursor = null;
    prevCursor = null;
    totalCount = null;
    // A new filter/search is a new list: start it at the top, since the scroll
    // container keeps its offset across the swap.
    loadLeagues().then(scrollContainerToTop);
  }

  function handleSortChange(newSort: 'title' | 'created_at') {
    sort = newSort;
    nextCursor = null;
    prevCursor = null;
    totalCount = null;
    loadLeagues().then(scrollContainerToTop);
  }

  function handleSearch(query: string) {
    searchQuery = query;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      nextCursor = null;
      prevCursor = null;
      totalCount = null;
      loadLeagues().then(scrollContainerToTop);
    }, 300);
  }

  function handleSelectLeague(league: TvShow) {
    if (window.matchMedia('(max-width: 768px)').matches) {
      goto(`/sports/${league.id}`);
    } else {
      pushEntity('tv_show', league.id);
    }
  }

  async function handleAddComplete(id: string) {
    showAddModal = false;
    await loadLeagues();
    try {
      const league = await api.getSportsLeague(id);
      pushEntity('tv_show', id, league);
    } catch {
      // League was added, just refresh the list
    }
  }

  function handleCloseSidebar() {
    if (editMode) {
      editMode = false;
    } else {
      clearStack();
    }
  }

  function handleExpandLeague() {
    const entity = nav.current;
    if (entity?.type === 'tv_show') {
      editMode = false;
      clearStack();
      goto(`/sports/${entity.id}`);
    }
  }

  function handleBack() {
    editMode = false;
    popEntity();
  }

  function handleToggleEdit() {
    editMode = !editMode;
  }

  function handleEditCancel() {
    editMode = false;
  }

  // Load entity data when nav stack changes
  $effect(() => {
    const entity = nav.current;
    if (!entity) return;
    if (entity.data) return;

    if (entity.type === 'tv_show') {
      loadNavLeagueData(entity.id);
    } else if (entity.type === 'season') {
      loadNavSeasonData(entity.id);
    } else if (entity.type === 'episode') {
      loadNavEventData(entity.id);
    }
  });

  async function loadNavLeagueData(leagueId: string) {
    setLoadingEntity(true);
    try {
      const league = await api.getSportsLeague(leagueId);
      updateCurrentEntityData(league);
    } catch (e) {
      console.error('Failed to load league:', e);
    } finally {
      setLoadingEntity(false);
    }
  }

  async function loadNavSeasonData(seasonId: string) {
    setLoadingEntity(true);
    try {
      const season = await api.getSportsSeason(seasonId);
      updateCurrentEntityData(season);
    } catch (e) {
      console.error('Failed to load season:', e);
    } finally {
      setLoadingEntity(false);
    }
  }

  async function loadNavEventData(eventId: string) {
    setLoadingEntity(true);
    try {
      const event = await api.getSportsEvent(eventId);
      updateCurrentEntityData(event);
    } catch (e) {
      console.error('Failed to load event:', e);
    } finally {
      setLoadingEntity(false);
    }
  }

  async function handleMonitorToggle(monitored: boolean, mode?: string) {
    const entity = nav.current;
    if (entity?.type !== 'tv_show' || !entity.data) return;
    const league = entity.data as TvShow;
    try {
      await api.setSportsLeagueMonitored(
        entity.id,
        monitored,
        mode ?? league.auto_want_mode ?? 'none'
      );
      showMessage(monitored ? 'League monitored' : 'League unmonitored');
      const updated = await api.getSportsLeague(entity.id);
      updateCurrentEntityData(updated);
      await reloadInPlace();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update monitor');
    }
  }

  async function handleAutoWantChange(mode: string) {
    const entity = nav.current;
    if (entity?.type !== 'tv_show' || !entity.data) return;
    const league = entity.data as TvShow;
    try {
      const monitored = mode !== 'none' ? true : league.is_monitored;
      await api.setSportsLeagueMonitored(entity.id, monitored, mode);
      if (mode !== 'none' && !league.is_monitored) {
        showMessage('League monitored with auto-want');
      }
      const updated = await api.getSportsLeague(entity.id);
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
      await api.wantAllMissingSports(entity.id);
      showMessage('Wanting all missing events');
      const updated = await api.getSportsLeague(entity.id);
      updateCurrentEntityData(updated);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want missing events');
    }
  }

  async function handleUnwantAll() {
    const entity = nav.current;
    if (entity?.type !== 'tv_show') return;
    const league = entity.data as TvShow | undefined;
    if (
      !confirm(
        `Unwant every outstanding event in "${league?.title ?? 'this league'}"? Events already in your library are unaffected.`
      )
    )
      return;
    try {
      const result = await api.unwantAllSports(entity.id);
      showMessage(unwantMessage(result, 'league', 'event'));
      const updated = await api.getSportsLeague(entity.id);
      updateCurrentEntityData(updated);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant events');
    }
  }

  async function handleRefreshMetadata() {
    const entity = nav.current;
    if (entity?.type !== 'tv_show') return;
    try {
      await api.refreshSportsMetadata(entity.id);
      showMessage('Metadata refresh queued');
      setTimeout(async () => {
        const league = await api.getSportsLeague(entity.id);
        updateCurrentEntityData(league);
      }, 1500);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to refresh metadata');
    }
  }

  function handleSeasonClick(seasonId: string) {
    pushEntity('season', seasonId);
  }

  function handleEventClick(eventId: string) {
    pushEntity('episode', eventId);
  }

  async function handleWantSeason() {
    const entity = nav.current;
    if (entity?.type !== 'season') return;
    try {
      await api.wantSportsSeason(entity.id);
      showMessage('Season events wanted');
      const season = await api.getSportsSeason(entity.id);
      updateCurrentEntityData(season);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want season');
    }
  }

  async function handleUnwantSeason() {
    const entity = nav.current;
    if (entity?.type !== 'season') return;
    if (
      !confirm(
        'Unwant every outstanding event in this season? Events already in your library are unaffected.'
      )
    )
      return;
    try {
      const result = await api.unwantSportsSeason(entity.id);
      showMessage(unwantMessage(result, 'season', 'event'));
      const season = await api.getSportsSeason(entity.id);
      updateCurrentEntityData(season);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant season');
    }
  }

  async function handleSeasonAutoWantChange(mode: string) {
    const entity = nav.current;
    if (entity?.type !== 'season') return;
    try {
      await api.setSportsSeasonAutoWant(entity.id, mode);
      showMessage('Season auto-want updated');
      const season = await api.getSportsSeason(entity.id);
      updateCurrentEntityData(season);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update season auto-want');
    }
  }

  async function handleWantEvent() {
    const entity = nav.current;
    if (entity?.type !== 'episode') return;
    try {
      await api.wantSportsEvent(entity.id);
      showMessage('Event added to wanted');
      const event = await api.getSportsEvent(entity.id);
      updateCurrentEntityData(event);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want event');
    }
  }

  async function handleUnwantEvent() {
    const entity = nav.current;
    if (entity?.type !== 'episode') return;
    try {
      await api.unwantSportsEvent(entity.id);
      showMessage('Event removed from wanted');
      const event = await api.getSportsEvent(entity.id);
      updateCurrentEntityData(event);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant event');
    }
  }

  function handleSearchEventReleases(eventId?: string) {
    const eid = eventId ?? (nav.current?.type === 'episode' ? nav.current.id : null);
    if (eid) {
      releaseSearchEventId = eid;
    }
  }

  async function handleMoveFile(fileId: string) {
    const entity = nav.current;
    if (entity?.type !== 'episode' || !entity.data) return;
    const event = entity.data as TvEpisode;
    movingFileId = fileId;
    try {
      const season = await api.getSportsSeason(event.season_id);
      moveTargetItems = season.episodes
        .filter((ep) => ep.id !== event.id)
        .map((ep) => ({ value: ep.id, label: ep.title || `Event ${ep.episode_sort_order}` }));
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load sibling events');
      movingFileId = null;
    }
  }

  async function doMoveFile(targetEventId: string) {
    const entity = nav.current;
    if (entity?.type !== 'episode' || !movingFileId) return;
    await api.moveSportsEventFile(entity.id, movingFileId, targetEventId);
    movingFileId = null;
    moveTargetItems = [];
    const event = await api.getSportsEvent(entity.id);
    updateCurrentEntityData(event);
  }

  async function handleLeagueEditSave(
    data: TvShowFormData,
    posterFile?: File,
    backdropFile?: File,
    logoFile?: File
  ) {
    const entity = nav.current;
    if (entity?.type !== 'tv_show' || !entity.data) return;
    loading = true;
    try {
      const { cast: _cast, crew: _crew, ...metadata } = data;
      await api.updateSportsLeague(entity.id, metadata as unknown as Record<string, unknown>);
      if (posterFile) await api.uploadSportsLeaguePoster(entity.id, posterFile);
      if (backdropFile) await api.uploadSportsLeagueBackdrop(entity.id, backdropFile);
      if (logoFile) await api.uploadSportsLeagueLogo(entity.id, logoFile);
      showMessage('League updated');
      editMode = false;
      const league = await api.getSportsLeague(entity.id);
      updateCurrentEntityData(league);
      await reloadInPlace();
    } catch {
      showMessage('Failed to update league');
    } finally {
      loading = false;
    }
  }

  async function handleSeasonEditSave(data: SeasonFormData, posterFile?: File) {
    const entity = nav.current;
    if (entity?.type !== 'season' || !entity.data) return;
    loading = true;
    try {
      await api.updateSportsSeason(entity.id, data as unknown as Record<string, unknown>);
      if (posterFile) {
        await api.uploadSportsSeasonPoster(entity.id, posterFile);
      }
      showMessage('Season updated');
      editMode = false;
      const season = await api.getSportsSeason(entity.id);
      updateCurrentEntityData(season);
    } catch {
      showMessage('Failed to update season');
    } finally {
      loading = false;
    }
  }

  async function handleEventEditSave(data: EpisodeFormData) {
    const entity = nav.current;
    if (entity?.type !== 'episode' || !entity.data) return;
    loading = true;
    try {
      await api.updateSportsEvent(entity.id, data as unknown as Record<string, unknown>);
      showMessage('Event updated');
      editMode = false;
      const event = await api.getSportsEvent(entity.id);
      updateCurrentEntityData(event);
    } catch {
      showMessage('Failed to update event');
    } finally {
      loading = false;
    }
  }

  async function handleDeleteLeague() {
    const entity = nav.current;
    if (entity?.type !== 'tv_show') return;
    if (!confirm('Delete this league? This cannot be undone.')) return;
    try {
      await api.deleteSportsLeague(entity.id);
      showMessage('League deleted');
      clearStack();
      await reloadInPlace();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete league');
    }
  }

  const showEditButton = $derived(
    (nav.current?.type === 'tv_show' ||
      nav.current?.type === 'season' ||
      nav.current?.type === 'episode') &&
      !editMode
  );

  const sidebarTitle = $derived.by(() => {
    if (editMode) {
      if (nav.current?.type === 'season') return 'Edit Season';
      if (nav.current?.type === 'episode') return 'Edit Event';
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
      return ep.title || `Event ${ep.episode_sort_order}`;
    }
    return 'Details';
  });

  const sectionTitle = $derived(
    filter === 'monitored'
      ? 'Monitored Leagues'
      : filter === 'ended'
        ? 'Ended Leagues'
        : 'All Leagues'
  );

  const countDisplay = $derived(
    totalCount != null ? `${totalCount} leagues` : `${leagues.length} leagues`
  );
  const ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ#'.split('');

  const statusBadge = (league: TvShow) => {
    if (league.is_monitored && league.status === 'ended') return { label: 'Ended', cls: 'ended' };
    if (league.is_monitored) return { label: 'Monitored', cls: 'monitored' };
    return null;
  };
</script>

<svelte:head>
  <title>Sports — Kobbler</title>
</svelte:head>

{#if error}
  <div class="error-wrapper">
    <ErrorBanner message={error} onRetry={() => loadLeagues()} onDismiss={() => (error = '')} />
  </div>
{/if}

<div class="sports-page page-container">
  <div class="toolbar">
    <div class="toolbar-left">
      <div class="view-controls">
        <button
          class="view-tab"
          class:active={filter === 'all'}
          type="button"
          onclick={() => handleFilterChange('all')}
        >
          All
        </button>
        <button
          class="view-tab"
          class:active={filter === 'monitored'}
          type="button"
          onclick={() => handleFilterChange('monitored')}
        >
          Monitored
        </button>
        <button
          class="view-tab"
          class:active={filter === 'ended'}
          type="button"
          onclick={() => handleFilterChange('ended')}
        >
          Ended
        </button>
      </div>
    </div>

    <div class="toolbar-right">
      <div class="sort-controls">
        <select
          class="sort-select"
          value={sort}
          onchange={(e) =>
            handleSortChange((e.target as HTMLSelectElement).value as 'title' | 'created_at')}
        >
          <option value="title">A-Z</option>
          <option value="created_at">Recently Added</option>
        </select>
      </div>
      <div class="search-box">
        <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor" class="search-icon">
          <path
            d="M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z"
          ></path>
        </svg>
        <input
          type="text"
          placeholder="Search leagues..."
          value={localSearchQuery}
          oninput={(e) => {
            localSearchQuery = (e.target as HTMLInputElement).value;
            handleSearch(localSearchQuery);
          }}
        />
        {#if localSearchQuery}
          <button
            type="button"
            class="clear-btn"
            aria-label="Clear search"
            onclick={() => {
              localSearchQuery = '';
              handleSearch('');
            }}
          >
            <svg viewBox="0 0 24 24" width="12" height="12" fill="currentColor">
              <path
                d="M19 6.41 17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z"
              ></path>
            </svg>
          </button>
        {/if}
      </div>

      <button
        class="icon-btn add-btn"
        type="button"
        title="Add league"
        aria-label="Add league"
        onclick={() => (showAddModal = true)}
      >
        <span class="material-icons-outlined" style="font-size: 18px;">add</span>
      </button>
    </div>
  </div>

  {#if loading && leagues.length === 0}
    <div class="loading-card">Loading leagues...</div>
  {:else if leagues.length === 0}
    <div class="empty-state">
      <h3>No leagues</h3>
      <p class="muted">Add a sports league to get started.</p>
    </div>
  {:else}
    <div class="section-header">
      <h2>{sectionTitle}</h2>
      <span class="count">{countDisplay}</span>
    </div>

    <div class="grid-container">
      <BidirectionalScroll
        hasMore={nextCursor !== null}
        hasPrevious={prevCursor !== null}
        {loading}
        onLoadMore={loadMoreLeagues}
        onLoadPrevious={loadPreviousLeagues}
      >
        <section class="leagues-grid">
          {#each leagues as league (league.id)}
            {@const badge = statusBadge(league)}
            {@const posterUrl = league.poster_url
              ? `${league.poster_url}?t=${encodeURIComponent(league.updated_at || '')}`
              : null}
            <button
              class="league-card"
              type="button"
              data-item-id={league.id}
              onclick={() => handleSelectLeague(league)}
              in:fade={{ duration: 220 }}
            >
              <div class="poster" style={posterUrl ? `background-image: url(${posterUrl});` : ''}>
                {#if !posterUrl}
                  <div class="poster-placeholder">
                    <span class="material-icons-outlined" style="font-size: 32px;"
                      >emoji_events</span
                    >
                  </div>
                {/if}
                {#if badge}
                  <span class="status-badge {badge.cls}">{badge.label}</span>
                {/if}
              </div>
              <div class="meta">
                <strong class="title">{league.title}</strong>
                <div class="subtitle">
                  {#if league.network}
                    <span>{league.network}</span>
                  {/if}
                  {#if league.year}
                    <span>({league.year})</span>
                  {/if}
                </div>
                {#if league.total_episode_count != null}
                  <div class="event-info">{league.total_episode_count} events</div>
                {/if}
              </div>
            </button>
          {/each}
        </section>
      </BidirectionalScroll>
    </div>

    {#if filter === 'all' && sort === 'title'}
      {@const availableSet = new Set(availableLetters)}
      <aside class="alpha-jump">
        <button type="button" class="alpha-btn" onclick={() => handleJumpToLetter(null)}>*</button>
        {#each ALPHABET as letter (letter)}
          <button
            type="button"
            class="alpha-btn"
            onclick={() => handleJumpToLetter(letter)}
            disabled={availableLetters.length > 0 && !availableSet.has(letter)}
          >
            {letter}
          </button>
        {/each}
      </aside>
    {/if}
  {/if}
</div>

<ExpandableSidebar
  open={nav.isOpen}
  expanded={editMode}
  title={sidebarTitle}
  showEdit={showEditButton}
  showRefresh={nav.current?.type === 'tv_show' && !editMode}
  {editMode}
  onClose={handleCloseSidebar}
  onExpand={nav.current?.type === 'tv_show' ? handleExpandLeague : undefined}
  onToggleEdit={handleToggleEdit}
  onRefresh={handleRefreshMetadata}
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
          <button
            class="btn-danger-sm"
            type="button"
            onclick={handleDeleteLeague}
            disabled={loading}
          >
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
          onSave={handleLeagueEditSave}
          onCancel={handleEditCancel}
          saving={loading}
          hideCastCrew
          mediaType="sport"
          hideFooter
          bind:requestSave={formRequestSave}
          bind:canSave={formCanSave}
        />
      {/key}
    {:else}
      <SportsLeagueDetailsPanel
        league={nav.current.data as TvShow}
        onMonitorToggle={handleMonitorToggle}
        onAutoWantChange={handleAutoWantChange}
        onWantAllMissing={handleWantAllMissing}
        onUnwantAll={handleUnwantAll}
        onRefreshMetadata={handleRefreshMetadata}
        onSeasonClick={handleSeasonClick}
      />
    {/if}
  {:else if nav.current?.type === 'season' && nav.current.data}
    {#if editMode}
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
    {:else}
      <SportsSeasonDetailsPanel
        season={nav.current.data as TvSeason}
        onEventClick={handleEventClick}
        onPlayEvent={handlePlayEvent}
        onWantSeason={handleWantSeason}
        onUnwantSeason={handleUnwantSeason}
        onAutoWantChange={handleSeasonAutoWantChange}
        onEdit={handleToggleEdit}
        onDelete={async () => {
          const season = nav.current?.data as TvSeason;
          if (!season || !confirm('Delete this season? This cannot be undone.')) return;
          try {
            await api.deleteSportsSeason(season.id);
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
          onSave={handleEventEditSave}
          onCancel={handleEditCancel}
          saving={loading}
          hideFooter
          bind:requestSave={formRequestSave}
          bind:canSave={formCanSave}
        />
      {/key}
    {:else}
      <SportsEventDetailsPanel
        event={nav.current.data as TvEpisode}
        onWant={handleWantEvent}
        onUnwant={handleUnwantEvent}
        onSearchReleases={stats.stats.kobload_configured
          ? () => handleSearchEventReleases()
          : undefined}
        onMoveFile={handleMoveFile}
        onEdit={handleToggleEdit}
        onDelete={async () => {
          const event = nav.current?.data as TvEpisode;
          if (!event || !confirm('Delete this event? This cannot be undone.')) return;
          try {
            await api.deleteSportsEvent(event.id);
            showMessage('Event deleted');
            popEntity();
            reloadInPlace();
          } catch (e) {
            showMessage(e instanceof Error ? e.message : 'Failed to delete event');
          }
        }}
      />
    {/if}
  {:else if nav.isOpen}
    <div class="loading-sidebar">Loading...</div>
  {/if}
</ExpandableSidebar>

{#if showAddModal}
  <SportsSearchModal onClose={() => (showAddModal = false)} onComplete={handleAddComplete} />
{/if}

{#if releaseSearchEventId}
  <ReleaseSearchModal
    entityTitle={(nav.current?.data as TvEpisode)?.title || 'Event'}
    onClose={() => (releaseSearchEventId = null)}
    searchFn={(query, scope) =>
      api.searchSportsEventReleases(releaseSearchEventId!, query || undefined, scope)}
    scopeLabel="sports"
    smartSearchFn={() => api.searchSportsEventReleases(releaseSearchEventId!)}
    queueFn={async (release, source) => {
      await api.queueSportsEventRelease(releaseSearchEventId!, source, release);
      const entity = nav.current;
      if (entity?.type === 'episode') {
        const event = await api.getSportsEvent(entity.id);
        updateCurrentEntityData(event);
      }
    }}
    wantFn={async () => {
      await api.wantSportsEvent(releaseSearchEventId!);
      const entity = nav.current;
      if (entity?.type === 'episode') {
        const event = await api.getSportsEvent(entity.id);
        updateCurrentEntityData(event);
      }
    }}
  />
{/if}

{#if movingFileId}
  <MoveFileModal
    title="Move to event"
    items={moveTargetItems}
    onMove={doMoveFile}
    onClose={() => {
      movingFileId = null;
      moveTargetItems = [];
    }}
  />
{/if}

{#if playback.isActive}
  <VideoPlayer
    sessionResponse={playback.session!}
    creationParams={{
      media_type: 'episode',
      file_id: playback.playingFileId!,
      capabilities: playback.capabilities!
    }}
    mediaId=""
    onClose={playback.close}
  />
{/if}

<style>
  .error-wrapper {
    padding: 12px 24px 0;
  }

  .toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
    margin-bottom: 16px;
  }

  .toolbar-left {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .toolbar-right {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .view-controls {
    display: flex;
    gap: 2px;
    background: var(--card);
    padding: 4px;
    border-radius: 8px;
    border: 1px solid var(--ring);
  }

  .view-tab {
    padding: 6px 12px;
    border-radius: 6px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    color: var(--muted);
    border: none;
    background: none;
    transition: background 0.15s;
  }

  .view-tab:hover {
    background: rgba(255, 255, 255, 0.05);
    color: var(--ink);
  }

  .view-tab.active {
    background: var(--bg-2);
    color: var(--ink);
  }

  .sort-controls {
    display: flex;
    align-items: center;
  }

  .sort-select {
    appearance: none;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 6px;
    padding: 6px 28px 6px 10px;
    font-size: 12px;
    font-weight: 600;
    color: var(--muted);
    cursor: pointer;
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='6' viewBox='0 0 10 6'%3E%3Cpath d='M0 0l5 6 5-6z' fill='%23888'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 8px center;
  }

  .sort-select:hover {
    color: var(--ink);
    border-color: var(--accent);
  }

  .search-box {
    display: flex;
    align-items: center;
    gap: 8px;
    background: var(--bg);
    border: 1px solid var(--ring);
    border-radius: 10px;
    padding: 6px 10px;
    flex: 1;
    max-width: 280px;
  }

  .search-box:focus-within {
    border-color: var(--accent);
  }

  .search-box input {
    border: none;
    background: transparent;
    outline: none;
    font-size: 13px;
    color: var(--ink);
    flex: 1;
    min-width: 0;
  }

  .search-box input::placeholder {
    color: var(--muted);
  }

  .search-icon {
    color: var(--muted);
    flex-shrink: 0;
  }

  .clear-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    color: var(--muted);
    cursor: pointer;
    padding: 2px;
    border-radius: 4px;
  }

  .clear-btn:hover {
    color: var(--ink);
    background: var(--bg-2);
  }

  .icon-btn {
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: var(--card);
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
  }

  .icon-btn:hover:not(:disabled) {
    background: var(--bg-2);
    color: var(--ink);
  }

  .section-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 16px;
    gap: 16px;
  }

  .section-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .count {
    font-size: 13px;
    color: var(--muted);
  }

  .grid-container {
    flex: 1;
    min-width: 0;
    padding-right: 30px;
  }

  .leagues-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 24px;
    padding-bottom: 120px;
  }

  .league-card {
    appearance: none;
    text-align: left;
    color: inherit;
    font: inherit;
    cursor: pointer;
    border-radius: 12px;
    padding: 10px;
    background: var(--card);
    border: 1px solid var(--ring);
    display: flex;
    flex-direction: column;
    gap: 8px;
    position: relative;
    overflow: hidden;
    transition:
      transform 0.15s ease,
      box-shadow 0.15s ease;
  }

  .league-card:hover {
    transform: translateY(-3px);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
  }

  :global([data-theme='light']) .league-card:hover {
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.12);
  }

  .poster {
    aspect-ratio: 1 / 1;
    width: 100%;
    border-radius: 12px;
    background: linear-gradient(140deg, #2e323a, #1a1d23);
    background-size: cover;
    background-position: center;
    position: relative;
    overflow: hidden;
    border: 1px solid var(--ring);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
  }

  :global([data-theme='light']) .poster {
    background: linear-gradient(140deg, #e5e0d8, #d5cfc5);
    background-size: cover;
    background-position: center;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.12);
  }

  .poster-placeholder {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 100%;
    color: var(--muted);
    opacity: 0.4;
  }

  .status-badge {
    position: absolute;
    top: 8px;
    right: 8px;
    font-size: 10px;
    font-weight: 800;
    padding: 2px 6px;
    border-radius: 4px;
    box-shadow: 0 2px 4px rgba(0, 0, 0, 0.3);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .status-badge.monitored {
    background: var(--success, #22c55e);
    color: #000;
  }

  .status-badge.ended {
    background: var(--muted);
    color: #fff;
  }

  .meta {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .title {
    font-size: 14px;
    font-weight: 700;
    line-height: 1.3;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .subtitle {
    display: flex;
    gap: 6px;
    color: var(--muted);
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .event-info {
    color: var(--accent);
    font-size: 11px;
    margin-top: 2px;
  }

  .alpha-jump {
    position: fixed;
    right: 8px;
    top: 50%;
    transform: translateY(-50%);
    display: flex;
    flex-direction: column;
    gap: 2px;
    z-index: 30;
    background: var(--card);
    padding: 4px;
    border-radius: 8px;
    border: 1px solid var(--ring);
  }

  .alpha-btn {
    width: 20px;
    height: 18px;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 4px;
    border: none;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    font-size: 10px;
    font-weight: 600;
    text-align: center;
    transition: background 0.15s;
  }

  .alpha-btn:hover:not(:disabled) {
    background: var(--accent-dim);
    color: var(--accent);
  }

  .alpha-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  .empty-state {
    border: none;
    opacity: 0.7;
    margin-top: 24px;
  }

  .empty-state h3 {
    margin: 0 0 8px;
  }

  .muted {
    color: var(--muted);
    font-size: 13px;
  }

  .loading-card {
    border: none;
    background: none;
    color: var(--muted);
    padding: 24px;
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

  .sidebar-footer-actions .spacer {
    flex: 1;
  }

  .btn-danger-sm {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 6px 12px;
    border-radius: 8px;
    font-size: 12px;
    font-weight: 600;
    border: none;
    background: var(--error, #ef4444);
    color: #fff;
    cursor: pointer;
  }

  .btn-danger-sm:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-danger-sm :global(.material-icons-outlined) {
    font-size: 14px;
  }

  @media (max-width: 768px) {
    .toolbar {
      flex-wrap: wrap;
      gap: 8px;
    }

    .toolbar-left {
      flex: 1 1 100%;
    }

    .toolbar-right {
      flex: 1 1 100%;
    }

    .search-box {
      flex: 1;
      max-width: none;
    }

    .leagues-grid {
      grid-template-columns: repeat(auto-fill, minmax(115px, 1fr));
      gap: 12px;
    }

    .alpha-jump {
      display: none;
    }

    .grid-container {
      padding-right: 0;
    }
  }
</style>
