<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import SeriesFullPageView from '#lib/components/SeriesFullPageView.svelte';
  import SeriesEditForm from '#lib/components/SeriesEditForm.svelte';
  import { type SeriesFormData } from '#lib/types.js';
  import ReleaseSearchModal from '#lib/components/ReleaseSearchModal.svelte';
  import ContentPageView from '#lib/components/ContentPageView.svelte';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { getStatsState } from '#lib/stores/stats.svelte.js';
  import * as api from '#lib/api/client.js';
  import { hasOutstandingBookWants } from '#lib/utils/want-status.js';
  import type { SeriesSummary, Book } from '#lib/types.js';

  // Load data from +page.ts
  let { data } = $props();

  // State
  let series = $state<SeriesSummary | null>(null);
  let books = $state<Book[]>([]);
  let editMode = $state(false);
  let saving = $state(false);
  let deleting = $state(false);
  let showReleaseSearch = $state(false);

  // Sync state with data from prop
  $effect(() => {
    if (data.series) {
      series = data.series;
      books = data.books;
    }
  });

  const stats = getStatsState();
  const seriesId = $derived(page.params.id ?? '');

  // Navigation state for detail page stack
  type DetailState = { fromDetail?: boolean; closeUrl?: string };
  const pageState = $derived(page.state as DetailState);

  // Show back button if we navigated here from another detail page
  const canGoBack = $derived(pageState?.fromDetail === true);

  // Close URL - use passed state or default to series list with this series selected
  const defaultCloseUrl = $derived(`/series?around=${seriesId}&selected=${seriesId}`);
  const closeUrl = $derived(pageState?.closeUrl || defaultCloseUrl);

  // Handle initial error
  $effect(() => {
    if (data.error && !series) {
      showMessage(data.error);
      goto('/series');
    }
  });

  // Reload when navigating to a different series within the details group
  $effect(() => {
    if (data.series && data.series.id !== series?.id) {
      series = data.series;
      books = data.books;
    }
  });

  const handleBack = () => {
    window.history.back();
  };

  const handleClose = () => {
    goto(closeUrl);
  };

  const handleEdit = () => {
    editMode = true;
  };

  const handleEditCancel = () => {
    editMode = false;
  };

  const handleSave = async (data: SeriesFormData) => {
    if (!series) return;

    saving = true;
    try {
      await api.updateSeries(series.id, {
        name: data.name,
        identifiers: data.identifiers,
        image_url: data.image_url,
        description: data.description,
        total_book_count: data.total_book_count,
        is_monitored: data.is_monitored,
        auto_want_mode: data.auto_want_mode
      });
      showMessage('Series updated');
      editMode = false;
      series = await api.getSeriesById(seriesId);
    } catch {
      showMessage('Update failed');
    } finally {
      saving = false;
    }
  };

  const handleMerge = async (targetId: string) => {
    if (!series) return;

    saving = true;
    try {
      await api.mergeSeries(series.id, targetId);
      showMessage('Series merged');
      goto(`/series/${targetId}`);
    } catch {
      showMessage('Merge failed');
    } finally {
      saving = false;
    }
  };

  const handleDelete = async () => {
    if (!series) return;

    deleting = true;
    try {
      await api.deleteSeries(series.id);
      showMessage('Series deleted');
      goto('/series');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete series');
    } finally {
      deleting = false;
    }
  };

  const handleTrack = async () => {
    if (!series) return;

    saving = true;
    try {
      await api.updateSeriesMonitoring(series.id, { is_monitored: true });
      showMessage('Series tracking enabled');
      series = await api.getSeriesById(seriesId);
    } catch {
      showMessage('Failed to enable tracking');
    } finally {
      saving = false;
    }
  };

  const handleUntrack = async () => {
    if (!series) return;

    saving = true;
    try {
      await api.updateSeriesMonitoring(series.id, { is_monitored: false });
      showMessage('Series tracking disabled');
      series = await api.getSeriesById(seriesId);
    } catch {
      showMessage('Failed to disable tracking');
    } finally {
      saving = false;
    }
  };

  const handleRefresh = async () => {
    if (!series) return;

    // If series has a hardcover ID, trigger backend metadata refresh
    const hasHardcover = series.identifiers?.hardcover;
    if (hasHardcover) {
      try {
        const result = await api.refreshSeriesMetadata(seriesId);
        showMessage(result.message);
      } catch {
        showMessage('Failed to queue metadata refresh');
      }
    }

    const [seriesData, booksData] = await Promise.all([
      api.getSeriesById(seriesId),
      api.getSeriesBooks(seriesId, true)
    ]);
    series = seriesData;
    books = booksData;
  };

  const handleBookClick = (book: Book) => {
    goto(`/library/books/${book.id}`, {
      state: { fromDetail: true, closeUrl: closeUrl || defaultCloseUrl }
    });
  };

  const handleAuthorClick = (_authorName: string, authorId: string) => {
    goto(`/authors/${authorId}`, {
      state: { fromDetail: true, closeUrl: closeUrl || defaultCloseUrl }
    });
  };

  const anyWanted = $derived(hasOutstandingBookWants(books));

  const handleUnwantAll = async () => {
    if (!series) return;
    if (!confirm('Stop looking for every book still being chased for this series?')) return;
    saving = true;
    try {
      const result = await api.unwantAllForSeries(series.id);
      showMessage(
        `Stopped looking for ${result.unwanted_count} book${result.unwanted_count === 1 ? '' : 's'}`
      );
      books = await api.getSeriesBooks(seriesId, true);
    } catch {
      showMessage('Failed to unwant books');
    } finally {
      saving = false;
    }
  };

  const handleWantAllMissing = async () => {
    if (!series) return;
    saving = true;
    try {
      const result = await api.wantAllPendingForSeries(series.id);
      showMessage(`Marked ${result.wanted_count} missing books as wanted`);
      books = await api.getSeriesBooks(seriesId, true);
    } catch {
      showMessage('Failed to mark books as wanted');
    } finally {
      saving = false;
    }
  };

  const handleSearchSeries = () => {
    if (!series) return;
    goto(`/library?q=${encodeURIComponent(series.name)}`);
  };

  const handleSearchReleases = () => {
    showReleaseSearch = true;
  };

  // State for external form control
  let triggerSave = $state<(() => void) | undefined>(undefined);
  let formValid = $state(false);
