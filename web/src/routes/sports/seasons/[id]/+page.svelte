<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import ContentPageView from '#lib/components/ContentPageView.svelte';
  import HeroActions from '#lib/components/shared/HeroActions.svelte';
  import MetaLine from '#lib/components/shared/MetaLine.svelte';
  import { formatCalendarDate, pluralize } from '#lib/utils/format.js';
  import EntityHero from '#lib/components/shared/EntityHero.svelte';
  import SeasonForm from '#lib/components/tv/SeasonForm.svelte';
  import type { SeasonFormData } from '#lib/components/tv/SeasonForm.svelte';
  import EpisodeForm from '#lib/components/tv/EpisodeForm.svelte';
  import type { EpisodeFormData } from '#lib/components/tv/EpisodeForm.svelte';
  import { SearchSelect } from '@koblibs/ui';
  import { showMessage } from '#lib/context/app.svelte.js';
  import * as api from '#lib/api/client.js';
  import { hasOutstandingWants, unwantMessage } from '#lib/utils/want-status.js';
  import { getStatsState } from '#lib/stores/stats.svelte.js';
  import ReleaseSearchModal from '#lib/components/ReleaseSearchModal.svelte';
  import TvEpisodeRow from '#lib/components/tv/TvEpisodeRow.svelte';
  import type { TvSeason, TvShow } from '#lib/types.js';
  import EpisodeBatchActions from '#lib/components/tv/EpisodeBatchActions.svelte';
  import BulkSelectToggle from '#lib/components/shared/BulkSelectToggle.svelte';
  import { EpisodeBulkSelection } from '#lib/utils/episode-bulk.svelte.js';

  let season = $state<TvSeason | null>(null);
  let loading = $state(true);
  let editing = $state(false);
  let saving = $state(false);
  let deleting = $state(false);
  let addingEvent = $state(false);
  let savingEvent = $state(false);
  let parentLeague = $state<TvShow | null>(null);
  let formValid = $state(false);
  let triggerSave = $state<(() => void) | undefined>();

  const seasonId = $derived(page.params.id);
  const statsState = getStatsState();
  let releaseSearchEventId = $state<string | null>(null);

  const bulk = new EpisodeBulkSelection({
    onDone: async () => {
      if (season) await loadSeason(season.id);
    },
    notify: showMessage,
    noun: 'event'
  });

  async function loadSeason(id: string) {
    loading = true;
    try {
      season = await api.getSportsSeason(id);
      if (season?.show_id && !parentLeague) {
        try {
          parentLeague = await api.getSportsLeague(season.show_id);
        } catch {
          // Non-critical
        }
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load season');
    } finally {
      loading = false;
    }
  }

  const handleBack = () => {
    if (season?.show_id) {
      goto(`/sports/${season.show_id}`);
    } else {
      goto('/sports');
    }
  };

  async function handleWantSeason() {
    if (!season) return;
    try {
      await api.wantSportsSeason(season.id);
      showMessage('Season events wanted');
      await loadSeason(season.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want season');
    }
  }

  async function handleUnwantSeason() {
    if (!season) return;
    if (
      !confirm(
        'Unwant every outstanding event in this season? Events already in your library are unaffected.'
      )
    )
      return;
    try {
      const result = await api.unwantSportsSeason(season.id);
      showMessage(unwantMessage(result, 'season', 'event'));
      await loadSeason(season.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant season');
    }
  }

  const handleEdit = () => {
    editing = true;
  };

  const handleEditCancel = () => {
    editing = false;
  };

  const handleEditDelete = async () => {
    if (!season) return;
    if (!confirm('Delete this season? This cannot be undone.')) return;
    deleting = true;
    try {
      await api.deleteSportsSeason(season.id);
      showMessage('Season deleted');
      if (season.show_id) {
        goto(`/sports/${season.show_id}`);
      } else {
        goto('/sports');
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete season');
    } finally {
      deleting = false;
    }
  };

  const handleClose = () => handleBack();

  const eventTitle = (event: { title: string | null; episode_sort_order: number }) =>
    event.title || `Event ${event.episode_sort_order}`;

  const releaseSearchEvent = $derived(
    season?.episodes.find((e) => e.id === releaseSearchEventId) ?? null
  );

  async function reloadSeason() {
    if (season) await loadSeason(season.id);
  }

  async function handleWantEvent(eventId: string) {
    try {
      await api.wantSportsEvent(eventId);
      showMessage('Event added to wanted');
      await reloadSeason();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want event');
    }
  }

  async function handleUnwantEvent(eventId: string) {
    try {
      await api.unwantSportsEvent(eventId);
      showMessage('Event removed from wanted');
      await reloadSeason();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unwant event');
    }
  }

  async function handleEventWatchToggle(eventId: string) {
    const watched = !(season?.episodes.find((e) => e.id === eventId)?.watched ?? false);
    try {
      await api.updateEpisodeWatchState(eventId, {
        watched,
        watched_at: watched ? new Date().toISOString() : null
      });
      showMessage(watched ? 'Marked as watched' : 'Marked as unwatched');
      await reloadSeason();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to toggle watch state');
    }
  }

  async function handleSeasonWatchToggle(watched: boolean) {
    if (!season) return;
    try {
      const result = await api.bulkUpdateSeasonWatchState(season.id, watched);
      const n = result.updated_count;
      showMessage(`Marked ${n} event${n === 1 ? '' : 's'} as ${watched ? 'watched' : 'unwatched'}`);
      await reloadSeason();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update watch state');
    }
  }

  const handleSave = async (data: SeasonFormData, posterFile?: File) => {
    if (!season) return;
    saving = true;
    try {
      await api.updateSportsSeason(season.id, data as unknown as Record<string, unknown>);
      if (posterFile) {
        await api.uploadSportsSeasonPoster(season.id, posterFile);
      }
      showMessage('Season updated');
      editing = false;
      await loadSeason(season.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update season');
    } finally {
      saving = false;
    }
  };

  const handleAddEvent = () => {
    addingEvent = true;
  };

  const handleSaveEvent = async (data: EpisodeFormData) => {
    if (!season) return;
    savingEvent = true;
    try {
      await api.createSportsEvent(season.id, {
        episode_identifier: data.episode_identifier || data.title,
        episode_sort_order: data.episode_sort_order ?? 0,
        title: data.title,
        description: data.description || undefined,
        air_date: data.air_date || undefined,
        runtime: data.runtime ?? undefined
      });
      showMessage('Event created');
      addingEvent = false;
      await loadSeason(season.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to create event');
    } finally {
      savingEvent = false;
    }
  };

  const autoWantOptions = [
    { value: 'inherit', label: 'Inherit' },
    { value: 'none', label: 'None' },
    { value: 'all', label: 'All Events' },
    { value: 'new_only', label: 'New Only' },
    { value: 'fill_gaps', label: 'Fill Gaps' }
  ];

  const handleAutoWantChange = async (value: string) => {
    if (!season) return;
    try {
      await api.setSportsSeasonAutoWant(season.id, value);
      showMessage('Auto-want updated');
      await loadSeason(season.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update auto-want');
    }
  };

  onMount(() => {
    if (seasonId) loadSeason(seasonId);
  });

  $effect(() => {
    if (seasonId && (!season || season.id !== seasonId)) loadSeason(seasonId);
  });

  const sortedEvents = $derived(
    season ? [...season.episodes].sort((a, b) => a.episode_sort_order - b.episode_sort_order) : []
  );

  const anyWanted = $derived(hasOutstandingWants(sortedEvents));
  const allWatched = $derived(
    sortedEvents.length > 0 && sortedEvents.every((e) => e.watched === true)
  );
</script>

<svelte:head>
  <title>{season?.title || `Season ${season?.season_sort_order ?? ''}`} — Kobbler</title>
</svelte:head>

{#if loading && !season}
  <div class="loading-state"><p>Loading season...</p></div>
{:else if season && editing}
  <ContentPageView
    noHeader={true}
    noPadding={true}
    pageBackdropUrl={parentLeague?.backdrop_url || season.poster_url}
    onClose={handleClose}
  >
    {#snippet headerActions()}
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
        <h2 class="edit-hero-title">Editing: {season.title || `${season.season_sort_order}`}</h2>
      </div>
    </div>

    <div class="page-container">
      <SeasonForm
        {season}
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
{:else if season && addingEvent}
  <ContentPageView
    noHeader={true}
    noPadding={true}
    pageBackdropUrl={parentLeague?.backdrop_url || season.poster_url}
    onClose={() => (addingEvent = false)}
  >
    {#snippet headerActions()}
      <button type="button" class="glass-btn" onclick={() => (addingEvent = false)} title="Close"
        ><span class="material-icons-outlined">close</span></button
      >
    {/snippet}

    <div class="edit-hero">
      <div class="edit-hero-content">
        <h2 class="edit-hero-title">Add Event</h2>
      </div>
    </div>

    <div class="page-container">
      <EpisodeForm
        createMode
        onSave={handleSaveEvent}
        onCancel={() => (addingEvent = false)}
        saving={savingEvent}
      />
    </div>
  </ContentPageView>
{:else if season}
  <ContentPageView
    noHeader={true}
    noPadding={true}
    pageBackdropUrl={parentLeague?.backdrop_url || season.poster_url}
    onBack={handleBack}
    onClose={handleBack}
    onEdit={handleEdit}
  >
    <EntityHero
      title={season.title || `${season.season_sort_order}`}
      subtitle={parentLeague?.title}
      coverUrl={season.poster_url}
      backdropUrl={parentLeague?.backdrop_url || undefined}
      logoUrl={parentLeague?.logo_url}
      description={season.description}
      hideRating={true}
    >
      {#snippet meta()}
        <MetaLine
          items={[
            formatCalendarDate(season!.air_date),
            pluralize(season!.episodes.length, 'event')
          ]}
        />
      {/snippet}

      {#snippet actions()}
        <HeroActions
          actions={[
            {
              label: 'Want Missing',
              icon: 'bookmark_add',
              tone: 'primary',
              pinned: true,
              onSelect: handleWantSeason
            },
            {
              label: allWatched ? 'Mark Unwatched' : 'Mark Watched',
              icon: allWatched ? 'visibility_off' : 'visibility',
              pinned: true,
              onSelect: () => handleSeasonWatchToggle(!allWatched)
            },
            anyWanted && {
              label: 'Unwant All',
              icon: 'playlist_remove',
              title: 'Drop every outstanding want in this season',
              onSelect: handleUnwantSeason
            }
          ]}
        />
      {/snippet}
    </EntityHero>

    <div class="full-page-main-grid page-container">
      <div class="details-column">
        <div class="card-section">
          <div class="section-head">
            <div class="label">Events</div>
            {#if sortedEvents.length > 0}
              <BulkSelectToggle
                active={bulk.active}
                title="Select events"
                onToggle={() => bulk.toggleMode()}
              />
            {/if}
          </div>
          {#if sortedEvents.length === 0}
            <p class="muted">No events available.</p>
          {:else}
            {#if bulk.active}
              <EpisodeBatchActions
                selectedCount={bulk.count}
                totalCount={sortedEvents.length}
                loading={bulk.busy}
                noun="event"
                onSelectAll={() => bulk.selectMany(sortedEvents)}
                onClearSelection={() => bulk.clear()}
                onApply={(action) => bulk.apply(action)}
              />
            {/if}
            <div class="events-list">
              {#each sortedEvents as event (event.id)}
                <TvEpisodeRow
                  episode={event}
                  href="/sports/events/{event.id}"
                  number={event.episode_sort_order}
                  title={eventTitle(event)}
                  noun="event"
                  onWant={() => handleWantEvent(event.id)}
                  onUnwant={() => handleUnwantEvent(event.id)}
                  onWatchToggle={() => handleEventWatchToggle(event.id)}
                  onSearchReleases={statsState.stats.kobload_configured
                    ? () => (releaseSearchEventId = event.id)
                    : undefined}
                  selectionMode={bulk.active}
                  selected={bulk.has(event.id)}
                  onToggleSelect={(shiftKey: boolean) =>
                    bulk.toggle(event.id, sortedEvents, shiftKey)}
                />
              {/each}
            </div>
          {/if}
        </div>
      </div>

      <div class="actions-column side-first">
        <div class="card-section">
          <div class="label">Manage</div>
          <div class="manage-actions">
            <div class="manage-row">
              <span class="manage-label">Auto-want</span>
              <div class="auto-want-wrapper">
                <SearchSelect
                  items={autoWantOptions}
                  value={season?.auto_want_mode || 'inherit'}
                  onchange={handleAutoWantChange}
                />
              </div>
            </div>
            <button class="btn btn-outline full-width" type="button" onclick={handleAddEvent}>
              <span class="material-icons-outlined">add</span>
              Add Event
            </button>
          </div>
        </div>

        <div class="card-section">
          <div class="label">Stats</div>
          <div class="stats-grid">
            <div class="stat">
              <span class="stat-value">{season.episodes.length}</span>
              <span class="stat-label">Events</span>
            </div>
            <div class="stat">
              <span class="stat-value">{season.episodes.filter((e) => e.is_in_library).length}</span
              >
              <span class="stat-label">In Library</span>
            </div>
            <div class="stat">
              <span class="stat-value"
                >{season.episodes.filter((e) => e.want_status === 'wanted').length}</span
              >
              <span class="stat-label">Wanted</span>
            </div>
            <div class="stat">
              <span class="stat-value">{season.episodes.filter((e) => e.watched).length}</span>
              <span class="stat-label">Watched</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  </ContentPageView>
{/if}

{#if releaseSearchEvent}
  <ReleaseSearchModal
    entityTitle={eventTitle(releaseSearchEvent)}
    onClose={() => (releaseSearchEventId = null)}
    searchFn={(query, scope) =>
      api.searchSportsEventReleases(releaseSearchEvent.id, query || undefined, scope)}
    scopeLabel="sports"
    smartSearchFn={() => api.searchSportsEventReleases(releaseSearchEvent.id)}
    queueFn={async (release, source) => {
      await api.queueSportsEventRelease(releaseSearchEvent.id, source, release);
      await reloadSeason();
    }}
    wantFn={async () => {
      await api.wantSportsEvent(releaseSearchEvent.id);
      await reloadSeason();
    }}
  />
{/if}

<style>
  .full-page-main-grid {
    --detail-side-width: 340px;
  }

  .loading-state {
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 200px;
    color: var(--muted);
  }

  .edit-hero {
    position: relative;
    z-index: 1;
    padding: 40px 32px 24px;
  }

  .edit-hero-content {
    max-width: 800px;
    margin: 0 auto;
  }

  .edit-hero-title {
    margin: 0;
    font-size: 24px;
    font-weight: 800;
    color: var(--ink);
  }

  .edit-footer {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 16px 32px;
    background: var(--card);
    border-top: 1px solid var(--ring);
  }

  .spacer {
    flex: 1;
  }

  .btn-sec {
    background: var(--bg-2);
    border: 1px solid var(--ring);
    color: var(--ink);
  }

  .btn-danger {
    background: transparent;
    border: 1px solid var(--ring);
    color: var(--error, #ef4444);
  }

  .btn-danger:hover {
    background: rgba(239, 68, 68, 0.1);
    border-color: var(--error, #ef4444);
  }

  .manage-actions {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .manage-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  .manage-label {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
  }

  .auto-want-wrapper {
    min-width: 140px;
  }

  .full-width {
    width: 100%;
    justify-content: center;
  }

  .stats-grid {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 12px;
  }

  .stat {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
  }

  .stat-value {
    font-size: 20px;
    font-weight: 800;
    color: var(--ink);
  }

  .stat-label {
    font-size: 11px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .card-section {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 24px;
  }

  .label {
    font-size: 11px;
    font-weight: 800;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.1em;
    margin-bottom: 12px;
  }

  .events-list {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  .btn {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 10px 20px;
    border-radius: 100px;
    font-size: 14px;
    font-weight: 700;
    cursor: pointer;
    border: none;
    transition: all 0.2s;
  }

  .btn-primary {
    background: var(--accent);
    color: var(--bg);
  }

  .muted {
    color: var(--muted);
    font-size: 13px;
  }

  /* Phone widths: trim the section padding so event rows have room. */
  @media (max-width: 600px) {
    .card-section {
      padding: 16px 10px;
    }
  }
</style>
