<script lang="ts">
  import { goto } from '$app/navigation';
  import type {
    IngestQueueGroup,
    IngestQueueStatusCounts,
    LoadState,
    SeriesSummary,
    AuthorSummary,
    AuthorSearchResult,
    SeriesSearchResult,
    DeviceSummary,
    Book
  } from '$lib/types';
  import IngestQueueCard from '$lib/components/IngestQueueCard.svelte';
  import { ExpandableSidebar } from '@koblibs/ui';
  import BookDetailsPanel from '$lib/components/BookDetailsPanel.svelte';
  import BookForm from '$lib/components/BookForm.svelte';
  import SeriesSidebarContent from '$lib/components/SeriesSidebarContent.svelte';
  import AuthorSidebarContent from '$lib/components/AuthorSidebarContent.svelte';
  import AddEntityModal from '$lib/components/AddEntityModal.svelte';
  import { createDelayedLoading } from '$lib/utils/delayed-loading.svelte';
  import {
    IngestBatchActions,
    IngestApprovalWizard,
    type BookFormData
  } from '$lib/components/ingest';
  import {
    getNavigationState,
    pushEntity,
    popEntity,
    clearStack,
    setLoadingEntity,
    updateCurrentEntityData
  } from '$lib/stores/navigation.svelte';
  import * as api from '$lib/api/client';
  import type { MetadataResult } from '$lib/api/client';
  import type { ProviderInfo } from '$lib/components/MetadataSearchModal.svelte';
  import {
    formatSize,
    getProviderCode,
    getProviderColor,
    getProviderName
  } from '$lib/utils/format';

  interface Props {
    groups: IngestQueueGroup[];
    totalGroups: number;
    hasMore: boolean;
    statusCounts?: IngestQueueStatusCounts;
    loading: boolean;
    loadState: LoadState;
    seriesList: SeriesSummary[];
    devices: DeviceSummary[];
    onApprove: (id: string, deviceIds?: string[]) => Promise<void>;
    onReject: (id: string) => Promise<void>;
    onRestore: (id: string) => Promise<void>;
    onDelete: (id: string) => Promise<void>;
    onExtractMetadata: (id: string) => Promise<void>;
    onUpdateMetadata?: (id: string, data: BookFormData, coverFile?: File) => Promise<void>;
    onBatchReject?: (ids: string[]) => Promise<void>;
    onUploadBook?: (file: File) => Promise<void>;
    onLoadMore?: () => void;
    onSearch?: (search: string) => void;
    onStatusFilter?: (status: string) => void;
  }

  let {
    groups,
    totalGroups: _totalGroups,
    hasMore,
    statusCounts,
    loading,
    loadState,
    seriesList: _seriesList,
    devices = [],
    onApprove,
    onReject,
    onRestore,
    onDelete: _onDelete,
    onExtractMetadata: _onExtractMetadata,
    onUpdateMetadata,
    onBatchReject,
    onUploadBook,
    onLoadMore,
    onSearch,
    onStatusFilter
  }: Props = $props();

  // Search state
  let searchQuery = $state('');

  // Delayed loading for initial load (prevents flash)
  const initialLoadingState = createDelayedLoading(false);
  $effect(() => {
    initialLoadingState.loading = loadState.status === 'loading';
  });

  let selectedGroupKey = $state<string | null>(null);
  let selectedItemId = $state<string | null>(null);
  let statusFilter = $state<string>('pending');
  let showEditMode = $state(false);
  let editSaving = $state(false);

  // BookForm external footer bindings
  let formRequestSave: (() => void) | undefined = $state();
  let formCanSave: boolean | undefined = $state();

  // Multi-select state
  let multiSelectMode = $state(false);
  let selectedIds = $state<Set<string>>(new Set());

  // Wizard state
  let showWizard = $state(false);
  let wizardItems = $state<Book[]>([]);
  let wizardIndex = $state(0);

  // Upload state
  let uploadFileInput: HTMLInputElement | null = $state(null);
  let uploading = $state(false);

  // Device selection state - default to all devices selected
  let selectedDeviceIds = $state<string[]>([]);
  let deviceIdsInitialized = false;

  // Navigation sidebar state (for author/series drill-down)
  const nav = getNavigationState();
  let navSeriesBooks = $state<Book[]>([]);
  let navAuthorBooks = $state<Book[]>([]);

  // Edit error state
  let editError = $state<string | undefined>(undefined);

  // Metadata search modal state
  let showMetadataSearch = $state(false);
  let editSearchResults = $state<MetadataResult[]>([]);
  let editProviders = $state<ProviderInfo[]>([]);

  // Derived navigation state
  const currentNavEntity = $derived(nav.current);
  const currentNavBook = $derived(
    currentNavEntity?.type === 'book' ? (currentNavEntity.data as Book | undefined) : undefined
  );
  const currentNavSeries = $derived(
    currentNavEntity?.type === 'series'
      ? (currentNavEntity.data as SeriesSummary | undefined)
      : undefined
  );
  const currentNavAuthor = $derived(
    currentNavEntity?.type === 'author'
      ? (currentNavEntity.data as AuthorSummary | undefined)
      : undefined
  );
  // Show nav sidebar when we have any entity in the stack (navigation mode)
  const showNavSidebar = $derived(nav.isOpen && currentNavEntity !== undefined);
  // Find the selected group and item (needed for sidebar state)
  const selectedGroup = $derived(
    selectedGroupKey ? (groups.find((g) => g.group_key === selectedGroupKey) ?? null) : null
  );
  const selectedItem = $derived(
    selectedItemId && selectedGroup
      ? (selectedGroup.items.find((i) => i.id === selectedItemId) ?? null)
      : null
  );
  // Hide the original item details sidebar when nav sidebar is active
  const showItemDetailsSidebar = $derived(!showNavSidebar && selectedItemId !== null);
  // Unified sidebar state
  const sidebarOpen = $derived(showItemDetailsSidebar || showNavSidebar);
  const sidebarTitle = $derived(
    showEditMode
      ? 'Edit Metadata'
      : showNavSidebar
        ? currentNavEntity?.type === 'book'
          ? 'Book Details'
          : currentNavEntity?.type === 'series'
            ? 'Series Details'
            : 'Author Details'
        : 'Ingest Book Details'
  );
  const showEditButton = $derived(
    showNavSidebar ? currentNavEntity?.type === 'book' : selectedItem !== null && !showEditMode
  );

  // Initialize selected devices when devices load (only once)
  $effect(() => {
    if (devices.length > 0 && !deviceIdsInitialized) {
      selectedDeviceIds = devices.map((d) => d.id);
      deviceIdsInitialized = true;
    }
  });

  // Load data when navigation stack changes
  $effect(() => {
    const entity = nav.current;
    if (!entity) return;
    if (entity.data) return; // Already have data

    // Load data based on type
    if (entity.type === 'book') {
      loadNavBookData(entity.id);
    } else if (entity.type === 'series') {
      loadNavSeriesData(entity.id);
    } else if (entity.type === 'author') {
      loadNavAuthorData(entity.id);
    }
  });

  async function loadNavBookData(bookId: string) {
    setLoadingEntity(true);
    try {
      const book = await api.getBook(bookId);
      updateCurrentEntityData(book);
    } catch (e) {
      console.error('Failed to load book:', e);
    } finally {
      setLoadingEntity(false);
    }
  }

  async function loadNavSeriesData(seriesId: string) {
    setLoadingEntity(true);
    try {
      const [series, seriesBooks] = await Promise.all([
        api.getSeriesById(seriesId),
        api.getSeriesBooks(seriesId, true)
      ]);
      updateCurrentEntityData(series);
      navSeriesBooks = seriesBooks;
    } catch (e) {
      console.error('Failed to load series:', e);
    } finally {
      setLoadingEntity(false);
    }
  }

  async function loadNavAuthorData(authorId: string) {
    setLoadingEntity(true);
    try {
      const [author, authorBooks] = await Promise.all([
        api.getAuthor(authorId),
        api.getAuthorBooks(authorId, true)
      ]);
      updateCurrentEntityData(author);
      navAuthorBooks = authorBooks;
    } catch (e) {
      console.error('Failed to load author:', e);
    } finally {
      setLoadingEntity(false);
    }
  }

  // Toggle device selection
  const handleDeviceToggle = (deviceId: string) => {
    if (selectedDeviceIds.includes(deviceId)) {
      selectedDeviceIds = selectedDeviceIds.filter((id) => id !== deviceId);
    } else {
      selectedDeviceIds = [...selectedDeviceIds, deviceId];
    }
  };

  // Navigation handlers - push to stack for sidebar navigation
  const handleAuthorClick = (_authorName: string, authorId: string) => {
    // If viewing an ingest item, push it first so we can go back to it
    if (selectedItem) {
      pushEntity('book', selectedItem.id, selectedItem);
    }
    pushEntity('author', authorId);
  };

  const handleSeriesClick = (seriesId: string) => {
    // If viewing an ingest item, push it first so we can go back to it
    if (selectedItem) {
      pushEntity('book', selectedItem.id, selectedItem);
    }
    pushEntity('series', seriesId);
  };

  const handleNavBack = () => {
    popEntity();
    navSeriesBooks = [];
    navAuthorBooks = [];
  };

  const handleNavClose = () => {
    clearStack();
    navSeriesBooks = [];
    navAuthorBooks = [];
  };

  const handleNavBookClick = (book: Book) => {
    if (book.is_in_library !== false) {
      pushEntity('book', book.id, book);
    }
  };

  // Metadata search handlers
  const handleMetadataResultsSelected = (
    results: MetadataResult[] | AuthorSearchResult[] | SeriesSearchResult[]
  ) => {
    showMetadataSearch = false;

    // In book mode, we only get MetadataResult[]
    const metadataResults = results as MetadataResult[];

    // Get provider info from results
    const providerMap = new Map<string, ProviderInfo>();
    for (const result of metadataResults) {
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

    editSearchResults = metadataResults;
    editProviders = Array.from(providerMap.values());
  };

  // Helper to get all items from all groups
  const allItems = $derived(groups.flatMap((g) => g.items));

  // Use server-provided counts when available, fallback to local counting
  const pendingCount = $derived(
    statusCounts?.pending ?? allItems.filter((i) => i.ingest_status === 'pending').length
  );
  const extractingCount = $derived(
    statusCounts?.extracting ?? allItems.filter((i) => i.ingest_status === 'extracting').length
  );
  const _convertingCount = $derived(statusCounts?.converting ?? 0);
  const approvedCount = $derived(statusCounts?.approved ?? 0);
  const rejectedCount = $derived(statusCounts?.rejected ?? 0);
  const errorCount = $derived(statusCounts?.error ?? 0);

  // Groups are filtered by the backend - no client-side filtering needed
  // When onStatusFilter is provided (required), the parent handles backend filtering
  const filteredGroups = $derived(groups);

  // File upload handlers
  const handleFileSelect = async (event: Event) => {
    const input = event.target as HTMLInputElement;
    const file = input.files?.[0];
    if (!file || !onUploadBook) return;

    uploading = true;
    try {
      await onUploadBook(file);
    } finally {
      uploading = false;
      if (uploadFileInput) uploadFileInput.value = '';
    }
  };

  // Handle status filter change
  const handleStatusFilterChange = (value: string) => {
    statusFilter = value;
    if (onStatusFilter) {
      onStatusFilter(value);
    }
  };

  // Search handler
  const handleSearchInput = (e: Event) => {
    const value = (e.target as HTMLInputElement).value;
    searchQuery = value;
    if (onSearch) {
      onSearch(value);
    }
  };

  // Multi-select helpers
  const toggleSelect = (id: string) => {
    const newSet = new Set(selectedIds);
    if (newSet.has(id)) {
      newSet.delete(id);
    } else {
      newSet.add(id);
    }
    selectedIds = newSet;
  };

  const selectAllFiltered = () => {
    selectedIds = new Set(allItems.map((i) => i.id));
  };

  const clearSelection = () => {
    selectedIds = new Set();
  };

  const toggleMultiSelectMode = () => {
    multiSelectMode = !multiSelectMode;
    if (!multiSelectMode) {
      selectedIds = new Set();
    }
  };

  const handleBatchReject = async () => {
    if (!onBatchReject || selectedIds.size === 0) return;
    await onBatchReject([...selectedIds]);
    selectedIds = new Set();
    multiSelectMode = false;
  };

  // Card select handler
  const handleCardSelect = (
    groupKey: string,
    itemId: string,
    _event?: MouseEvent | KeyboardEvent
  ) => {
    if (multiSelectMode) {
      toggleSelect(itemId);
    } else if (window.matchMedia('(max-width: 768px)').matches) {
      goto(`/ingest-queue/${itemId}`);
    } else {
      if (selectedGroupKey === groupKey && selectedItemId === itemId) {
        selectedGroupKey = null;
        selectedItemId = null;
      } else {
        selectedGroupKey = groupKey;
        selectedItemId = itemId;
      }
    }
  };

  const closeDetail = () => {
    selectedGroupKey = null;
    selectedItemId = null;
  };

  // Wizard helpers
  const startWizard = () => {
    const pendingSelected = allItems.filter(
      (i) => selectedIds.has(i.id) && i.ingest_status === 'pending'
    );
    if (pendingSelected.length === 0) return;
    wizardItems = pendingSelected;
    wizardIndex = 0;
    showWizard = true;
  };

  const wizardApprove = async (_scope: string) => {
    const item = wizardItems[wizardIndex];
    if (!item) return;
    // Use selectedDeviceIds for batch approval
    await onApprove(item.id, selectedDeviceIds);
    nextWizardItem();
  };

  const wizardReject = async () => {
    const item = wizardItems[wizardIndex];
    if (!item) return;
    await onReject(item.id);
    nextWizardItem();
  };

  const wizardSkip = () => {
    nextWizardItem();
  };

  const nextWizardItem = () => {
    if (wizardIndex < wizardItems.length - 1) {
      wizardIndex++;
    } else {
      closeWizard();
    }
  };

  const closeWizard = () => {
    showWizard = false;
    wizardItems = [];
    wizardIndex = 0;
    selectedIds = new Set();
    multiSelectMode = false;
  };

  // Infinite scroll
  let scrollSentinel = $state<HTMLDivElement | null>(null);

  $effect(() => {
    if (!scrollSentinel || !onLoadMore) return;

    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0].isIntersecting && hasMore && loadState.status !== 'loading') {
          onLoadMore();
        }
      },
      { threshold: 0.1 }
    );

    observer.observe(scrollSentinel);

    return () => observer.disconnect();
  });
