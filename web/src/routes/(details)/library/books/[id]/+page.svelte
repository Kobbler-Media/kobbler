<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import BookFullPageView from '#lib/components/BookFullPageView.svelte';
  import BookForm from '#lib/components/BookForm.svelte';
  import { type BookFormData } from '#lib/types.js';
  import AddEntityModal from '#lib/components/AddEntityModal.svelte';
  import QuickLinkProviderModal from '#lib/components/QuickLinkProviderModal.svelte';
  import { DeleteConfirmModal } from '@koblibs/ui';
  import ReleaseSearchModal from '#lib/components/ReleaseSearchModal.svelte';
  import ChapterEditor from '#lib/components/ChapterEditor.svelte';
  import ContentPageView from '#lib/components/ContentPageView.svelte';
  import { loadShelves, showMessage } from '#lib/context/app.svelte.js';
  import { getStatsState } from '#lib/stores/stats.svelte.js';
  import * as api from '#lib/api/client.js';
  import { searchBookReleases, queueBookRelease } from '#lib/api/client.js';
  import type { MetadataResult } from '#lib/api/client.js';
  import type { ProviderInfo } from '#lib/components/MetadataSearchModal.svelte';
  import { getProviderCode, getProviderColor, getProviderName } from '#lib/utils/format.js';
  import type { Book, AuthorSearchResult, SeriesSearchResult } from '#lib/types.js';

  // Load data from +page.ts
  let { data } = $props();

  // State
  let book = $state<Book | null>(null);
  let editMode = $state(false);
  let saving = $state(false);
  let saveError = $state<string | undefined>(undefined);
  let deleting = $state(false);
  let showLinkMetadataModal = $state(false);
  let showLinkHardcoverModal = $state(false);
  let showDeleteConfirm = $state(false);
  let showReleaseSearch = $state(false);
  let showChapterEditor = $state(false);

  // Sync state with data from prop
  $effect(() => {
    if (data.book) {
      book = data.book;
    }
  });

  // Search results for edit mode (populated when user searches metadata)
  let editSearchResults = $state<MetadataResult[]>([]);
  let editProviders = $state<ProviderInfo[]>([]);

  const stats = getStatsState();
  const bookId = $derived(page.params.id ?? '');

  // Navigation state for detail page stack
  type DetailState = { fromDetail?: boolean; closeUrl?: string; editing?: boolean };
  const pageState = $derived(page.state as DetailState);

  // Show back button if we navigated here from another detail page
  const canGoBack = $derived(pageState?.fromDetail === true);

  // Close URL - use passed state or default to library with this book selected
  const defaultCloseUrl = $derived.by(() => {
    let ownership = 'all';
    if (book?.is_in_library) {
      ownership = 'owned';
    } else if (
      book?.want_status &&
      ['wanted', 'searching', 'downloading'].includes(book.want_status)
    ) {
      ownership = 'wanted';
    }
    return `/library?around=${bookId}&book=${bookId}&ownership=${ownership}`;
  });
  const closeUrl = $derived(pageState?.closeUrl || defaultCloseUrl);

  // Load shelves once on mount
  onMount(() => {
    if (pageState?.editing) {
      editMode = true;
    }
    loadShelves();
  });

  // Handle initial error
  $effect(() => {
    if (data.error && !book) {
      showMessage(data.error);
      goto('/library');
    }
  });

  // Reload book when navigating to a different book within the details group
  $effect(() => {
    if (data.book && data.book.id !== book?.id) {
      book = data.book;
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
    editSearchResults = [];
    editProviders = [];
  };

  const handleSave = async (data: BookFormData, coverFile?: File) => {
    if (!book) return;

    saving = true;
    saveError = undefined;
    try {
      await api.updateBook(book.id, {
        title: data.title,
        authors: data.authors,
        series_id: data.series_id ?? undefined,
        series_name: data.series_name ?? undefined,
        series_index: data.series_index ?? undefined,
        publisher: data.publisher ?? undefined,
        pubdate: data.pubdate ?? undefined,
        genres: data.genres,
        description: data.description ?? undefined,
        identifiers: data.identifiers,
        series_memberships: data.series_memberships
      });

      if (coverFile) {
        await api.uploadBookCover(book.id, coverFile);
      }

      showMessage('Book updated');
      editMode = false;
      book = await api.getBook(bookId);
    } catch (e) {
      saveError = e instanceof Error ? e.message : 'Update failed';
    } finally {
      saving = false;
    }
  };

  const handleDelete = async () => {
    if (!book) return;

    deleting = true;
    try {
      await api.deleteBook(book.id);
      showMessage('Book deleted');
      goto('/library');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete book');
    } finally {
      deleting = false;
      showDeleteConfirm = false;
    }
  };

  const handleRefresh = async () => {
    if (!book) return;
    book = await api.getBook(bookId);
  };

  const handleForceResync = async () => {
    if (!book) return;
    try {
      await api.touchBooks([book.id]);
      showMessage('Book will be re-synced');
      book = await api.getBook(bookId);
    } catch {
      showMessage('Force resync failed');
    }
  };

  const handleMetadataResultsSelected = (
    results: MetadataResult[] | AuthorSearchResult[] | SeriesSearchResult[]
  ) => {
    showLinkMetadataModal = false;
    const bookResults = results as MetadataResult[];

    const providerMap = new Map<string, ProviderInfo>();
    for (const result of bookResults) {
      if (!providerMap.has(result.provider)) {
        providerMap.set(result.provider, {
          id: result.provider,
          name: getProviderName(result.provider),
          code: getProviderCode(result.provider),
          color: getProviderColor(result.provider),
          priority: providerMap.size
        });
      }
    }

    editSearchResults = bookResults;
    editProviders = Array.from(providerMap.values());

    const identifiers: Record<string, string> = {};
    for (const result of bookResults) {
      if (result.external_id && result.provider) {
        identifiers[result.provider] = result.external_id;
      }
      if (result.identifiers) {
        Object.assign(identifiers, result.identifiers);
      }
    }

    if (book && Object.keys(identifiers).length > 0) {
      book = { ...book, identifiers: { ...book.identifiers, ...identifiers } };
    }

    showMessage(`Loaded metadata from ${bookResults.length} source(s)`);
  };

  const handleLinkHardcover = async (hardcoverId: string) => {
    if (!book) return;
    const identifiers = { ...book.identifiers, hardcover: hardcoverId };
    await api.updateBook(book.id, { identifiers });
    showMessage('Hardcover linked successfully');
    book = await api.getBook(bookId);
  };

  const handleAuthorClick = (_authorName: string, authorId: string) => {
    goto(`/authors/${authorId}`, {
      state: { fromDetail: true, closeUrl: closeUrl || defaultCloseUrl }
    });
  };

  const handleSeriesClick = (seriesId: string) => {
    goto(`/series/${seriesId}`, {
      state: { fromDetail: true, closeUrl: closeUrl || defaultCloseUrl }
    });
  };

  const handleGenreClick = (genre: string) => {
    goto(`/library?q=${encodeURIComponent(genre)}`);
  };

  const handleSearchReleases = () => {
    showReleaseSearch = true;
  };

  // State for external form control
  let triggerSave = $state<(() => void) | undefined>(undefined);
  let formValid = $state(false);
</script>

<svelte:head>
  <title>{book?.title ?? 'Book'} — Kobbler</title>
</svelte:head>

{#if book}
  {#if editMode}
    <ContentPageView
      noHeader={true}
      noPadding={true}
      pageBackdropUrl={book.cover_url}
      onClose={handleClose}
    >
      {#snippet headerActions()}
        <button
          type="button"
          class="glass-btn"
          onclick={() => (showLinkMetadataModal = true)}
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
          <h2 class="edit-hero-title">Editing: {book.title}</h2>
        </div>
      </div>

      <div class="page-container">
        <BookForm
          {book}
          searchResults={editSearchResults}
          providers={editProviders}
          hideFooter
          bind:requestSave={triggerSave}
          bind:canSave={formValid}
          onSave={handleSave}
          onCancel={handleEditCancel}
          onDelete={() => (showDeleteConfirm = true)}
          {saving}
          {deleting}
          error={saveError}
        />
      </div>

      {#snippet footer()}
        <div class="edit-footer">
          <button
            type="button"
            class="btn btn-danger"
            onclick={() => (showDeleteConfirm = true)}
            disabled={deleting || saving}
          >
            <span class="material-icons-outlined">delete</span>
            {deleting ? 'Deleting...' : 'Delete Book'}
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
    <BookFullPageView
      {book}
      {canGoBack}
      onBack={handleBack}
      onClose={handleClose}
      onEdit={handleEdit}
      onForceResync={handleForceResync}
      onRefresh={handleRefresh}
      onSearchReleases={stats.stats.kobload_configured ? handleSearchReleases : undefined}
      onAuthorClick={handleAuthorClick}
      onSeriesClick={handleSeriesClick}
      onGenreClick={handleGenreClick}
      onEditChapters={book.has_audiobook ? () => (showChapterEditor = true) : undefined}
    />
  {/if}
{:else}
  <div class="error-container">
    <p>Book not found</p>
    <button class="btn-sec" type="button" onclick={() => goto('/library')}>Back to Library</button>
  </div>
{/if}

<!-- Delete Confirm Modal -->
{#if showDeleteConfirm && book}
  <DeleteConfirmModal
    title="Delete Book"
    message={`Are you sure you want to delete "${book.title}"? This will remove the book and all associated data (shelves, reading states, etc.). This action cannot be undone.`}
    confirmLabel={deleting ? 'Deleting...' : 'Delete Book'}
    loading={deleting}
    onCancel={() => (showDeleteConfirm = false)}
    onConfirm={handleDelete}
  />
{/if}

<!-- Link Metadata Modal (full metadata search) -->
{#if showLinkMetadataModal && book}
  <AddEntityModal
    mode="book"
    linkMode={true}
    existingId={book.id}
    initialSearchValues={{ title: book.title, author: book.authors?.[0]?.name }}
    onComplete={() => (showLinkMetadataModal = false)}
    onResultsSelected={handleMetadataResultsSelected}
    onCancel={() => (showLinkMetadataModal = false)}
  />
{/if}

<!-- Quick Link Provider Modal -->
{#if showLinkHardcoverModal && book}
  <QuickLinkProviderModal
    bookTitle={book.title}
    bookAuthor={book.authors?.[0]?.name}
    provider="hardcover"
    providerName="Hardcover"
    onLink={handleLinkHardcover}
    onClose={() => (showLinkHardcoverModal = false)}
  />
{/if}

<!-- Chapter Editor Modal -->
{#if showChapterEditor && book}
  <ChapterEditor
    bookId={book.id}
    onClose={() => (showChapterEditor = false)}
    onSaved={handleRefresh}
  />
{/if}

<!-- Release Search Modal -->
{#if showReleaseSearch && book}
  <ReleaseSearchModal
    entityTitle={book.title}
    searchFn={(query, scope) => searchBookReleases(book!.id, query, scope)}
    scopeLabel="book"
    smartSearchFn={() => searchBookReleases(book!.id)}
    queueFn={(release, source) =>
      queueBookRelease(book!.id, release as Record<string, unknown>, source)}
    wantFn={async () => {
      await api.wantBook(book!.id);
      showMessage(`Searching for best match for "${book!.title}"`);
      book = await api.getBook(bookId);
    }}
    initialQuery={`${book.title} ${book.authors?.[0]?.name || ''}`}
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
    padding-top: 8px;
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
