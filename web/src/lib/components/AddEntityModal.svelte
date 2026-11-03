<script lang="ts">
  /**
   * AddEntityModal - Orchestrates the add entity flow.
   *
   * Standard flow: SEARCH → EDIT → DONE
   *   - Opens MetadataSearchModal for entity type
   *   - On select: merge metadata (books), show create form with data
   *   - On skip: show create form with empty data
   *   - On form submit: call create API
   *   - On success: call onComplete with new entity ID
   *
   * Link mode: SEARCH → DONE (no edit step)
   *   - Opens MetadataSearchModal with hideManual=true
   *   - On select: extract identifiers, update existing entity, call onComplete
   *   - Used for linking metadata sources to existing books
   */
  import type { MetadataResult } from '#lib/api/client.js';
  import type { AuthorSearchResult, SeriesSearchResult } from '#lib/types.js';
  import MetadataSearchModal, {
    type SearchSavedState,
    type ProviderInfo
  } from './MetadataSearchModal.svelte';
  import type { BookFormData, AuthorFormData, SeriesFormData } from '#lib/types.js';
  import BookForm from './BookForm.svelte';
  import AuthorEditForm from './AuthorEditForm.svelte';
  import SeriesEditForm from './SeriesEditForm.svelte';
  import * as api from '#lib/api/client.js';

  export type EntityMode = 'book' | 'author' | 'series';
  type FlowState = 'search' | 'edit';

  interface Props {
    mode: EntityMode;
    /** Link mode: skip edit step, just extract identifiers and update existing entity */
    linkMode?: boolean;
    /** ID of existing entity to link (required when linkMode=true without onResultsSelected) */
    existingId?: string;
    /** Initial search values to pre-populate the search form */
    initialSearchValues?: { title?: string; author?: string };
    onComplete: (id: string) => void;
    onCancel: () => void;
    /** Alternative callback for link mode: return selected results instead of updating */
    onResultsSelected?: (
      results: MetadataResult[] | AuthorSearchResult[] | SeriesSearchResult[]
    ) => void;
  }

  let {
    mode,
    linkMode = false,
    existingId,
    initialSearchValues,
    onComplete,
    onCancel,
    onResultsSelected
  }: Props = $props();

  // Flow state
  let flowState = $state<FlowState>('search');
  let saving = $state(false);
  let error = $state<string | null>(null);

  // Data from search (depends on mode)
  let _bookData = $state<MetadataResult | null>(null);
  let bookSearchResults = $state<MetadataResult[]>([]);
  let bookProviders = $state<ProviderInfo[]>([]);
  let authorData = $state<AuthorSearchResult | null>(null);
  let seriesData = $state<SeriesSearchResult | null>(null);

  // Preserved search state for when user goes back from edit to search
  let savedSearchState = $state<SearchSavedState | undefined>(undefined);
  let searchModalRef = $state<{ getState: () => SearchSavedState } | null>(null);

  const modalTitle = $derived(() => {
    if (linkMode) {
      switch (mode) {
        case 'book':
          return 'Link Metadata';
        case 'author':
          return 'Link Author';
        case 'series':
          return 'Link Series';
      }
    }
    switch (mode) {
      case 'book':
        return flowState === 'search' ? 'Add Book' : 'Create Book';
      case 'author':
        return flowState === 'search' ? 'Add Author' : 'Create Author';
      case 'series':
        return flowState === 'search' ? 'Add Series' : 'Create Series';
    }
  });

  // Extract identifiers from metadata results
  function extractIdentifiers(results: MetadataResult[]): Record<string, string> {
    const identifiers: Record<string, string> = {};
    for (const result of results) {
      // Add provider-specific external ID
      if (result.external_id && result.provider) {
        identifiers[result.provider] = result.external_id;
      }
      // Add any other identifiers from the result
      if (result.identifiers) {
        Object.assign(identifiers, result.identifiers);
      }
    }
    return identifiers;
  }

  // Handle search selection
  async function handleSearchSelect(
    results: MetadataResult[] | AuthorSearchResult[] | SeriesSearchResult[],
    providers: ProviderInfo[],
    merged?: MetadataResult
  ) {
    // Link mode: skip edit step, return results via callback or update directly
    if (linkMode) {
      // If callback provided, return results for caller to handle
      if (onResultsSelected) {
        onResultsSelected(results);
        return;
      }

      // Otherwise, update the existing entity directly (books only for now)
      if (existingId && mode === 'book') {
        const bookResults = results as MetadataResult[];
        const identifiers = extractIdentifiers(bookResults);
        saving = true;
        error = null;
        try {
          await api.updateBook(existingId, { identifiers });
          onComplete(existingId);
        } catch (e) {
          console.error('Failed to link metadata:', e);
          error = e instanceof Error ? e.message : 'Failed to link metadata';
          saving = false;
        }
      }
      return;
    }

    // Standard flow: save search state and transition to edit
    if (searchModalRef) {
      savedSearchState = searchModalRef.getState();
    }

    if (mode === 'book') {
      // For books, we keep raw results for source-aware fields (merged data not needed)
      _bookData = merged || (results[0] as MetadataResult) || null;
      bookSearchResults = results as MetadataResult[];
      bookProviders = providers;
    } else if (mode === 'author') {
      // For authors, take the first result
      authorData = (results[0] as AuthorSearchResult) || null;
    } else if (mode === 'series') {
      // For series, take the first result
      seriesData = (results[0] as SeriesSearchResult) || null;
    }
    flowState = 'edit';
  }

  // Handle skip to custom entry
  function handleSkip() {
    // Clear any data and go to edit mode
    _bookData = null;
    bookSearchResults = [];
    bookProviders = [];
    authorData = null;
    seriesData = null;
    flowState = 'edit';
  }

  // Handle book save (with optional cover file upload)
  async function handleBookSave(data: BookFormData, coverFile?: File) {
    saving = true;
    error = null;

    try {
      // Build identifiers if provided
      let identifiers: Record<string, string> | undefined;
      if (data.identifiers && Object.keys(data.identifiers).length > 0) {
        identifiers = data.identifiers;
      }

      // Create the book first
      const result = await api.createBook({
        title: data.title,
        authors: data.authors.length > 0 ? data.authors : undefined,
        series_id: data.series_id || undefined,
        series_name: data.series_name || undefined,
        series_index: data.series_index ?? undefined,
        publisher: data.publisher || undefined,
        pubdate: data.pubdate || undefined,
        description: data.description || undefined,
        language: data.language || undefined,
        page_count: data.page_count ?? undefined,
        identifiers,
        genres: data.genres,
        rating: data.rating ?? undefined,
        cover_url: data.cover_url || undefined,
        want: data.want ?? true
      });

      // If there's a cover file, upload it after book creation
      if (coverFile) {
        try {
          await api.uploadBookCover(result.id, coverFile);
        } catch (coverErr) {
          console.error('Failed to upload cover:', coverErr);
          // Don't fail the whole operation if cover upload fails
          // Book was created successfully
        }
      }

      onComplete(result.id);
    } catch (e) {
      console.error('Failed to create book:', e);
      error = e instanceof Error ? e.message : 'Failed to create book';
      saving = false;
    }
  }

  // Handle author save
  async function handleAuthorSave(data: AuthorFormData) {
    saving = true;
    error = null;

    try {
      const result = await api.createAuthor({
        name: data.name,
        identifiers: data.identifiers,
        image_url: data.image_url,
        bio: data.bio,
        is_monitored: data.is_monitored,
        auto_want_mode: data.auto_want_mode
      });

      onComplete(result.id);
    } catch (e) {
      console.error('Failed to create author:', e);
      error = e instanceof Error ? e.message : 'Failed to create author';
      saving = false;
    }
  }

  // Handle series save
  async function handleSeriesSave(data: SeriesFormData) {
    saving = true;
    error = null;

    try {
      const result = await api.createSeries({
        name: data.name,
        identifiers: data.identifiers,
        image_url: data.image_url,
        description: data.description,
        total_book_count: data.total_book_count,
        is_monitored: data.is_monitored,
        auto_want_mode: data.auto_want_mode
      });

      onComplete(result.id);
    } catch (e) {
      console.error('Failed to create series:', e);
      error = e instanceof Error ? e.message : 'Failed to create series';
      saving = false;
    }
  }

  // Handle cancel at any stage
  function handleCancel() {
    onCancel();
  }

  // Handle going back from edit to search
  function handleBackToSearch() {
    flowState = 'search';
    error = null;
  }
