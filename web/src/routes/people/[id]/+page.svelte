<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import ContentPageView from '$lib/components/ContentPageView.svelte';
  import EntityHero from '$lib/components/shared/EntityHero.svelte';
  import IdentifierLinks from '$lib/components/IdentifierLinks.svelte';
  import ErrorBanner from '$lib/components/shared/ErrorBanner.svelte';
  import PersonForm from '$lib/components/people/PersonForm.svelte';
  import type { PersonFormData } from '$lib/components/people/PersonForm.svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import * as api from '$lib/api/client';
  import type { PersonDetail, PersonFilmographyCredit } from '$lib/types';
  import { mediaTypeChip } from '$lib/utils/chip-mappings';
  import IconChip from '$lib/components/shared/IconChip.svelte';

  let person = $state<PersonDetail | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let editing = $state(false);
  let saving = $state(false);
  let deleting = $state(false);
  let refreshing = $state(false);

  // Filmography state
  let filmography = $state<PersonFilmographyCredit[]>([]);
  let filmographyLoading = $state(false);
  let filmographyError = $state<string | null>(null);

  // Filter state
  let creditTypeFilter = $state<'all' | 'cast' | 'crew'>('all');
  let mediaTypeFilter = $state<'all' | 'movie' | 'tv'>('all');
  let showDismissed = $state(false);

  // Monitoring state
  let localAutoWantMode = $state<string | undefined>(undefined);
  let updatingAutoWant = $state(false);

  // Want/dismiss in-progress tracking
  let wantingCredits = $state<Set<string>>(new Set());
  let dismissingCredits = $state<Set<string>>(new Set());

  const personId = $derived($page.params.id);

  const effectiveAutoWantMode = $derived(localAutoWantMode ?? person?.auto_want_mode ?? 'none');

  const autoWantModes = [
    { value: 'none', label: 'None' },
    { value: 'new_only', label: 'New' },
    { value: 'all', label: 'All' }
  ] as const;

  const groupedFilmography = $derived(groupCredits(filmography));

  // Filtered filmography credits (grouped — one entry per movie/show)
  const filteredCredits = $derived.by(() => {
    let credits = groupedFilmography;

    if (!showDismissed) {
      credits = credits.filter((c) => !c.dismissed);
    }

    if (creditTypeFilter !== 'all') {
      credits = credits.filter((c) => c.creditTypes.has(creditTypeFilter));
    }

    if (mediaTypeFilter !== 'all') {
      credits = credits.filter((c) => c.media_type === mediaTypeFilter);
    }

    // Sort by year descending (newest first), nulls last
    return credits.toSorted((a, b) => {
      const ya = a.year ?? 0;
      const yb = b.year ?? 0;
      return yb - ya;
    });
  });

  const dismissedCount = $derived(groupedFilmography.filter((c) => c.dismissed).length);

  // Library stats from provider filmography (grouped to avoid double-counting)
  const libraryMovieCount = $derived(
    groupedFilmography.filter((c) => c.media_type === 'movie' && c.in_library).length
  );
  const libraryTvCount = $derived(
    groupedFilmography.filter((c) => c.media_type === 'tv' && c.in_library).length
  );

  type GroupedCredit = {
    provider_id: string;
    media_type: string;
    title: string;
    year?: number;
    poster_url?: string;
    in_library: boolean;
    library_id?: string;
    want_status?: string;
    dismissed: boolean;
    roles: string[];
    creditTypes: Set<string>;
  };

  function creditKey(c: { provider_id: string; media_type: string }): string {
    return `${c.provider_id}:${c.media_type}`;
  }

  function groupCredits(credits: PersonFilmographyCredit[]): GroupedCredit[] {
    const map = new Map<string, GroupedCredit>();
    for (const c of credits) {
      const key = creditKey(c);
      let group = map.get(key);
      if (!group) {
        group = {
          provider_id: c.provider_id,
          media_type: c.media_type,
          title: c.title,
          year: c.year,
          poster_url: c.poster_url,
          in_library: c.in_library,
          library_id: c.library_id,
          want_status: c.want_status,
          dismissed: c.dismissed,
          roles: [],
          creditTypes: new Set()
        };
        map.set(key, group);
      }
      if (c.in_library) group.in_library = true;
      if (c.library_id) group.library_id = c.library_id;
      if (c.want_status && c.want_status !== 'none') group.want_status = c.want_status;

      group.creditTypes.add(c.credit_type);

      const roleDesc =
        c.credit_type === 'cast' ? c.character || 'Unknown role' : c.job || c.department || 'Crew';
      if (!group.roles.includes(roleDesc)) {
        group.roles.push(roleDesc);
      }
    }
    return Array.from(map.values());
  }

  async function loadPerson(id: string) {
    loading = true;
    error = null;
    try {
      person = await api.getPerson(id);
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load person';
    } finally {
      loading = false;
    }
  }

  async function loadFilmography(id: string) {
    filmographyLoading = true;
    filmographyError = null;
    try {
      const result = await api.getPersonFilmography(id);
      filmography = result.credits;
    } catch (e) {
      filmography = [];
      filmographyError = e instanceof Error ? e.message : 'Failed to load filmography';
    } finally {
      filmographyLoading = false;
    }
  }

  async function handleWant(credit: GroupedCredit) {
    if (!person) return;
    const key = creditKey(credit);
    wantingCredits.add(key);
    wantingCredits = new Set(wantingCredits);
    try {
      await api.wantPersonCredit(
        person.id,
        credit.provider_id,
        credit.media_type,
        credit.title,
        credit.year
      );
      // Update the local filmography entry
      filmography = filmography.map((c) =>
        creditKey(c) === key ? { ...c, in_library: true, want_status: 'wanted' } : c
      );
      showMessage(`Added "${credit.title}" to wanted`);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to want credit');
    } finally {
      wantingCredits.delete(key);
      wantingCredits = new Set(wantingCredits);
    }
  }

  async function handleDismiss(credit: GroupedCredit) {
    if (!person) return;
    const key = creditKey(credit);
    dismissingCredits.add(key);
    dismissingCredits = new Set(dismissingCredits);
    try {
      await api.dismissPersonCredit(person.id, credit.provider_id, credit.media_type);
      filmography = filmography.map((c) => (creditKey(c) === key ? { ...c, dismissed: true } : c));
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to dismiss');
    } finally {
      dismissingCredits.delete(key);
      dismissingCredits = new Set(dismissingCredits);
    }
  }

  async function handleUndismiss(credit: GroupedCredit) {
    if (!person) return;
    const key = creditKey(credit);
    dismissingCredits.add(key);
    dismissingCredits = new Set(dismissingCredits);
    try {
      await api.undismissPersonCredit(person.id, credit.provider_id, credit.media_type);
      filmography = filmography.map((c) => (creditKey(c) === key ? { ...c, dismissed: false } : c));
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to undismiss');
    } finally {
      dismissingCredits.delete(key);
      dismissingCredits = new Set(dismissingCredits);
    }
  }

  async function handleAutoWantModeChange(value: string) {
    if (!person || value === effectiveAutoWantMode) return;

    updatingAutoWant = true;
    try {
      if (value !== 'none' && !person.is_monitored) {
        await api.updatePersonMonitoring(person.id, {
          is_monitored: true,
          auto_want_mode: value
        });
      } else {
        await api.updatePersonMonitoring(person.id, {
          auto_want_mode: value
        });
      }
      localAutoWantMode = value;
      await loadPerson(person.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update monitoring');
      localAutoWantMode = undefined;
    } finally {
      updatingAutoWant = false;
    }
  }

  async function handleMonitoringToggle() {
    if (!person) return;
    const newMonitored = !person.is_monitored;
    try {
      await api.updatePersonMonitoring(person.id, {
        is_monitored: newMonitored,
        auto_want_mode: newMonitored ? person.auto_want_mode : 'none'
      });
      await loadPerson(person.id);
      localAutoWantMode = undefined;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to toggle monitoring');
    }
  }

  async function handleRefresh() {
    if (!person) return;
    refreshing = true;
    try {
      await api.refreshPersonMetadata(person.id);
      showMessage('Enrichment queued');
      // Reload after a brief delay to show updated data
      setTimeout(async () => {
        await loadPerson(person!.id);
        await loadFilmography(person!.id);
        refreshing = false;
      }, 2000);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to refresh');
      refreshing = false;
    }
  }

  async function handleWantAllMissing() {
    if (!person) return;
    const missing = groupedFilmography.filter(
      (c) => !c.in_library && !c.dismissed && c.creditTypes.has('cast')
    );
    if (missing.length === 0) {
      showMessage('No missing cast credits to want');
      return;
    }
    if (!confirm(`Want ${missing.length} missing cast credits?`)) return;
    let count = 0;
    for (const credit of missing) {
      try {
        await api.wantPersonCredit(
          person.id,
          credit.provider_id,
          credit.media_type,
          credit.title,
          credit.year
        );
        filmography = filmography.map((c) =>
          creditKey(c) === creditKey(credit) ? { ...c, in_library: true, want_status: 'wanted' } : c
        );
        count++;
      } catch {
        // Continue with next
      }
    }
    showMessage(`Added ${count} items to wanted`);
  }

  function handleCreditClick(credit: GroupedCredit) {
    if (!credit.library_id) return;
    if (credit.media_type === 'movie') {
      goto(`/movies/${credit.library_id}`);
    } else if (credit.media_type === 'tv') {
      goto(`/tv/${credit.library_id}`);
    }
  }

  const handleBack = () => window.history.back();

  const handleSave = async (data: PersonFormData) => {
    if (!person) return;
    saving = true;
    try {
      await api.updatePerson(person.id, { ...data });
      showMessage('Person updated');
      editing = false;
      await loadPerson(person.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update person');
    } finally {
      saving = false;
    }
  };

  const handleDelete = async () => {
    if (!person) return;
    if (!confirm(`Delete "${person.name}"? This cannot be undone.`)) return;
    deleting = true;
    try {
      await api.deletePerson(person.id);
      showMessage('Person deleted');
      window.history.back();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete person');
    } finally {
      deleting = false;
    }
  };

  function formatDates(person: PersonDetail): string {
    const parts: string[] = [];
    if (person.birth_date) {
      parts.push(`b. ${person.birth_date}`);
    }
    if (person.death_date) {
      parts.push(`d. ${person.death_date}`);
    }
    return parts.join(' · ');
  }

  function getWantStatusBadge(status?: string): { text: string; class: string } | null {
    switch (status) {
      case 'wanted':
        return { text: 'Wanted', class: 'wanted' };
      case 'searching':
        return { text: 'Searching', class: 'searching' };
      case 'downloading':
        return { text: 'Downloading', class: 'downloading' };
      case 'none':
        return { text: 'In Library', class: 'owned' };
      default:
        return status ? { text: status, class: 'owned' } : { text: 'In Library', class: 'owned' };
    }
  }

  onMount(() => {
    if (personId) {
      loadPerson(personId);
      loadFilmography(personId);
    }
  });

  $effect(() => {
    if (personId && (!person || person.id !== personId)) {
      loadPerson(personId);
      loadFilmography(personId);
    }
  });
</script>

<svelte:head>
  <title>{person?.name ?? 'Person'} — Kobbler</title>
</svelte:head>

{#if loading && !person}
  <div class="loading-state"><p>Loading...</p></div>
{:else if error && !person}
  <div class="error-state">
    <ErrorBanner message={error} onRetry={() => personId && loadPerson(personId)} />
  </div>
{:else if person && editing}
  <div class="edit-container">
    <h2>Edit Person</h2>
    <PersonForm
      {person}
      onSave={handleSave}
      onCancel={() => (editing = false)}
      onDelete={handleDelete}
      {saving}
      {deleting}
    />
  </div>
{:else if person}
  <ContentPageView
    noHeader={true}
    noPadding={true}
    pageBackdropUrl={person.image_url}
    onBack={handleBack}
    onClose={handleBack}
    onEdit={() => (editing = true)}
    onRefresh={handleRefresh}
    syncing={refreshing}
  >
    <EntityHero
      title={person.name}
      coverUrl={person.image_url}
      avatar={true}
      description={person.bio}
    >
      {#snippet subtitle()}
        <div class="hero-meta">
          {#if person?.known_for_department}
            <span class="department-label">{person.known_for_department}</span>
          {/if}
          {#if person?.known_for_department && (person?.birth_date || person?.death_date)}
            <span class="meta-sep">·</span>
          {/if}
          {#if person?.birth_date || person?.death_date}
            <span class="dates-text">{formatDates(person!)}</span>
          {/if}
          {#if person?.place_of_birth}
            <span class="meta-sep">·</span>
            <span class="place-text">{person.place_of_birth}</span>
          {/if}
        </div>
      {/snippet}

      {#snippet meta()}
        <span class="hero-stat">
          {libraryMovieCount} movies · {libraryTvCount} TV shows in library
        </span>
      {/snippet}

      {#snippet actions()}
        <button
          type="button"
          class="monitored-pill"
          class:inactive={!person?.is_monitored}
          onclick={handleMonitoringToggle}
        >
          <span class="material-icons-outlined"
            >{person?.is_monitored ? 'star' : 'star_outline'}</span
          >
          {person?.is_monitored ? 'Monitored' : 'Monitor'}
        </button>
      {/snippet}

      {#snippet extraContent()}
        {#if person?.identifiers && Object.keys(person.identifiers).length > 0}
          <div class="hero-identifiers">
            <IdentifierLinks identifiers={person.identifiers} entityType="person" />
          </div>
        {/if}
      {/snippet}
    </EntityHero>

    <div class="main-grid page-container">
      <!-- Left column: Filmography -->
      <div class="filmography-column">
        <div class="section card-section">
          <div class="section-header">
            <div class="label">Filmography</div>
            <div class="filter-controls">
              <div class="filter-group">
                {#each [{ value: 'all', label: 'All' }, { value: 'cast', label: 'Cast' }, { value: 'crew', label: 'Crew' }] as tab (tab.value)}
                  <button
                    type="button"
                    class="filter-chip"
                    class:active={creditTypeFilter === tab.value}
                    onclick={() => (creditTypeFilter = tab.value as 'all' | 'cast' | 'crew')}
                  >
                    {tab.label}
                  </button>
                {/each}
              </div>
              <div class="filter-group">
                {#each [{ value: 'all', label: 'All' }, { value: 'movie', label: 'Movies' }, { value: 'tv', label: 'TV' }] as tab (tab.value)}
                  <button
                    type="button"
                    class="filter-chip"
                    class:active={mediaTypeFilter === tab.value}
                    onclick={() => (mediaTypeFilter = tab.value as 'all' | 'movie' | 'tv')}
                  >
                    {tab.label}
                  </button>
                {/each}
              </div>
            </div>
          </div>

          {#if filmographyLoading}
            <div class="filmography-loading">Loading filmography...</div>
          {:else if filmographyError}
            <ErrorBanner
              message={filmographyError}
              onRetry={() => personId && loadFilmography(personId)}
            />
          {:else if filteredCredits.length === 0}
            <div class="empty-filmography">No credits match the current filters.</div>
          {:else}
            <div class="credit-list">
              {#each filteredCredits as credit (creditKey(credit))}
                {@const isWanting = wantingCredits.has(creditKey(credit))}
                {@const isDismissing = dismissingCredits.has(creditKey(credit))}
                {@const badge = credit.in_library ? getWantStatusBadge(credit.want_status) : null}
                <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
                <div
                  class="credit-item"
                  class:in-library={credit.in_library}
                  class:dismissed={credit.dismissed}
                  class:clickable={!!credit.library_id}
                  role={credit.library_id ? 'button' : undefined}
                  tabindex={credit.library_id ? 0 : undefined}
                  onclick={() => handleCreditClick(credit)}
                  onkeydown={(e) => {
                    if ((e.key === 'Enter' || e.key === ' ') && credit.library_id) {
                      e.preventDefault();
                      handleCreditClick(credit);
                    }
                  }}
                >
                  <div class="credit-poster-wrap">
                    {#if credit.poster_url}
                      <img
                        class="credit-poster"
                        src={credit.poster_url}
                        alt={credit.title}
                        loading="lazy"
                      />
                    {:else}
                      <div class="credit-poster-placeholder">
                        <span class="material-icons-outlined">
                          {credit.media_type === 'movie' ? 'movie' : 'tv'}
                        </span>
                      </div>
                    {/if}
                    <span class="chip-pos-bottom-left">
                      <IconChip chip={mediaTypeChip(credit.media_type)} />
                    </span>
                  </div>

                  <div class="credit-details">
                    <div class="credit-title-row">
                      <span class="credit-title">{credit.title}</span>
                      {#if credit.year}
                        <span class="credit-year">{credit.year}</span>
                      {/if}
                    </div>
                    <div class="credit-role">
                      {credit.roles.join(', ')}
                    </div>
                  </div>

                  <div class="credit-status">
                    {#if credit.in_library && badge}
                      <span class="status-pill {badge.class}">{badge.text}</span>
                    {/if}
                  </div>

                  <div class="credit-actions">
                    {#if credit.dismissed}
                      <button
                        type="button"
                        class="action-btn undismiss"
                        disabled={isDismissing}
                        onclick={(e) => {
                          e.stopPropagation();
                          handleUndismiss(credit);
                        }}
                        title="Restore"
                      >
                        <span class="material-icons-outlined">undo</span>
                      </button>
                    {:else if !credit.in_library}
                      <button
                        type="button"
                        class="action-btn want"
                        disabled={isWanting}
                        onclick={(e) => {
                          e.stopPropagation();
                          handleWant(credit);
                        }}
                        title="Want"
                      >
                        {isWanting ? '...' : 'Want'}
                      </button>
                      <button
                        type="button"
                        class="action-btn dismiss"
                        disabled={isDismissing}
                        onclick={(e) => {
                          e.stopPropagation();
                          handleDismiss(credit);
                        }}
                        title="Dismiss"
                      >
                        <span class="material-icons-outlined">close</span>
                      </button>
                    {/if}
                  </div>
                </div>
              {/each}
            </div>
          {/if}

          {#if dismissedCount > 0}
            <button
              type="button"
              class="show-dismissed-toggle"
              onclick={() => (showDismissed = !showDismissed)}
            >
              {showDismissed ? 'Hide' : 'Show'}
              {dismissedCount} dismissed
            </button>
          {/if}
        </div>
      </div>

      <!-- Right column: Monitoring sidebar -->
      <div class="sidebar-column">
        <div class="section card-section accent-border">
          <div class="collection-header">
            <div class="label">Monitoring</div>
            <div class="section-actions">
              <button
                type="button"
                class="icon-btn-small"
                onclick={handleWantAllMissing}
                title="Want all missing cast credits"
              >
                <span class="material-icons-outlined">playlist_add</span>
              </button>
            </div>
          </div>

          <div class="stat-row">
            <span class="stat-text"
              >{libraryMovieCount} movies · {libraryTvCount} TV shows in library</span
            >
          </div>

          <div class="auto-want-row">
            <span class="auto-want-label">Auto-want</span>
            <div class="segmented-toggle" class:disabled={updatingAutoWant}>
              {#each autoWantModes as mode (mode.value)}
                <button
                  type="button"
                  class="segment"
                  class:active={effectiveAutoWantMode === mode.value}
                  disabled={updatingAutoWant}
                  onclick={() => handleAutoWantModeChange(mode.value)}
                >
                  {mode.label}
                </button>
              {/each}
            </div>
          </div>

          <div class="monitoring-note">
            {#if effectiveAutoWantMode === 'none'}
              Auto-want is off. New credits won't be automatically wanted.
            {:else if effectiveAutoWantMode === 'new_only'}
              New acting credits released after monitoring started will be auto-wanted.
            {:else if effectiveAutoWantMode === 'all'}
              All acting credits not in library will be auto-wanted.
            {/if}
          </div>
        </div>
      </div>
    </div>
  </ContentPageView>
{/if}

<style>
  .loading-state,
  .error-state {
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 200px;
    padding: 24px;
    color: var(--muted);
  }

  .edit-container {
    max-width: 800px;
    margin: 0 auto;
    padding: 24px;
  }

  .edit-container h2 {
    margin: 0 0 24px;
    font-size: 20px;
    font-weight: 700;
  }

  /* ── Hero extras ── */
  .hero-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }

  .department-label {
    font-weight: 600;
    color: var(--accent);
  }

  .meta-sep {
    color: var(--muted);
    opacity: 0.5;
  }

  .dates-text,
  .place-text {
    color: var(--muted);
    font-size: 14px;
  }

  .hero-stat {
    font-size: 13px;
  }

  .hero-identifiers {
    margin-top: 4px;
  }

  .monitored-pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    background: rgba(139, 92, 246, 0.15);
    border: 1px solid var(--monitored);
    color: var(--monitored);
    padding: 6px 14px;
    border-radius: 100px;
    font-size: 12px;
    font-weight: 700;
    cursor: pointer;
    transition: all 0.2s;
  }

  .monitored-pill:hover {
    background: rgba(139, 92, 246, 0.25);
    transform: translateY(-1px);
  }

  .monitored-pill.inactive {
    background: var(--bg-2);
    border-color: var(--ring);
    color: var(--muted);
  }

  .monitored-pill :global(.material-icons-outlined) {
    font-size: 16px;
  }

  /* ── Main grid ── */
  .main-grid {
    display: grid;
    grid-template-columns: 1fr 400px;
    gap: 32px;
    align-items: start;
  }

  .filmography-column,
  .sidebar-column {
    display: flex;
    flex-direction: column;
    gap: 24px;
  }

  .card-section {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 24px;
  }

  .card-section.accent-border {
    border-color: var(--accent);
    background: linear-gradient(to bottom right, var(--card), var(--bg-2));
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .label {
    font-size: 11px;
    font-weight: 800;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.1em;
  }

  /* ── Section header with filters ── */
  .section-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
  }

  .filter-controls {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }

  .filter-group {
    display: flex;
    background: var(--bg-2);
    border: 1px solid var(--ring);
    border-radius: 8px;
    overflow: hidden;
  }

  .filter-chip {
    padding: 5px 12px;
    font-size: 11px;
    font-weight: 600;
    background: transparent;
    border: none;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
    white-space: nowrap;
  }

  .filter-chip:not(:last-child) {
    border-right: 1px solid var(--ring);
  }

  .filter-chip:hover:not(.active) {
    background: var(--ring);
    color: var(--ink);
  }

  .filter-chip.active {
    background: var(--accent);
    color: #1d130c;
  }

  /* ── Filmography credit list ── */
  .filmography-loading,
  .empty-filmography {
    padding: 32px 0;
    text-align: center;
    color: var(--muted);
    font-size: 13px;
  }

  .credit-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .credit-item {
    display: grid;
    grid-template-columns: 56px 1fr auto auto;
    gap: 12px;
    padding: 10px;
    border-radius: 10px;
    align-items: center;
    background: var(--bg-2);
    border: 1px solid transparent;
    transition: all 0.15s;
  }

  .credit-item.clickable {
    cursor: pointer;
  }

  .credit-item.clickable:hover {
    background: var(--card);
    border-color: var(--ring);
    transform: translateX(4px);
  }

  .credit-item.dismissed {
    opacity: 0.5;
  }

  .credit-poster-wrap {
    position: relative;
    width: 56px;
    flex-shrink: 0;
  }

  .credit-poster {
    display: block;
    width: 56px;
    height: 84px;
    border-radius: 8px;
    object-fit: cover;
    overflow: hidden;
    background: var(--bg-2);
    font-size: 10px;
    color: var(--muted);
  }

  .credit-poster-placeholder {
    width: 56px;
    height: 84px;
    border-radius: 8px;
    background: var(--bg);
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
  }

  .credit-poster-placeholder :global(.material-icons-outlined) {
    font-size: 20px;
    color: var(--muted);
  }

  .chip-pos-bottom-left {
    position: absolute;
    bottom: 4px;
    left: 4px;
  }

  .credit-details {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .credit-title-row {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
  }

  .credit-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }

  .credit-year {
    font-size: 12px;
    color: var(--muted);
    flex-shrink: 0;
  }

  .credit-role {
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .credit-status {
    flex-shrink: 0;
  }

  .status-pill {
    font-size: 10px;
    font-weight: 800;
    padding: 3px 8px;
    border-radius: 6px;
    border: 1px solid var(--ring);
    color: var(--muted);
    text-transform: uppercase;
    white-space: nowrap;
  }

  .status-pill.owned {
    border-color: rgba(34, 197, 94, 0.3);
    color: #4ade80;
    background: rgba(34, 197, 94, 0.1);
  }

  .status-pill.wanted {
    border-color: rgba(234, 179, 8, 0.3);
    color: var(--wanted);
    background: rgba(234, 179, 8, 0.1);
  }

  .status-pill.searching,
  .status-pill.downloading {
    border-color: rgba(96, 165, 250, 0.3);
    color: #60a5fa;
    background: rgba(96, 165, 250, 0.1);
  }

  .credit-actions {
    display: flex;
    gap: 6px;
    flex-shrink: 0;
  }

  .action-btn {
    font-size: 11px;
    font-weight: 700;
    padding: 6px 12px;
    border-radius: 100px;
    cursor: pointer;
    transition: all 0.2s;
    white-space: nowrap;
  }

  .action-btn.want {
    border: 1px solid var(--accent);
    background: transparent;
    color: var(--accent);
  }

  .action-btn.want:hover:not(:disabled) {
    background: var(--accent);
    color: #1d130c;
  }

  .action-btn.dismiss {
    width: 32px;
    height: 32px;
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 50%;
    background: transparent;
    color: var(--muted);
  }

  .action-btn.dismiss :global(.material-icons-outlined) {
    font-size: 16px;
  }

  .action-btn.dismiss:hover:not(:disabled) {
    background: var(--ring);
    color: var(--ink);
  }

  .action-btn.undismiss {
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .action-btn.undismiss :global(.material-icons-outlined) {
    font-size: 14px;
  }

  .action-btn.undismiss:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }

  .action-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .show-dismissed-toggle {
    font-size: 12px;
    font-weight: 600;
    color: var(--muted);
    background: none;
    border: none;
    cursor: pointer;
    padding: 8px 0;
    text-align: center;
  }

  .show-dismissed-toggle:hover {
    color: var(--ink);
  }

  /* ── Sidebar ── */
  .collection-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .section-actions {
    display: flex;
    gap: 8px;
  }

  .icon-btn-small {
    width: 36px;
    height: 36px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 10px;
    background: var(--bg-2);
    color: var(--muted);
    cursor: pointer;
    transition: all 0.2s;
  }

  .icon-btn-small:hover:not(:disabled) {
    background: var(--ring);
    color: var(--ink);
  }

  .stat-row {
    padding: 4px 0;
  }

  .stat-text {
    font-size: 13px;
    color: var(--muted);
    font-weight: 500;
  }

  .auto-want-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding-top: 12px;
    border-top: 1px solid var(--ring);
  }

  .auto-want-label {
    font-size: 12px;
    font-weight: 700;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    white-space: nowrap;
  }

  .segmented-toggle {
    display: flex;
    background: var(--bg-2);
    border: 1px solid var(--ring);
    border-radius: 8px;
    overflow: hidden;
  }

  .segmented-toggle.disabled {
    opacity: 0.6;
    pointer-events: none;
  }

  .segment {
    padding: 6px 14px;
    font-size: 12px;
    font-weight: 600;
    background: transparent;
    border: none;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
    white-space: nowrap;
  }

  .segment:not(:last-child) {
    border-right: 1px solid var(--ring);
  }

  .segment:hover:not(:disabled):not(.active) {
    background: var(--ring);
    color: var(--ink);
  }

  .segment.active {
    background: var(--accent);
    color: #1d130c;
  }

  .monitoring-note {
    font-size: 12px;
    color: var(--muted);
    line-height: 1.5;
    padding-top: 8px;
    opacity: 0.8;
  }

  /* ── Responsive ── */
  @media (max-width: 1100px) {
    .main-grid {
      grid-template-columns: 1fr;
    }

    .sidebar-column {
      order: -1;
    }
  }

  @media (max-width: 640px) {
    .section-header {
      flex-direction: column;
      gap: 8px;
    }

    .credit-item {
      grid-template-columns: 48px 1fr auto;
    }

    .credit-status {
      display: none;
    }

    .credit-poster,
    .credit-poster-placeholder {
      width: 48px;
      height: 72px;
    }
  }
</style>
