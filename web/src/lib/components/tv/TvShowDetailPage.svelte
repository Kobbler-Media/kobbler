<script lang="ts">
  import { onMount } from 'svelte';
  import type { TvEpisode, TvShow, TvWatchState } from '$lib/types';
  import {
    getTvShowRating,
    updateTvShowRating,
    deleteTvShowRating,
    getEpisodeFiles,
    getEpisodeWatchState
  } from '$lib/api/client';
  import ContentPageView from '$lib/components/ContentPageView.svelte';
  import GenreChips from '$lib/components/GenreChips.svelte';
  import DualRatingStars from '$lib/components/DualRatingStars.svelte';
  import { DeleteConfirmModal } from '@koblibs/ui';
  import TvSeasonAccordion from './TvSeasonAccordion.svelte';
  import EpisodeBatchActions from './EpisodeBatchActions.svelte';
  import { EpisodeBulkSelection } from '$lib/utils/episode-bulk.svelte';
  import BulkSelectToggle from '$lib/components/shared/BulkSelectToggle.svelte';
  import EntityHero from '../shared/EntityHero.svelte';
  import HeroActions from '../shared/HeroActions.svelte';
  import MetaLine from '../shared/MetaLine.svelte';
  import KidsAccess from '../shared/KidsAccess.svelte';
  import { getAuthState } from '$lib/stores/auth.svelte';
  import { formatShowStatus, formatDuration } from '$lib/utils/format';
  import { hasOutstandingWants } from '$lib/utils/want-status';
  import VideoPlayer from '$lib/components/VideoPlayer/VideoPlayer.svelte';
  import { usePlayback } from '$lib/composables/usePlayback.svelte';
  import { useStreamPlayback } from '$lib/composables/useStreamPlayback.svelte';
  import { showMessage } from '$lib/context/app.svelte';

  interface Props {
    show: TvShow;
    syncing?: boolean;
    onMonitorToggle: (monitored: boolean, mode: string) => void;
    onWantAllMissing: () => void;
    onUnwantAll: () => void;
    onRefresh: () => void;
    onWantSeason: (seasonId: string) => void;
    onUnwantSeason: (seasonId: string) => void;
    onWantEpisode: (episodeId: string) => void;
    onUnwantEpisode: (episodeId: string) => void;
    onWatchToggle: (episodeId: string) => void;
    onSearchReleases?: (episodeId: string) => void;
    onClose: () => void;
    onBack?: () => void;
    onEdit?: () => void;
    onAddSeason?: () => void;
    onBulkWatchToggle?: (watched: boolean) => void;
    onRewatch?: () => Promise<void>;
    onBulkSeasonWatchToggle?: (seasonId: string, watched: boolean) => void;
    onCastClick?: (personId: string, personName: string) => void;
    onGenreClick?: (genre: string) => void;
    onSeasonAutoWantChange?: (seasonId: string, mode: string) => void;
    onReanalyze?: (force?: boolean) => void;
    onWatchlistToggle?: (inWatchlist: boolean) => void;
    /** Refetch the show after a bulk episode edit. */
    onReload?: () => Promise<void> | void;
  }

  let {
    show,
    syncing = false,
    onMonitorToggle,
    onWantAllMissing,
    onUnwantAll,
    onRefresh,
    onWantSeason,
    onUnwantSeason,
    onWantEpisode,
    onUnwantEpisode,
    onWatchToggle,
    onBulkWatchToggle,
    onRewatch,
    onBulkSeasonWatchToggle,
    onSearchReleases,
    onClose,
    onBack,
    onEdit,
    onAddSeason,
    onCastClick,
    onGenreClick,
    onSeasonAutoWantChange,
    onReanalyze,
    onWatchlistToggle,
    onReload
  }: Props = $props();

  const auth = getAuthState();

  const bulk = new EpisodeBulkSelection({
    onDone: async () => {
      await onReload?.();
    },
    notify: showMessage
  });

  const allEpisodes = $derived(show.seasons?.flatMap((s) => s.episodes ?? []) ?? []);
  const hasWatchHistory = $derived(
    allEpisodes.some((e) => e.watched === true || (e.watch_position_seconds ?? 0) > 0)
  );

  let confirmingRewatch = $state(false);
  let rewatching = $state(false);

  async function confirmRewatch() {
    if (!onRewatch) return;
    rewatching = true;
    try {
      await onRewatch();
      confirmingRewatch = false;
    } finally {
      rewatching = false;
    }
  }

  let expandedSeasons = $state<Set<string>>(new Set());
  let showRating = $state<number | null>(null);
  let playingEpisodeId = $state<string | null>(null);
  let playingEpisodeWatchState = $state<TvWatchState | null>(null);

  const playback = usePlayback({
    mediaType: 'episode',
    refreshWatchState: async () => {
      if (playingEpisodeId) {
        playingEpisodeWatchState = await getEpisodeWatchState(playingEpisodeId);
      }
    }
  });

  // Server picks continue_episode via watch history / first-aired fallback.
  // When it's null (no aired episodes, or user finished them all) there's
  // nothing useful to pre-warm.
  const targetEpisode = $derived(
    show.continue_episode
      ? {
          episodeId: show.continue_episode.episode_id,
          positionSeconds: show.continue_episode.position_seconds ?? 0
        }
      : null
  );

  const hasLibraryFiles = $derived(
    (show.seasons ?? []).some((s) => s.episodes.some((e) => e.is_in_library))
  );

  const stream = useStreamPlayback({
    mediaType: 'episode',
    getMediaId: () => targetEpisode?.episodeId ?? '',
    getSearchParams: () => ({ episode_id: targetEpisode?.episodeId ?? '' }),
    getResumePosition: () => undefined,
    getEstimatedDuration: () => undefined,
    onMessage: showMessage
  });

  function findEpisode(episodeId: string) {
    for (const season of show.seasons ?? []) {
      for (const ep of season.episodes) {
        if (ep.id === episodeId) return { season, episode: ep };
      }
    }
    return null;
  }

  async function handlePlayEpisode(episodeId: string) {
    try {
      const [files, watchState] = await Promise.all([
        getEpisodeFiles(episodeId),
        getEpisodeWatchState(episodeId)
      ]);
      if (files.length === 0) {
        showMessage('No files available for this episode');
        return;
      }
      playingEpisodeId = episodeId;
      playingEpisodeWatchState = watchState;
      // Don't resume if the episode is already watched (position is near the end)
      const resumePos = watchState?.watched
        ? undefined
        : (watchState?.position_seconds ?? undefined);
      await playback.play(files[0].id, undefined, resumePos);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to start playback');
    }
  }

  // Pre-warm policy:
  //   show has any library files  + target has file            -> local file pre-warm
  //   show has any library files  + target has no file         -> skip (never debrid from library)
  //   show has NO library files   + has watch history          -> debrid pre-warm continue_episode
  //   show has NO library files   + no watch history           -> debrid pre-warm first aired episode
  async function preWarmTargetEpisode() {
    const target = targetEpisode;
    if (!target) return;
    try {
      const files = await getEpisodeFiles(target.episodeId);
      if (files.length > 0) {
        await playback.preWarm(files[0].id, target.positionSeconds || undefined);
        return;
      }
      if (hasLibraryFiles) return;
      stream.preWarmStream();
    } catch {
      // Pre-warm is best-effort
    }
  }

  async function fetchShowRating() {
    try {
      const result = await getTvShowRating(show.id);
      showRating = result.rating;
    } catch {
      showRating = null;
    }
  }

  async function handleRatingChange(rating: number | null) {
    try {
      if (rating === null) {
        await deleteTvShowRating(show.id);
      } else {
        await updateTvShowRating(show.id, rating);
      }
      showRating = rating;
    } catch (e) {
      console.error('Failed to update show rating:', e);
    }
  }

  onMount(() => {
    fetchShowRating();
    return () => playback.teardownPreWarm();
  });

  $effect(() => {
    show.id;
    fetchShowRating();
  });

  // Stable key so pre-warm only re-triggers when the target episode (or its
  // resume position) actually changes, not on every SSE object replacement.
  const preWarmKey = $derived(
    targetEpisode ? `${targetEpisode.episodeId}:${targetEpisode.positionSeconds}` : null
  );

  $effect(() => {
    const key = preWarmKey;
    if (key) {
      preWarmTargetEpisode();
    } else {
      playback.teardownPreWarm();
      stream.reset();
    }
  });

  const autoWantMode = $derived(show.auto_want_mode || 'none');
  const seasons = $derived(show.seasons ?? []);
  const allWatched = $derived(
    seasons.length > 0 && seasons.every((s) => s.episodes.every((e) => e.watched === true))
  );
  const sortedSeasons = $derived(
    [...seasons].sort((a, b) => a.season_sort_order - b.season_sort_order)
  );
  const anyWanted = $derived(hasOutstandingWants(seasons.flatMap((s) => s.episodes ?? [])));
  const runtimeDisplay = $derived(show.runtime ? `${show.runtime} min` : null);

  const continueEpisode = $derived.by(() => {
    const ce = show.continue_episode;
    if (!ce || !show.seasons) return null;
    const season = show.seasons[ce.season_idx];
    if (!season) return null;
    const episode = season.episodes[ce.episode_idx];
    if (!episode) return null;
    const sNum = String(season.season_sort_order).padStart(2, '0');
    const eNum = episode.episode_sort_order;
    return {
      id: ce.episode_id,
      label: episode.title ? `S${sNum}E${eNum} \u00b7 ${episode.title}` : `S${sNum}E${eNum}`,
      isResume: ce.position_seconds > 0,
      positionSeconds: ce.position_seconds,
      totalSeconds: episode.runtime ? episode.runtime * 60 : null
    };
  });

  const continueProgressPercent = $derived.by(() => {
    if (!continueEpisode?.isResume || !continueEpisode.totalSeconds) return 0;
    return Math.min(100, (continueEpisode.positionSeconds / continueEpisode.totalSeconds) * 100);
  });

  const toggleSeason = (seasonId: string) => {
    const next = new Set(expandedSeasons);
    if (next.has(seasonId)) {
      next.delete(seasonId);
    } else {
      next.add(seasonId);
    }
    expandedSeasons = next;
  };

  const handleMonitorChange = () => {
    onMonitorToggle(!show.is_monitored, autoWantMode);
  };

  const handleWatchlistChange = () => {
    onWatchlistToggle?.(!show.in_watchlist);
  };

  const autoWantOptions = [
    { value: 'none', label: 'None' },
    { value: 'all', label: 'All Episodes' },
    { value: 'new_only', label: 'New Only' },
    { value: 'fill_gaps', label: 'Fill Gaps' }
  ];

  const handleAutoWantChange = (value: string) => {
    onMonitorToggle(show.is_monitored, value);
  };