</script>

<div
  class="modal-backdrop"
  role="presentation"
  onclick={handleCancel}
  onkeydown={(e) => e.key === 'Escape' && handleCancel()}
>
  <div
    class="modal-container"
    role="presentation"
    onclick={(e) => e.stopPropagation()}
    onkeydown={() => {}}
  >
    <div class="modal-header">
      <h2 class="modal-title">{modalTitle()}</h2>
      <button class="close-btn" aria-label="Close" onclick={handleCancel}>
        <span class="material-icons-outlined">close</span>
      </button>
    </div>

    {#if error}
      <div class="error-banner">
        <span class="material-icons-outlined">error</span>
        <span>{error}</span>
        <button aria-label="Close error" onclick={() => (error = null)}>
          <span class="material-icons-outlined">close</span>
        </button>
      </div>
    {/if}

    <div class="modal-content">
      {#if flowState === 'search'}
        <MetadataSearchModal
          bind:this={searchModalRef}
          {mode}
          embedded={true}
          savedState={savedSearchState}
          initialValues={initialSearchValues}
          hideManual={linkMode}
          onSelect={handleSearchSelect}
          onSkip={handleSkip}
          onCancel={handleCancel}
        />
      {:else if flowState === 'edit'}
        {#if mode === 'book'}
          <BookForm
            searchResults={bookSearchResults}
            providers={bookProviders}
            onSave={handleBookSave}
            onCancel={handleBackToSearch}
            {saving}
          />
        {:else if mode === 'author'}
          <AuthorEditForm
            initialData={authorData
              ? {
                  name: authorData.name,
                  image_url: authorData.image_url,
                  identifiers:
                    authorData.external_id && authorData.provider
                      ? { [authorData.provider]: authorData.external_id }
                      : undefined,
                  is_monitored: true,
                  auto_want_mode: 'none'
                }
              : undefined}
            searchResult={authorData}
            onSave={handleAuthorSave}
            onCancel={handleBackToSearch}
            {saving}
          />
        {:else if mode === 'series'}
          <SeriesEditForm
            initialData={seriesData
              ? {
                  name: seriesData.name,
                  image_url: seriesData.image_url,
                  description: seriesData.description,
                  total_book_count: seriesData.book_count,
                  identifiers:
                    seriesData.external_id && seriesData.provider
                      ? { [seriesData.provider]: seriesData.external_id }
                      : undefined,
                  is_monitored: true,
                  auto_want_mode: 'inherit'
                }
              : undefined}
            searchResult={seriesData}
            onSave={handleSeriesSave}
            onCancel={handleBackToSearch}
            {saving}
          />
        {/if}
      {/if}
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
    padding: 20px;
  }

  .modal-container {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    width: 100%;
    max-width: 950px;
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    /* Don't clip - let dropdown menus render outside bounds */
  }

  @media (max-width: 768px) {
    .modal-container {
      max-width: 100%;
      max-height: 100%;
      border-radius: 0;
      height: 100%;
    }

    .modal-backdrop {
      padding: 0;
    }
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 16px;
    border-bottom: 1px solid var(--ring);
  }

  .modal-title {
    font-size: 15px;
    font-weight: 600;
    margin: 0;
  }

  .close-btn {
    width: 28px;
    height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: none;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    border-radius: 6px;
  }

  .close-btn:hover {
    background: var(--bg-2);
    color: var(--ink);
  }

  .error-banner {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 16px;
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
    font-size: 13px;
  }

  .error-banner button {
    margin-left: auto;
    border: none;
    background: transparent;
    color: var(--danger);
    cursor: pointer;
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .error-banner .material-icons-outlined {
    font-size: 18px;
  }

  .modal-content {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    display: flex;
    flex-direction: column;
  }

  /* Ensure nested modals/forms fill the container properly */
  .modal-content :global(> *) {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0; /* Allow flex children to shrink */
  }
</style>
