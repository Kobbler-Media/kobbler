<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import BookFullPageView from '$lib/components/BookFullPageView.svelte';
  import BookForm from '$lib/components/BookForm.svelte';
  import { type BookFormData } from '$lib/types';
  import AddEntityModal from '$lib/components/AddEntityModal.svelte';
  import ContentPageView from '$lib/components/ContentPageView.svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import * as api from '$lib/api/client';
  import type { MetadataResult } from '$lib/api/client';
  import type { ProviderInfo } from '$lib/components/MetadataSearchModal.svelte';
  import { getProviderCode, getProviderColor, getProviderName } from '$lib/utils/format';
  import type { Book, AuthorSearchResult, SeriesSearchResult } from '$lib/types';

  // Load data from +page.ts
  let { data } = $props();

  // State
  let item = $state<Book | null>(null);
  let editMode = $state(false);
  let saving = $state(false);
  let saveError = $state<string | undefined>(undefined);
  let loading = $state(false);

  // Metadata search state
  let showMetadataSearch = $state(false);
  let editSearchResults = $state<MetadataResult[]>([]);
  let editProviders = $state<ProviderInfo[]>([]);

  // Sync state with data from prop
  $effect(() => {
    if (data.item) {
      item = data.item;
    }
  });

  const itemId = $derived($page.params.id ?? '');

  // Navigation state for detail page stack
  type DetailState = { fromDetail?: boolean; closeUrl?: string };
  const pageState = $derived($page.state as DetailState);

  // Show back button if we navigated here from another detail page
  const canGoBack = $derived(pageState?.fromDetail === true);

  // Close URL - use passed state or default to ingest queue
  const closeUrl = $derived(pageState?.closeUrl || '/ingest-queue');

  // Handle initial error
  $effect(() => {
    if (data.error && !item) {
      showMessage(data.error);
      goto('/ingest-queue');
    }
  });

  // Reload item when navigating to a different item
  $effect(() => {
    if (data.item && data.item.id !== item?.id) {
      item = data.item;
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

  const handleSave = async (formData: BookFormData, coverFile?: File) => {
    if (!item) return;

    saving = true;
    saveError = undefined;
    try {
      await api.updateBook(item.id, {
        title: formData.title,
        authors: formData.authors,
        series_id: formData.series_id ?? undefined,
        series_name: formData.series_name ?? undefined,
        series_index: formData.series_index ?? undefined,
        publisher: formData.publisher ?? undefined,
        pubdate: formData.pubdate ?? undefined,
        genres: formData.genres,
        description: formData.description ?? undefined,
        identifiers: formData.identifiers,
        series_memberships: formData.series_memberships
      });

      // Upload cover if provided
      if (coverFile) {
        await api.uploadIngestCover(item.id, coverFile);
      }

      showMessage('Metadata updated');
      editMode = false;
      item = await api.getIngestQueueItem(itemId);
    } catch (e) {
      saveError = e instanceof Error ? e.message : 'Update failed';
    } finally {
      saving = false;
    }
  };

  const handleRefresh = async () => {
    if (!item) return;
    item = await api.getIngestQueueItem(itemId);
  };

  const handleApprove = async () => {
    if (!item) return;
    loading = true;
    try {
      await api.approveIngestItem(item.id);
      showMessage('Item approved');
      goto('/ingest-queue');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Approve failed');
    } finally {
      loading = false;
    }
  };

  const handleReject = async () => {
    if (!item) return;
    loading = true;
    try {
      await api.rejectIngestItem(item.id);
      showMessage('Item rejected');
      goto('/ingest-queue');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Reject failed');
    } finally {
      loading = false;
    }
  };

  const handleRestore = async () => {
    if (!item) return;
    loading = true;
    try {
      await api.restoreIngestQueueItem(item.id);
      showMessage('Item restored');
      item = await api.getIngestQueueItem(itemId);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Restore failed');
    } finally {
      loading = false;
    }
  };

  const handleMetadataResultsSelected = (
    results: MetadataResult[] | AuthorSearchResult[] | SeriesSearchResult[]
  ) => {
    showMetadataSearch = false;

    const bookResults = results as MetadataResult[];

    // Get provider info from results
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

    // Extract identifiers from results
    const identifiers: Record<string, string> = {};
    for (const result of bookResults) {
      if (result.external_id && result.provider) {
        identifiers[result.provider] = result.external_id;
        if (result.provider === 'hardcover') {
          identifiers['hardcover_slug'] = result.external_id;
        }
      }
      if (result.identifiers) {
        Object.assign(identifiers, result.identifiers);
      }
    }

    // Merge identifiers into the item object so they're available in the form
    if (item && Object.keys(identifiers).length > 0) {
      item = { ...item, identifiers: { ...item.identifiers, ...identifiers } };
    }

    showMessage(`Loaded metadata from ${bookResults.length} source(s)`);
  };

  const handleAuthorClick = (_authorName: string, authorId: string) => {
    goto(`/authors/${authorId}`, {
      state: { fromDetail: true, closeUrl: `/ingest-queue/${itemId}` }
    });
  };

  const handleSeriesClick = (seriesId: string) => {
    goto(`/series/${seriesId}`, {
      state: { fromDetail: true, closeUrl: `/ingest-queue/${itemId}` }
    });
  };

  const handleGenreClick = (genre: string) => {
    goto(`/library?q=${encodeURIComponent(genre)}`);
  };

  // State for external form control
  let triggerSave = $state<(() => void) | undefined>(undefined);
  let formValid = $state(false);
</script>

<svelte:head>
  <title>{item?.title ?? 'Ingest Item'} — Kobbler</title>
</svelte:head>

{#if item}
  {#if editMode}
    <ContentPageView title="Edit Book" onClose={handleClose}>
      {#snippet headerActions()}
        <button
          type="button"
          class="icon-btn"
          onclick={() => (showMetadataSearch = true)}
          title="Search Metadata"
        >
          <span class="material-icons-outlined">search</span>
        </button>
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
        <BookForm
          book={item}
          searchResults={editSearchResults}
          providers={editProviders}
          hideFooter
          bind:requestSave={triggerSave}
          bind:canSave={formValid}
          onSave={handleSave}
          onCancel={handleEditCancel}
          {saving}
          error={saveError}
        />
      </div>

      {#snippet footer()}
        <div class="action-footer">
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
    <BookFullPageView
      book={item}
      context="ingest"
      {canGoBack}
      onBack={handleBack}
      onClose={handleClose}
      onEdit={handleEdit}
      onRefresh={handleRefresh}
      onAuthorClick={handleAuthorClick}
      onSeriesClick={handleSeriesClick}
      onGenreClick={handleGenreClick}
    >
      {#snippet footer()}
        <div class="action-footer">
          {#if item?.ingest_status === 'rejected'}
            <button class="btn btn-sec" type="button" onclick={handleRestore} disabled={loading}>
              Restore
            </button>
          {:else if item}
            <button class="btn btn-danger" type="button" onclick={handleReject} disabled={loading}>
              Reject
            </button>
          {/if}
          <div class="spacer"></div>
          <button class="btn btn-primary" type="button" onclick={handleApprove} disabled={loading}>
            Approve
          </button>
        </div>
      {/snippet}
    </BookFullPageView>
  {/if}
{:else}
  <div class="error-container">
    <p>Item not found</p>
    <button class="btn btn-sec" type="button" onclick={() => goto('/ingest-queue')}
      >Back to Ingest Queue</button
    >
  </div>
{/if}

<!-- Metadata Search Modal -->
{#if showMetadataSearch && item}
  <AddEntityModal
    mode="book"
    linkMode={true}
    existingId={item.id}
    initialSearchValues={{
      title: item.title || item.ingest_info?.file_name || '',
      author: item.authors?.[0]?.name
    }}
    onComplete={() => (showMetadataSearch = false)}
    onResultsSelected={handleMetadataResultsSelected}
    onCancel={() => (showMetadataSearch = false)}
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

  .action-footer {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
  }

  .spacer {
    flex: 1;
  }

  @media (max-width: 768px) {
    .action-footer {
      gap: 8px;
    }
  }
</style>