</script>

<ContentPageView
  noHeader={true}
  noPadding={true}
  pageBackdropUrl={show.backdrop_url || show.poster_url}
  {onBack}
  {onClose}
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
    title={show.title}
    coverUrl={show.poster_url}
    backdropUrl={show.backdrop_url || undefined}
    logoUrl={show.logo_url}
    description={show.description}
  >
    {#snippet ratingContent()}
      <DualRatingStars
        communityRating={show.rating}
        communityRatingCount={show.rating_count ?? null}
        userRating={showRating}
        size="small"
        editable={true}
        onRatingChange={handleRatingChange}
      />
    {/snippet}

    {#snippet actions()}
      <HeroActions
        actions={[
          onWatchlistToggle && {
            label: show.in_watchlist ? 'On My List' : 'Want to Watch',
            icon: show.in_watchlist ? 'playlist_add_check' : 'playlist_add',
            title: show.in_watchlist
              ? 'Remove from your watch list'
              : 'Add to your watch list — shows up in Up Next',
            active: show.in_watchlist,
            pinned: true,
            onSelect: handleWatchlistChange
          },
          {
            label: show.is_monitored ? 'Monitored' : 'Monitor',
            icon: show.is_monitored ? 'star' : 'star_outline',
            active: show.is_monitored,
            pinned: true,
            onSelect: handleMonitorChange
          },
          { label: 'Want Missing', icon: 'library_add', onSelect: onWantAllMissing },
          anyWanted && {
            label: 'Unwant All',
            icon: 'playlist_remove',
            title: 'Drop every outstanding want on this show',
            onSelect: onUnwantAll
          },
          onBulkWatchToggle && {
            label: allWatched ? 'Mark Unwatched' : 'Mark Watched',
            icon: allWatched ? 'visibility_off' : 'visibility',
            onSelect: () => onBulkWatchToggle(!allWatched)
          },
          onRewatch &&
            hasWatchHistory && {
              label: 'Rewatch from the start',
              icon: 'restart_alt',
              onSelect: () => (confirmingRewatch = true)
            },
          onAddSeason && { label: 'Add Season', icon: 'add', onSelect: onAddSeason }
        ]}
        menuItems={autoWantOptions.map((o) => ({
          section: 'Auto-want',
          label: o.label,
          checked: autoWantMode === o.value,
          onSelect: () => handleAutoWantChange(o.value)
        }))}
      >
        {#snippet primary()}
          {#if continueEpisode}
            <button
              class="continue-btn"
              type="button"
              onclick={() => handlePlayEpisode(continueEpisode.id)}
            >
              <div class="continue-btn-main">
                <span class="material-icons-outlined">play_arrow</span>
                {continueEpisode.isResume ? 'Resume' : 'Play Next'}
              </div>
              <span class="continue-btn-episode">{continueEpisode.label}</span>
              {#if continueEpisode.isResume}
                <div class="continue-progress">
                  <div
                    class="continue-progress-fill"
                    style="width: {continueProgressPercent}%"
                  ></div>
                </div>
                <span class="continue-progress-label">
                  {formatDuration(continueEpisode.positionSeconds)}{continueEpisode.totalSeconds
                    ? ` / ${formatDuration(continueEpisode.totalSeconds)}`
                    : ''}
                </span>
              {/if}
            </button>
          {/if}
        {/snippet}
      </HeroActions>
    {/snippet}

    {#snippet meta()}
      <MetaLine>
        {#if show.year}<span>{show.year}</span>{/if}
        {#if show.network}<span>{show.network}</span>{/if}
        {#if show.status}<span class="status-pill">{formatShowStatus(show.status)}</span>{/if}
        {#if show.content_rating}<span class="content-rating">{show.content_rating}</span>{/if}
        {#if runtimeDisplay}<span>{runtimeDisplay}</span>{/if}
      </MetaLine>
    {/snippet}
  </EntityHero>

  <div class="full-page-main-grid page-container">
    <div class="details-column">
      <!-- Seasons -->
      <div class="section card-section">
        <div class="section-head">
          <div class="label">Seasons</div>
          {#if onReload && allEpisodes.length > 0}
            <BulkSelectToggle
              active={bulk.active}
              title="Select episodes"
              onToggle={() => bulk.toggleMode()}
            />
          {/if}
        </div>
        {#if sortedSeasons.length === 0}
          <p class="muted">No seasons available.</p>
        {:else}
          {#if bulk.active}
            <EpisodeBatchActions
              selectedCount={bulk.count}
              totalCount={allEpisodes.length}
              loading={bulk.busy}
              onSelectAll={() => bulk.selectMany(allEpisodes)}
              onClearSelection={() => bulk.clear()}
              onApply={(action) => bulk.apply(action)}
            />
          {/if}
          <div class="seasons-list">
            {#each sortedSeasons as season (season.id)}
              <TvSeasonAccordion
                {season}
                expanded={expandedSeasons.has(season.id)}
                onToggle={() => toggleSeason(season.id)}
                onWantSeason={() => onWantSeason(season.id)}
                onUnwantSeason={() => onUnwantSeason(season.id)}
                onWantEpisode={(epId: string) => onWantEpisode(epId)}
                onUnwantEpisode={(epId: string) => onUnwantEpisode(epId)}
                onWatchToggle={(epId: string) => onWatchToggle(epId)}
                onBulkWatchToggle={onBulkSeasonWatchToggle}
                {onSearchReleases}
                onPlayEpisode={handlePlayEpisode}
                onAutoWantChange={onSeasonAutoWantChange}
                selectionMode={bulk.active}
                isSelected={(id: string) => bulk.has(id)}
                onToggleSelect={(id: string, within: TvEpisode[], shiftKey: boolean) =>
                  bulk.toggle(id, within, shiftKey)}
                seasonAllSelected={bulk.allSelected(season.episodes ?? [])}
                onToggleSelectSeason={() => {
                  const eps = season.episodes ?? [];
                  if (bulk.allSelected(eps)) {
                    bulk.deselectMany(eps);
                  } else {
                    bulk.selectMany(eps);
                  }
                }}
              />
            {/each}
          </div>
        {/if}
      </div>

      <!-- Genres -->
      {#if show.genres?.length > 0}
        <div class="section compact-section">
          <div class="label">Genres</div>
          <GenreChips genres={show.genres} size="small" clickable={!!onGenreClick} {onGenreClick} />
        </div>
      {/if}
      {#if auth.isAdmin}
        <div class="section compact-section">
          <KidsAccess
            kind="tv"
            id={show.id}
            override={show.kids_override}
            allowed={show.kids_allowed}
          />
        </div>
      {/if}
    </div>

    <div class="actions-column">
      <!-- Cast -->
      {#if show.cast && show.cast.length > 0}
        <div class="section card-section">
          <div class="label">Cast</div>
          <div class="cast-grid">
            {#each show.cast.slice(0, 15) as member (member.id)}
              <div class="cast-item">
                {#if onCastClick}
                  <button
                    type="button"
                    class="cast-btn"
                    onclick={() => onCastClick?.(member.person_id, member.person_name)}
                  >
                    <div class="cast-mini-card">
                      {#if member.person_image_url}
                        <img class="cast-photo" src={member.person_image_url} alt="" />
                      {:else}
                        <div class="cast-photo placeholder">
                          <span class="material-icons-outlined">person</span>
                        </div>
                      {/if}
                      <div class="cast-info">
                        <span class="cast-name">{member.person_name}</span>
                        <span class="cast-character">{member.character_name}</span>
                      </div>
                    </div>
                  </button>
                {:else}
                  <div class="cast-mini-card">
                    {#if member.person_image_url}
                      <img class="cast-photo" src={member.person_image_url} alt="" />
                    {:else}
                      <div class="cast-photo placeholder">
                        <span class="material-icons-outlined">person</span>
                      </div>
                    {/if}
                    <div class="cast-info">
                      <span class="cast-name">{member.person_name}</span>
                      <span class="cast-character">{member.character_name}</span>
                    </div>
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        </div>
      {/if}
    </div>
  </div>
</ContentPageView>

{#if confirmingRewatch}
  <DeleteConfirmModal
    title="Rewatch from the start"
    message={`Rewatch ${show.title}? Every episode goes back to unwatched and Resume starts again from the first episode.`}
    confirmLabel="Start over"
    danger={false}
    loading={rewatching}
    onConfirm={confirmRewatch}
    onCancel={() => (confirmingRewatch = false)}
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
    mediaId={playingEpisodeId!}
    mediaInfo={{
      title: show.title,
      subtitle: (() => {
        const found = findEpisode(playingEpisodeId!);
        if (!found) return undefined;
        const { season, episode } = found;
        const prefix = `S${String(season.season_sort_order).padStart(2, '0')}E${episode.episode_sort_order}`;
        return episode.title ? `${prefix} \u00b7 ${episode.title}` : prefix;
      })(),
      seasonId: findEpisode(playingEpisodeId!)?.season.id,
      artworkUrl: findEpisode(playingEpisodeId!)?.episode.still_url ?? show.poster_url ?? undefined
    }}
    initialPosition={playingEpisodeWatchState?.watched
      ? undefined
      : (playingEpisodeWatchState?.position_seconds ?? undefined)}
    onClose={() => {
      playback.close();
      playingEpisodeId = null;
      playingEpisodeWatchState = null;
    }}
  />
{/if}

<style>
  .card-section {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 24px;
  }

  .compact-section {
    padding: 8px 0;
  }

  .content-rating {
    padding: 2px 8px;
    border: 1px solid var(--muted);
    border-radius: 6px;
    font-weight: 700;
    font-size: 12px;
    text-transform: uppercase;
  }

  .status-pill {
    padding: 2px 8px;
    border-radius: 6px;
    font-weight: 700;
    text-transform: capitalize;
    background: var(--bg-2);
    font-size: 12px;
  }

  .continue-btn {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding: 10px 24px;
    border-radius: 100px;
    border: none;
    background: var(--accent);
    color: var(--bg);
    cursor: pointer;
    font-weight: 700;
    transition: all 0.2s cubic-bezier(0.16, 1, 0.3, 1);
  }

  .continue-btn:hover {
    filter: brightness(1.1);
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(211, 140, 90, 0.25);
  }

  .continue-btn:active {
    transform: translateY(0);
  }

  .continue-btn-main {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 14px;
    font-weight: 700;
  }

  .continue-btn-main .material-icons-outlined {
    font-size: 20px;
  }

  .continue-btn-episode {
    font-size: 11px;
    font-weight: 500;
    opacity: 0.85;
    max-width: 220px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .continue-progress {
    width: 100%;
    height: 3px;
    background: rgba(0, 0, 0, 0.2);
    border-radius: 2px;
    overflow: hidden;
    margin-top: 2px;
  }

  .continue-progress-fill {
    height: 100%;
    background: var(--bg);
    border-radius: 2px;
    transition: width 0.3s ease;
  }

  .continue-progress-label {
    font-size: 10px;
    font-weight: 500;
    opacity: 0.7;
    font-variant-numeric: tabular-nums;
  }

  .label {
    font-size: 11px;
    font-weight: 800;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.1em;
    margin-bottom: 12px;
  }

  .seasons-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .cast-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: 8px;
  }

  .cast-btn {
    display: block;
    width: 100%;
    padding: 0;
    border: none;
    background: transparent;
    cursor: pointer;
    text-align: left;
    border-radius: 12px;
    transition: all 0.2s;
  }

  .cast-btn:hover {
    background: var(--bg-2);
  }

  .cast-mini-card {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px;
  }

  .cast-photo {
    flex-shrink: 0;
    width: 40px;
    height: 40px;
    border-radius: 50%;
    object-fit: cover;
    border: 1px solid var(--ring);
  }

  .cast-photo.placeholder {
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--bg-2);
    color: var(--muted);
  }

  .cast-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .cast-name {
    font-size: 13px;
    font-weight: 700;
    color: var(--ink);
  }

  .cast-character {
    font-size: 11px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Phone widths: trim the section padding so season rows keep to one line. */
  @media (max-width: 600px) {
    .card-section {
      padding: 16px 10px;
    }
  }

  .section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 12px;
  }

  .section-head .label {
    margin-bottom: 0;
  }
</style>
