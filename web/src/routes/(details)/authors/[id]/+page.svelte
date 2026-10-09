<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import AuthorFullPageView from '$lib/components/AuthorFullPageView.svelte';
  import AuthorEditForm from '$lib/components/AuthorEditForm.svelte';
  import { type AuthorFormData } from '$lib/types';
  import ReleaseSearchModal from '$lib/components/ReleaseSearchModal.svelte';
  import ContentPageView from '$lib/components/ContentPageView.svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import { getStatsState } from '$lib/stores/stats.svelte';
  import * as api from '$lib/api/client';
  import { hasOutstandingBookWants } from '$lib/utils/want-status';
  import type { AuthorSummary, Book, SeriesSummary } from '$lib/types';

  // Load data from +page.ts
  let { data } = $props();

  // State
  let author = $state<AuthorSummary | null>(null);
  let books = $state<Book[]>([]);
  let series = $state<SeriesSummary[]>([]);
  let editMode = $state(false);
  let saving = $state(false);
  let deleting = $state(false);
  let showReleaseSearch = $state(false);

  // Sync state with data from prop
  $effect(() => {
    if (data.author) {
      author = data.author;
      books = data.books;
      series = data.series;
    }
  });

  const stats = getStatsState();
  const authorId = $derived($page.params.id ?? '');

  // Navigation state for detail page stack
  type DetailState = { fromDetail?: boolean; closeUrl?: string };
  const pageState = $derived($page.state as DetailState);

  // Show back button if we navigated here from another detail page
  const canGoBack = $derived(pageState?.fromDetail === true);

  // Close URL - use passed state or default to authors list with this author selected
  const defaultCloseUrl = $derived(`/authors?around=${authorId}&selected=${authorId}`);
  const closeUrl = $derived(pageState?.closeUrl || defaultCloseUrl);

  // Handle initial error
  $effect(() => {
    if (data.error && !author) {
      showMessage(data.error);
      goto('/authors');
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

  const handleSave = async (data: AuthorFormData) => {
    if (!author) return;

    saving = true;
    try {
      await api.updateAuthor(author.id, {
        name: data.name,
        identifiers: data.identifiers,
        image_url: data.image_url,
        bio: data.bio,
        is_monitored: data.is_monitored,
        auto_want_mode: data.auto_want_mode
      });
      showMessage('Author updated');
      editMode = false;
      author = await api.getAuthor(authorId);
    } catch {
      showMessage('Update failed');
    } finally {
      saving = false;
    }
  };

  const handleMerge = async (targetId: string) => {
    if (!author) return;

    saving = true;
    try {
      await api.mergeAuthors(author.id, targetId);
      showMessage('Authors merged');
      goto(`/authors/${targetId}`);
    } catch {
      showMessage('Merge failed');
    } finally {
      saving = false;
    }
  };

  const handleDelete = async () => {
    if (!author) return;

    deleting = true;
    try {
      await api.deleteAuthor(author.id);
      showMessage('Author deleted');
      goto('/authors');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete author');
    } finally {
      deleting = false;
    }
  };

  const handleTrack = async () => {
    if (!author) return;

    saving = true;
    try {
      await api.updateAuthorMonitoring(author.id, { is_monitored: true });
      showMessage('Author tracking enabled');
      author = await api.getAuthor(authorId);
    } catch {
      showMessage('Failed to enable tracking');
    } finally {
      saving = false;
    }
  };

  const handleUntrack = async () => {
    if (!author) return;

    saving = true;
    try {
      await api.updateAuthorMonitoring(author.id, { is_monitored: false });
      showMessage('Author tracking disabled');
      author = await api.getAuthor(authorId);
    } catch {
      showMessage('Failed to disable tracking');
    } finally {
      saving = false;
    }
  };

  const handleRefresh = async () => {
    if (!author) return;

    // If author has a hardcover ID, trigger backend metadata refresh
    const hasHardcover = author.identifiers?.hardcover;
    if (hasHardcover) {
      try {
        const result = await api.refreshAuthorMetadata(authorId);
        showMessage(result.message);
      } catch {
        showMessage('Failed to queue metadata refresh');
      }
    }

    const [authorData, booksData] = await Promise.all([
      api.getAuthor(authorId),
      api.getAuthorBooks(authorId, true)
    ]);
    author = authorData;
    books = booksData;
  };

  const handleBookClick = (book: Book) => {
    goto(`/library/books/${book.id}`, {
      state: { fromDetail: true, closeUrl: closeUrl || defaultCloseUrl }
    });
  };

  const handleSeriesClick = (seriesId: string) => {
    goto(`/series/${seriesId}`, {
      state: { fromDetail: true, closeUrl: closeUrl || defaultCloseUrl }
    });
  };

  const anyWanted = $derived(hasOutstandingBookWants(books));

  const handleUnwantAll = async () => {
    if (!author) return;
    if (!confirm('Stop looking for every book still being chased for this author?')) return;
    saving = true;
    try {
      const result = await api.unwantAllForAuthor(author.id);
      showMessage(
        `Stopped looking for ${result.unwanted_count} book${result.unwanted_count === 1 ? '' : 's'}`
      );
      books = await api.getAuthorBooks(authorId, true);
    } catch {
      showMessage('Failed to unwant books');
    } finally {
      saving = false;
    }
  };

  const handleWantAllMissing = async () => {
    if (!author) return;

    saving = true;
    try {
      const result = await api.wantAllPendingForAuthor(author.id);
      showMessage(`Marked ${result.wanted_count} missing books as wanted`);
      books = await api.getAuthorBooks(authorId, true);
    } catch {
      showMessage('Failed to mark books as wanted');
    } finally {
      saving = false;
    }
  };

  const handleSearch = () => {
    if (!author) return;
    goto(`/library?q=${encodeURIComponent(author.name)}`);
  };

  const handleSearchReleases = () => {
    showReleaseSearch = true;
  };

  // State for external form control
  let triggerSave = $state<(() => void) | undefined>(undefined);
  let formValid = $state(false);
</script>

<svelte:head>
  <title>{author?.name ?? 'Author'} — Kobbler</title>
</svelte:head>

{#if author}
  {#if editMode}
    <ContentPageView title="Edit Author" onClose={handleClose}>
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
        <AuthorEditForm
          {author}
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
            {deleting ? 'Deleting...' : 'Delete Author'}
          </button>
          <div class="spacer"></div>
          <button type="button" class="btn btn-sec" onclick={handleEditCancel} disabled={saving}>
            Cancel
          </button>
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
    <AuthorFullPageView
      {author}
      {books}
      {series}
      {canGoBack}
      onBack={handleBack}
      onClose={handleClose}
      onEdit={handleEdit}
      onTrack={handleTrack}
      onUntrack={handleUntrack}
      onRefresh={handleRefresh}
      onBookClick={handleBookClick}
      onSeriesClick={handleSeriesClick}
      onWantAllMissing={handleWantAllMissing}
      onUnwantAll={anyWanted ? handleUnwantAll : undefined}
      onSearch={handleSearch}
      onSearchReleases={stats.stats.kobload_configured ? handleSearchReleases : undefined}
    />
  {/if}
{:else}
  <div class="error-container">
    <p>Author not found</p>
    <button class="btn-sec" type="button" onclick={() => goto('/authors')}>Back to Authors</button>
  </div>
{/if}

{#if showReleaseSearch && author}
  <ReleaseSearchModal
    entityTitle={author.name}
    searchFn={(query, scope) => api.searchLibraryReleases({ q: query, scope })}
    scopeLabel="book"
    queueFn={async (release, _source) => {
      await api.downloadToLibrary({ release });
      await handleRefresh();
    }}
    initialQuery={author.name}
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