</script>

<svelte:head>
  <title>{series?.name ?? 'Series'} — Kobbler</title>
</svelte:head>

{#if series}
  {#if editMode}
    <ContentPageView title="Edit Series" onClose={handleClose}>
      {#snippet headerActions()}
        <button
          type="button"
          class="icon-btn active"
          onclick={handleEditCancel}
          title="Editing (click to cancel)"
        >
          <span class="material-icons-outlined">edit</span>
        </button>
        <button type="button" class="icon-btn" onclick={handleClose} title="Close">
          <span class="material-icons-outlined">close</span>
        </button>
      {/snippet}

      <div class="edit-container">
        <SeriesEditForm
          {series}
          hideFooter
          bind:requestSave={triggerSave}
          bind:canSave={formValid}
          onSave={handleSave}
          onMerge={handleMerge}
          onDelete={handleDelete}
          onCancel={handleEditCancel}
          {saving}
          {deleting}
        />
      </div>

      {#snippet footer()}
        <div class="edit-footer">
          <button
            type="button"
            class="btn btn-danger"
            onclick={handleDelete}
            disabled={deleting || saving}
          >
            <span class="material-icons-outlined">delete</span>
            {deleting ? 'Deleting...' : 'Delete Series'}
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
  {:else}
    <SeriesFullPageView
      {series}
      {books}
      {canGoBack}
      onBack={handleBack}
      onClose={handleClose}
      onEdit={handleEdit}
      onTrack={handleTrack}
      onUntrack={handleUntrack}
      onRefresh={handleRefresh}
      onBookClick={handleBookClick}
      onAuthorClick={handleAuthorClick}
      onWantAllMissing={handleWantAllMissing}
      onUnwantAll={anyWanted ? handleUnwantAll : undefined}
      onSearchSeries={handleSearchSeries}
      onSearchReleases={stats.stats.kobload_configured ? handleSearchReleases : undefined}
    />
  {/if}
{:else}
  <div class="error-container">
    <p>Series not found</p>
    <button class="btn-sec" type="button" onclick={() => goto('/series')}>Back to Series</button>
  </div>
{/if}

{#if showReleaseSearch && series}
  <ReleaseSearchModal
    entityTitle={series.name}
    searchFn={(query, scope) => api.searchLibraryReleases({ q: query, scope })}
    scopeLabel="book"
    queueFn={async (release, _source) => {
      await api.downloadToLibrary({ release });
      await handleRefresh();
    }}
    initialQuery={series.name}
    onClose={() => (showReleaseSearch = false)}
  />
{/if}

<style>
  .error-container {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    gap: 16px;
  }

  .icon-btn {
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
  }

  .icon-btn:hover {
    background: var(--bg);
    color: var(--ink);
  }

  .icon-btn.active {
    background: var(--accent);
    border-color: var(--accent);
    color: #1d130c;
  }

  .icon-btn.active:hover {
    background: var(--accent);
    opacity: 0.9;
  }

  .icon-btn .material-icons-outlined {
    font-size: 18px;
  }

  .edit-container {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    width: 100%;
  }

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