</script>

<div class="page-layout page-container">
  <div class="library-main">
    {#if initialLoadingState.showLoading && groups.length === 0}
      <div class="loading-card">Loading ingest queue...</div>
    {/if}
    {#if loadState.status === 'error'}
      <div class="error">{loadState.error}</div>
    {/if}

    <div class="toolbar">
      <div class="toolbar-left">
        <div class="view-controls">
          <button
            type="button"
            class="view-tab"
            class:active={statusFilter === 'pending'}
            onclick={() => handleStatusFilterChange('pending')}
          >
            Pending
            <span class="tab-count">{pendingCount}</span>
          </button>
          {#if extractingCount > 0 || statusFilter === 'extracting'}
            <button
              type="button"
              class="view-tab"
              class:active={statusFilter === 'extracting'}
              onclick={() => handleStatusFilterChange('extracting')}
            >
              Extracting
              <span class="tab-count">{extractingCount}</span>
            </button>
          {/if}
          {#if errorCount > 0 || statusFilter === 'error'}
            <button
              type="button"
              class="view-tab error"
              class:active={statusFilter === 'error'}
              onclick={() => handleStatusFilterChange('error')}
            >
              Error
              <span class="tab-count">{errorCount}</span>
            </button>
          {/if}
          {#if rejectedCount > 0 || statusFilter === 'rejected'}
            <button
              type="button"
              class="view-tab error"
              class:active={statusFilter === 'rejected'}
              onclick={() => handleStatusFilterChange('rejected')}
            >
              Rejected
              <span class="tab-count">{rejectedCount}</span>
            </button>
          {/if}
        </div>
      </div>

      <div class="toolbar-right">
        {#if onSearch}
          <div class="search-box">
            <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor" class="search-icon">
              <path
                d="M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z"
              ></path>
            </svg>
            <input
              type="text"
              placeholder="Search files..."
              value={searchQuery}
              oninput={handleSearchInput}
            />
            {#if searchQuery}
              <button
                type="button"
                class="clear-btn"
                aria-label="Clear search"
                onclick={() => {
                  searchQuery = '';
                  onSearch?.('');
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
        {/if}
        <button
          class="icon-btn"
          class:active={multiSelectMode}
          type="button"
          title={multiSelectMode ? 'Cancel selection' : 'Toggle multi-select'}
          aria-label={multiSelectMode ? 'Cancel selection' : 'Toggle multi-select'}
          onclick={toggleMultiSelectMode}
        >
          <span class="material-icons-outlined" style="font-size: 16px;">select_all</span>
        </button>

        {#if onUploadBook}
          <input
            type="file"
            accept=".epub,.kepub,.mobi,.azw,.azw3,.pdf"
            class="hidden-file-input"
            bind:this={uploadFileInput}
            onchange={handleFileSelect}
          />
          <button
            class="icon-btn add-btn"
            type="button"
            title="Upload book"
            aria-label="Upload book"
            onclick={() => uploadFileInput?.click()}
            disabled={uploading || loading}
          >
            <span class="material-icons-outlined" style="font-size: 18px;">add</span>
          </button>
        {/if}
      </div>
    </div>

    {#if multiSelectMode}
      <IngestBatchActions
        selectedCount={selectedIds.size}
        {loading}
        onSelectAll={selectAllFiltered}
        onClearSelection={clearSelection}
        onProcessSelected={startWizard}
        onBatchReject={handleBatchReject}
        hasBatchReject={!!onBatchReject}
      />
    {/if}

    <div class="ingest-grid">
      {#if filteredGroups.length === 0}
        <div class="empty-state">
          <h3>No items in queue</h3>
          <p class="muted">
            {statusFilter === 'all'
              ? 'The ingest queue is empty.'
              : `No items with status "${statusFilter}".`}
          </p>
        </div>
      {:else}
        {#each filteredGroups as group (group.group_key)}
          <IngestQueueCard
            {group}
            selected={selectedGroupKey === group.group_key}
            {selectedItemId}
            {multiSelectMode}
            {selectedIds}
            onSelect={handleCardSelect}
          />
        {/each}

        <!-- Infinite scroll sentinel -->
        {#if hasMore}
          <div bind:this={scrollSentinel} class="scroll-sentinel">
            {#if loadState.status === 'loading'}
              <span class="muted">Loading more...</span>
            {/if}
          </div>
        {/if}
      {/if}
    </div>
  </div>
</div>

<!-- Unified Sidebar - switches content based on mode (item details, navigation, edit) -->
<ExpandableSidebar
  open={sidebarOpen}
  expanded={showEditMode}
  title={sidebarTitle}
  showEdit={showEditButton}
  editMode={showEditMode}
  onClose={() => {
    if (showEditMode) {
      showEditMode = false;
      editSearchResults = [];
      editProviders = [];
    } else if (showNavSidebar) {
      handleNavClose();
    } else {
      closeDetail();
    }
  }}
  onToggleEdit={showNavSidebar && currentNavEntity?.type !== 'book'
    ? undefined
    : () => (showEditMode = !showEditMode)}
  onExpand={selectedItem
    ? () => window.location.assign(`/ingest-queue/${selectedItem.id}`)
    : undefined}
  onSearch={() => (showMetadataSearch = true)}
>
  {#snippet headerLeft()}
    {#if showNavSidebar && nav.canGoBack}
      <button class="icon-btn small" type="button" title="Back" onclick={handleNavBack}>
        <span class="material-icons-outlined">arrow_back</span>
      </button>
    {/if}
  {/snippet}

  {#snippet footer()}
    {#if showItemDetailsSidebar && selectedItem}
      {#if showEditMode}
        <div class="sidebar-footer-actions">
          <button
            class="btn-sec"
            type="button"
            onclick={() => {
              showEditMode = false;
              editError = undefined;
              editSearchResults = [];
              editProviders = [];
            }}
            disabled={editSaving}
          >
            Cancel
          </button>
          <button
            class="btn-primary"
            type="button"
            onclick={() => formRequestSave?.()}
            disabled={!formCanSave}
          >
            {editSaving ? 'Saving...' : 'Save Changes'}
          </button>
        </div>
      {:else}
        <div class="sidebar-footer-actions">
          {#if selectedItem.ingest_status === 'rejected'}
            <button
              class="btn-sec"
              type="button"
              onclick={() => onRestore(selectedItem.id)}
              disabled={loading}
            >
              Restore
            </button>
          {:else}
            <button
              class="btn-danger-outline"
              type="button"
              onclick={() => onReject(selectedItem.id)}
              disabled={loading}
            >
              Reject
            </button>
          {/if}
          <button
            class="btn-primary"
            type="button"
            onclick={() => onApprove(selectedItem.id, selectedDeviceIds)}
            disabled={loading}
          >
            Approve
          </button>
        </div>
      {/if}
    {/if}
  {/snippet}

  <!-- Item Details (non-navigation mode) -->
  {#if showItemDetailsSidebar && selectedItem && selectedGroup}
    {#if showEditMode}
      <BookForm
        book={selectedItem}
        searchResults={editSearchResults}
        providers={editProviders}
        error={editError}
        onSave={async (data, coverFile) => {
          if (onUpdateMetadata) {
            editSaving = true;
            editError = undefined;
            try {
              await onUpdateMetadata(selectedItem.id, data, coverFile);
              showEditMode = false;
              editSearchResults = [];
              editProviders = [];
            } catch (e) {
              editError = e instanceof Error ? e.message : 'Update failed';
            } finally {
              editSaving = false;
            }
          }
        }}
        onCancel={() => {
          showEditMode = false;
          editError = undefined;
          editSearchResults = [];
          editProviders = [];
        }}
        saving={editSaving}
        hideFooter
        bind:requestSave={formRequestSave}
        bind:canSave={formCanSave}
      />
    {:else}
      <BookDetailsPanel
        book={selectedItem}
        context="ingest"
        {devices}
        {selectedDeviceIds}
        {loading}
        embedded={true}
        onClose={closeDetail}
        onApprove={() => onApprove(selectedItem.id, selectedDeviceIds)}
        onReject={selectedItem.ingest_status !== 'rejected'
          ? () => onReject(selectedItem.id)
          : undefined}
        onRestore={selectedItem.ingest_status === 'rejected'
          ? () => onRestore(selectedItem.id)
          : undefined}
        onDeviceToggle={handleDeviceToggle}
        onAuthorClick={handleAuthorClick}
        onSeriesClick={handleSeriesClick}
      />

      <!-- Format switcher when group has multiple items -->
      {#if selectedGroup.items.length > 1}
        <div class="format-section">
          <div class="format-label">Other Formats</div>
          <div class="format-tabs">
            {#each selectedGroup.items as formatItem (formatItem.id)}
              <button
                type="button"
                class="format-tab"
                class:active={selectedItem.id === formatItem.id}
                onclick={() => (selectedItemId = formatItem.id)}
              >
                <span class="format-name"
                  >{formatItem.ingest_info?.source_format?.toUpperCase() ||
                    formatItem.formats?.[0] ||
                    ''}</span
                >
                <span class="format-size"
                  >{formatSize(formatItem.ingest_info?.file_size, 'Unknown')}</span
                >
              </button>
            {/each}
          </div>
        </div>
      {/if}
    {/if}
    <!-- Navigation mode content (author/series drill-down) -->
  {:else if showNavSidebar}
    {#if nav.loadingEntity}
      <div class="loading-state">Loading...</div>
    {:else if currentNavEntity?.type === 'book' && currentNavBook}
      <BookDetailsPanel
        book={currentNavBook}
        context="library"
        {devices}
        {loading}
        embedded
        onClose={handleNavClose}
        onAuthorClick={handleAuthorClick}
        onSeriesClick={handleSeriesClick}
      />
    {:else if currentNavEntity?.type === 'series' && currentNavSeries}
      <SeriesSidebarContent
        series={currentNavSeries}
        books={navSeriesBooks}
        loadState={{ status: nav.loadingEntity ? 'loading' : 'success', error: '' }}
        onAuthorClick={handleAuthorClick}
        onBookClick={handleNavBookClick}
        onRefresh={async () => {
          if (currentNavSeries) {
            const [series, books] = await Promise.all([
              api.getSeriesById(currentNavSeries.id),
              api.getSeriesBooks(currentNavSeries.id, true)
            ]);
            updateCurrentEntityData(series);
            navSeriesBooks = books;
          }
        }}
      />
    {:else if currentNavEntity?.type === 'author' && currentNavAuthor}
      <AuthorSidebarContent
        author={currentNavAuthor}
        books={navAuthorBooks}
        loadState={{ status: nav.loadingEntity ? 'loading' : 'success', error: '' }}
        loading={nav.loadingEntity}
        onSeriesClick={handleSeriesClick}
        onBookClick={handleNavBookClick}
        onRefresh={async () => {
          if (currentNavAuthor) {
            const [author, books] = await Promise.all([
              api.getAuthor(currentNavAuthor.id),
              api.getAuthorBooks(currentNavAuthor.id, true)
            ]);
            updateCurrentEntityData(author);
            navAuthorBooks = books;
          }
        }}
      />
    {/if}
  {:else}
    <div class="sidebar-empty">
      <span class="material-icons-outlined sidebar-empty-icon">inbox</span>
      <div class="sidebar-empty-title">Select an item</div>
      <p class="muted">Click an item to view details</p>
      <div class="sidebar-stats">
        <div class="stat-item">
          <span class="stat-value">{pendingCount}</span>
          <span class="stat-label">Pending</span>
        </div>
        <div class="stat-item">
          <span class="stat-value">{approvedCount}</span>
          <span class="stat-label">Approved</span>
        </div>
        <div class="stat-item">
          <span class="stat-value">{rejectedCount}</span>
          <span class="stat-label">Rejected</span>
        </div>
      </div>
    </div>
  {/if}
</ExpandableSidebar>

{#if showWizard}
  <IngestApprovalWizard
    items={wizardItems}
    currentIndex={wizardIndex}
    {loading}
    onApprove={wizardApprove}
    onReject={wizardReject}
    onSkip={wizardSkip}
    onClose={closeWizard}
  />
{/if}

<!-- Metadata Search Modal (used in sidebar edit mode) -->
{#if showMetadataSearch && selectedItem}
  <AddEntityModal
    mode="book"
    linkMode={true}
    existingId={selectedItem.id}
    initialSearchValues={{
      title: selectedItem.title || selectedItem.ingest_info?.file_name || '',
      author: selectedItem.authors?.[0]?.name
    }}
    onComplete={() => (showMetadataSearch = false)}
    onResultsSelected={handleMetadataResultsSelected}
    onCancel={() => (showMetadataSearch = false)}
  />
{/if}

<style>
  /* Page-specific styles only - uses global styles from global.css */

  .sidebar-footer-actions {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
  }

  .sidebar-footer-actions :global(button) {
    padding: 8px 16px;
    font-size: 12px;
  }

  /* Toolbar layout - matches LibraryPage */
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

  /* View controls - matches LibraryPage */
  .view-controls {
    display: flex;
    gap: 2px;
    background: var(--card);
    padding: 4px;
    border-radius: 8px;
    border: 1px solid var(--ring);
  }

  .view-tab {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px;
    border-radius: 6px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    color: var(--muted);
    border: none;
    background: none;
    white-space: nowrap;
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

  .view-tab.error {
    color: var(--danger);
  }

  .view-tab.error.active {
    background: rgba(239, 68, 68, 0.15);
  }

  .tab-count {
    font-size: 10px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    min-width: 16px;
    height: 16px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 0 4px;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.08);
    color: var(--muted);
    line-height: 1;
  }

  :global([data-theme='light']) .tab-count {
    background: rgba(0, 0, 0, 0.07);
  }

  .view-tab.active .tab-count {
    background: var(--accent-dim);
    color: var(--accent);
  }

  .view-tab.error .tab-count {
    background: rgba(239, 68, 68, 0.12);
    color: var(--danger);
  }

  .view-tab.error.active .tab-count {
    background: rgba(239, 68, 68, 0.25);
  }

  /* Search box - matches LibraryPage inline-icon pattern */
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

  /* Icon button styling - matches LibraryPage */
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

  .icon-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .icon-btn.active {
    background: var(--accent-dim);
    color: var(--accent);
    border-color: var(--accent);
  }

  /* Grid layout for ingest items */
  .ingest-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 16px;
  }

  /* Empty state override for grid spanning */
  .ingest-grid .empty-state {
    grid-column: 1 / -1;
  }

  .scroll-sentinel {
    grid-column: 1 / -1;
    padding: 16px;
    text-align: center;
  }

  /* Hidden file input for upload */
  .hidden-file-input {
    display: none;
  }

  /* Format switcher for multi-format ingest groups */
  .format-section {
    padding: 16px;
    border-top: 1px solid var(--ring);
  }

  .format-label {
    font-size: 12px;
    color: var(--muted);
    margin-bottom: 8px;
  }

  .format-tabs {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .format-tab {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    padding: 8px 12px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    cursor: pointer;
    transition:
      background 0.15s,
      border-color 0.15s;
  }

  .format-tab:hover {
    background: var(--ring);
  }

  .format-tab.active {
    background: var(--accent);
    border-color: var(--accent);
    color: white;
  }

  .format-tab .format-name {
    font-size: 12px;
    font-weight: 600;
  }

  .format-tab .format-size {
    font-size: 10px;
    opacity: 0.7;
  }

  /* Sidebar empty state */
  .sidebar-empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 40px 20px;
    text-align: center;
  }

  .sidebar-empty-icon {
    font-size: 48px;
    color: var(--muted);
    margin-bottom: 16px;
    opacity: 0.5;
  }

  .sidebar-empty-title {
    font-size: 16px;
    font-weight: 600;
    color: var(--ink);
    margin-bottom: 8px;
  }

  .sidebar-stats {
    display: flex;
    gap: 24px;
    margin-top: 24px;
  }

  .stat-item {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
  }

  .stat-value {
    font-size: 24px;
    font-weight: 600;
    color: var(--accent);
  }

  .stat-label {
    font-size: 11px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  /* Loading state for navigation */
  .loading-state {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 40px;
    color: var(--muted);
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

    .ingest-grid {
      grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
      gap: 12px;
    }
  }
</style>
