<script lang="ts">
  /**
   * SeriesEditForm - Rich form for creating/editing series.
   * Works in both create mode (no existing series) and edit mode.
   */
  import type { SeriesSummary, SeriesSearchResult, SeriesFormData } from '#lib/types.js';
  import * as api from '#lib/api/client.js';
  import { SearchSelect } from '@koblibs/ui';
  import SourceField from './SourceField.svelte';
  import CoverSelector from './CoverSelector.svelte';
  import { getProviderCode, getProviderName } from '#lib/utils/format.js';

  type SourceValue = {
    source: string;
    sourceName: string;
    value: string;
  };

  interface Props {
    /** Existing series to edit. If undefined, form is in create mode. */
    series?: SeriesSummary | null;
    /** Initial data from metadata search (for create mode) */
    initialData?: Partial<{
      name: string;
      image_url: string | null;
      description: string | null;
      identifiers: Record<string, string>;
      total_book_count: number | null;
      is_monitored: boolean;
      auto_want_mode: string;
    }>;
    /** Metadata search result (for source attribution) */
    searchResult?: SeriesSearchResult | null;
    onSave: (data: SeriesFormData & { coverFile?: File }) => Promise<void>;
    onMerge?: (targetId: string) => Promise<void>;
    onDelete?: () => Promise<void>;
    onCancel: () => void;
    saving?: boolean;
    deleting?: boolean;
    /** When true, footer becomes a fixed full-width bottom bar. */
    fixedFooter?: boolean;
    /** When true, SeriesEditForm does not render its own footer. */
    hideFooter?: boolean;
    requestSave?: () => void;
    canSave?: boolean;
  }

  interface IdentifierEntry {
    key: string;
    value: string;
  }

  let {
    series = null,
    initialData,
    searchResult = null,
    onSave,
    onMerge,
    onDelete,
    onCancel,
    saving = false,
    deleting = false,
    fixedFooter = false,
    hideFooter = false,
    requestSave = $bindable(),
    canSave = $bindable()
  }: Props = $props();

  // Determine mode
  const isCreateMode = $derived(!series);

  // Form state
  let name = $state('');
  let imageUrl = $state('');
  let description = $state('');
  let totalBookCount = $state<number | null>(null);
  let isMonitored = $state(true);
  let autoWantMode = $state('inherit');
  let identifiers = $state<IdentifierEntry[]>([]);
  let coverFile = $state<File | undefined>(undefined);
  let selectedCoverUrl = $state<string | null>(null);

  // Sync state with props
  $effect(() => {
    if (series) {
      name = series.name || '';
      selectedCoverUrl = series.image_url || (series.cover_urls?.[0] ?? null);
      imageUrl = selectedCoverUrl || '';
      description = series.description || '';
      totalBookCount = series.total_book_count ?? null;
      isMonitored = series.is_monitored ?? true;
      autoWantMode = series.auto_want_mode || 'inherit';

      const ids = series.identifiers || {};
      identifiers = Object.entries(ids).map(([key, value]) => ({ key, value: String(value) }));
    } else if (initialData || searchResult) {
      name = initialData?.name || searchResult?.name || '';
      description = initialData?.description || searchResult?.description || '';
      totalBookCount = initialData?.total_book_count ?? searchResult?.book_count ?? null;
      isMonitored = initialData?.is_monitored ?? true;
      autoWantMode = initialData?.auto_want_mode || 'inherit';
      selectedCoverUrl = initialData?.image_url || searchResult?.image_url || null;
      imageUrl = selectedCoverUrl || '';

      const ids = initialData?.identifiers || {};
      const entries = Object.entries(ids).map(([key, value]) => ({ key, value: String(value) }));
      if (searchResult?.external_id && searchResult?.provider) {
        if (!entries.some((i) => i.key === searchResult.provider)) {
          entries.push({ key: searchResult.provider, value: searchResult.external_id });
        }
      }
      identifiers = entries;
    }
  });

  // Computed current cover URL (from first book in series - series don't have their own images)
  const currentCoverUrl = $derived(series?.cover_urls?.[0] ?? null);

  // Build search covers from searchResult for CoverSelector
  const coverOptions = $derived.by(() => {
    const options: { url: string; provider: string }[] = [];
    if (series?.image_url) options.push({ url: series.image_url, provider: 'Series' });
    if (series?.sources) {
      series.sources.forEach((s: any) => {
        if (s.image_url) options.push({ url: s.image_url, provider: s.provider });
      });
    }
    if (searchResult?.image_url)
      options.push({ url: searchResult.image_url, provider: searchResult.provider });
    return options;
  });

  // Build source values for name and description
  const nameSourceValues = $derived.by(() => {
    const values: SourceValue[] = [];
    if (series?.sources) {
      series.sources.forEach((s: any) => {
        values.push({ source: s.provider, sourceName: s.provider, value: s.name });
      });
    }
    if (searchResult?.name) {
      values.push({
        source: getProviderCode(searchResult.provider),
        sourceName: getProviderName(searchResult.provider),
        value: searchResult.name
      });
    }
    return values;
  });

  const descriptionSourceValues = $derived.by(() => {
    const values: SourceValue[] = [];
    if (series?.sources) {
      series.sources.forEach((s: any) => {
        if (s.description)
          values.push({ source: s.provider, sourceName: s.provider, value: s.description });
      });
    }
    if (searchResult?.description) {
      values.push({
        source: getProviderCode(searchResult.provider),
        sourceName: getProviderName(searchResult.provider),
        value: searchResult.description
      });
    }
    return values;
  });

  // Track current source for fields
  let nameSource = $state<string | undefined>(undefined);
  let descriptionSource = $state<string | undefined>(undefined);

  // Cover selection handler
  const handleCoverSelect = (url: string | null, file?: File) => {
    selectedCoverUrl = url;
    imageUrl = url || '';
    coverFile = file;
  };

  // Identifier form
  let newIdKey = $state('');
  let newIdValue = $state('');
  let showIdForm = $state(false);

  // Merge search
  let mergeSearch = $state('');
  let mergeResults = $state<SeriesSummary[]>([]);
  let mergeLoading = $state(false);
  let selectedMergeTarget = $state<SeriesSummary | null>(null);
  let showMergeConfirm = $state(false);

  const searchMergeTargets = async () => {
    if (!mergeSearch.trim() || mergeSearch.length < 2) {
      mergeResults = [];
      return;
    }
    mergeLoading = true;
    try {
      const response = (await api.getSeries({ limit: 20, search: mergeSearch })) as any;
      // Filter out the current series
      mergeResults = (response.items || []).filter((s: SeriesSummary) => s.id !== series?.id);
    } catch {
      mergeResults = [];
    } finally {
      mergeLoading = false;
    }
  };

  const handleMergeSelect = (target: SeriesSummary) => {
    selectedMergeTarget = target;
    showMergeConfirm = true;
  };

  const confirmMerge = async () => {
    if (!selectedMergeTarget || !onMerge) return;
    await onMerge(selectedMergeTarget.id);
    showMergeConfirm = false;
    selectedMergeTarget = null;
  };

  // Auto-want mode options (series has different options including 'inherit')
  const autoWantOptions = [
    { value: 'inherit', label: 'Inherit', description: "Use author's auto-want mode" },
    { value: 'none', label: 'None', description: 'Do not auto-want any books' },
    { value: 'new_only', label: 'New Only', description: 'Auto-want new releases only' },
    { value: 'all', label: 'All', description: 'Auto-want all books in series' },
    { value: 'fill_gaps', label: 'Fill Gaps', description: 'Auto-want missing books only' }
  ];

  const addIdentifier = () => {
    const key = newIdKey.trim().toLowerCase();
    const value = newIdValue.trim();
    if (key && value && !identifiers.some((id) => id.key === key)) {
      identifiers = [...identifiers, { key, value }];
      newIdKey = '';
      newIdValue = '';
      showIdForm = false;
    }
  };

  const removeIdentifier = (key: string) => {
    identifiers = identifiers.filter((id) => id.key !== key);
  };

  const handleSave = async () => {
    if (!name.trim()) return;

    const idObj: Record<string, string> = {};
    identifiers.forEach(({ key, value }) => {
      if (key && value) idObj[key] = value;
    });

    await onSave({
      name: name.trim(),
      identifiers: Object.keys(idObj).length > 0 ? idObj : undefined,
      image_url: imageUrl.trim() || null,
      description: description.trim() || null,
      total_book_count: totalBookCount,
      is_monitored: isMonitored,
      auto_want_mode: autoWantMode,
      coverFile
    });
  };

  // Expose save trigger for external footer buttons
  requestSave = handleSave;

  // Expose reactive save-ability
  $effect(() => {
    canSave = !!name.trim() && !saving;
  });
</script>

<div class="series-edit-form">
  <div class="form-content">
    <div class="form-inner">
      {#if series}
        <h2 class="form-title">Edit Series</h2>
      {/if}
      <div class="form-layout">
        <!-- Cover selection row at top -->
        <div class="cover-row-inline">
          <label class="field-label" for="series-cover">Cover</label>
          <CoverSelector
            id="series-cover"
            currentUrl={currentCoverUrl}
            searchCovers={coverOptions}
            selectedUrl={selectedCoverUrl}
            onSelect={handleCoverSelect}
          />
          {#if !isCreateMode && currentCoverUrl && !selectedCoverUrl}
            <span class="cover-hint">Using cover from first book in series</span>
          {/if}
        </div>

        <!-- Left: Cover Preview -->
        <div class="image-section">
          <div class="image-preview">
            {#if selectedCoverUrl || currentCoverUrl}
              <img src={selectedCoverUrl || currentCoverUrl} alt="Series Cover" class="cover-img" />
            {:else}
              <span class="material-icons-outlined placeholder-icon">collections_bookmark</span>
            {/if}
          </div>
        </div>

        <!-- Right: Fields -->
        <div class="fields-section">
          <!-- Name -->
          <SourceField
            label="Name *"
            value={name}
            sourceValues={nameSourceValues}
            currentSource={nameSource}
            placeholder="Series name..."
            onchange={(val: string, src?: string) => {
              name = val;
              nameSource = src;
            }}
          />

          <!-- Description -->
          <SourceField
            label="Description"
            value={description}
            sourceValues={descriptionSourceValues}
            currentSource={descriptionSource}
            type="textarea"
            placeholder="Series description..."
            onchange={(val: string, src?: string) => {
              description = val;
              descriptionSource = src;
            }}
          />

          <!-- Identifiers -->
          <div class="field-group">
            <div class="field-label-row">
              <label class="field-label" for="add-id-btn">Identifiers</label>
              <button
                id="add-id-btn"
                type="button"
                class="add-btn"
                onclick={() => (showIdForm = !showIdForm)}
              >
                <span class="material-icons-outlined">add</span>
              </button>
            </div>
            {#if showIdForm}
              <div class="id-add-form">
                <input
                  type="text"
                  class="form-input small"
                  bind:value={newIdKey}
                  placeholder="Key (e.g., hardcover)"
                />
                <input
                  type="text"
                  class="form-input small"
                  bind:value={newIdValue}
                  placeholder="Value"
                />
                <button type="button" class="btn-sec small" onclick={addIdentifier}>Add</button>
              </div>
            {/if}
            {#if identifiers.length > 0}
              <div class="identifiers-list">
                {#each identifiers as id (id.key)}
                  <div class="identifier-chip">
                    <span class="id-key">{id.key}:</span>
                    <span class="id-value">{id.value}</span>
                    <button
                      type="button"
                      class="remove-btn"
                      onclick={() => removeIdentifier(id.key)}
                    >
                      <span class="material-icons-outlined">close</span>
                    </button>
                  </div>
                {/each}
              </div>
            {:else}
              <div class="empty-text">No identifiers</div>
            {/if}
          </div>

          <!-- Merge Section (edit mode only) -->
          {#if !isCreateMode && onMerge}
            <div class="field-group">
              <label class="field-label" for="merge-search-input">Merge Into Another Series</label>
              <div class="merge-section">
                <div class="merge-search">
                  <input
                    id="merge-search-input"
                    type="text"
                    class="form-input"
                    bind:value={mergeSearch}
                    oninput={searchMergeTargets}
                    placeholder="Search for series to merge into..."
                  />
                  {#if mergeLoading}
                    <span class="search-loading">Searching...</span>
                  {/if}
                </div>
                {#if mergeResults.length > 0}
                  <div class="merge-results">
                    {#each mergeResults as target (target.id)}
                      <button
                        type="button"
                        class="merge-result"
                        onclick={() => handleMergeSelect(target)}
                      >
                        <span class="merge-name">{target.name}</span>
                        <span class="merge-books">{target.book_count} books</span>
                      </button>
                    {/each}
                  </div>
                {/if}
              </div>
              <span class="field-hint"
                >Merging will move all books to the target series and delete this one</span
              >
            </div>
          {/if}

          <!-- Merge Confirmation Modal -->
          {#if showMergeConfirm && selectedMergeTarget}
            <div
              class="merge-confirm-backdrop"
              role="presentation"
              onclick={() => (showMergeConfirm = false)}
              onkeydown={(e) => e.key === 'Escape' && (showMergeConfirm = false)}
            >
              <div
                class="merge-confirm-modal"
                role="presentation"
                onclick={(e) => e.stopPropagation()}
                onkeydown={() => {}}
              >
                <h3>Confirm Merge</h3>
                <p>
                  Merge <strong>{series?.name}</strong> into
                  <strong>{selectedMergeTarget.name}</strong>?
                </p>
                <p class="warning">
                  This will delete "{series?.name}" and move all books to the target.
                </p>
                <div class="merge-confirm-actions">
                  <button type="button" class="btn-sec" onclick={() => (showMergeConfirm = false)}>
                    Cancel
                  </button>
                  <button type="button" class="btn-danger" onclick={confirmMerge} disabled={saving}>
                    Merge Series
                  </button>
                </div>
              </div>
            </div>
          {/if}

          <!-- Monitoring Section -->
          <div class="field-group">
            <span class="field-label">Monitoring</span>
            <label class="toggle-switch">
              <input type="checkbox" bind:checked={isMonitored} />
              <span class="toggle-slider"></span>
              <span class="toggle-text">Monitor for new books</span>
            </label>
          </div>

          {#if isMonitored}
            <div class="field-group">
              <label class="field-label" for="auto-want-select">Auto-Want Mode</label>
              <SearchSelect
                id="auto-want-select"
                placeholder="Select auto-want mode..."
                items={autoWantOptions.map((opt) => ({
                  value: opt.value,
                  label: `${opt.label} - ${opt.description}`
                }))}
                bind:value={autoWantMode}
              />
            </div>
          {/if}
        </div>
      </div>
    </div>
  </div>

  <!-- Footer -->
  {#if !hideFooter}
    <div class="form-footer" class:fixed-footer={fixedFooter}>
      <div class={fixedFooter ? 'fixed-footer-inner' : 'footer-content'}>
        {#if !isCreateMode && onDelete}
          <button
            type="button"
            class="btn btn-danger"
            onclick={onDelete}
            disabled={deleting || saving}
          >
            <span class="material-icons-outlined">delete</span>
            {deleting ? 'Deleting...' : 'Delete Series'}
          </button>
        {/if}
        <div class="spacer"></div>
        <button type="button" class="btn btn-sec" onclick={onCancel} disabled={saving || deleting}>
          Cancel
        </button>
        <button
          type="button"
          class="btn btn-primary"
          onclick={handleSave}
          disabled={!name.trim() || saving || deleting}
        >
          <span class="material-icons-outlined">save</span>
          {saving ? 'Saving...' : isCreateMode ? 'Create Series' : 'Save Changes'}
        </button>
      </div>
    </div>
  {/if}
</div>

<style>
  .series-edit-form {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  .form-content {
    flex: 1;
    overflow-y: auto;
    padding: 0;
    width: 100%;
  }

  .form-inner {
    max-width: 1600px;
    margin: 0 auto;
    padding: 0 20px 40px;
  }

  .form-title {
    margin: 0 0 32px;
    font-size: 24px;
    font-weight: 800;
    color: var(--ink);
    letter-spacing: -0.02em;
  }

  .form-layout {
    display: grid;
    grid-template-columns: 200px 1fr;
    grid-auto-rows: min-content;
    gap: 40px;
    align-content: start;
  }

  .cover-row-inline {
    grid-column: 1 / -1;
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 8px;
  }

  .cover-row-inline .field-label {
    margin-bottom: 0;
  }

  .cover-hint {
    font-size: 11px;
    color: var(--muted);
    font-style: italic;
  }

  .image-section {
    display: flex;
    flex-direction: column;
    gap: 8px;
    grid-column: 1;
    position: sticky;
    top: 0;
  }

  .image-preview {
    width: 200px;
    height: 300px;
    border-radius: 8px;
    background-color: var(--bg-2);
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    overflow: hidden;
    box-shadow: var(--shadow);
  }

  .cover-img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }

  .placeholder-icon {
    font-size: 40px;
    color: var(--muted);
    opacity: 0.5;
  }

  .fields-section {
    display: flex;
    flex-direction: column;
    gap: 20px;
    min-width: 0;
    grid-column: 2;
  }

  .field-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .field-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .field-label-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .field-hint {
    font-size: 11px;
    color: var(--muted);
    margin-top: 2px;
  }

  .form-input {
    padding: 10px 12px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--bg);
    color: var(--ink);
    font-size: 14px;
    width: 100%;
  }

  .form-input:focus {
    outline: none;
    border-color: var(--accent);
  }

  .form-input.small {
    padding: 6px 10px;
    font-size: 13px;
  }

  .add-btn {
    width: 24px;
    height: 24px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 4px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    padding: 0;
  }

  .add-btn:hover {
    background: var(--bg);
    color: var(--ink);
  }

  .add-btn .material-icons-outlined {
    font-size: 16px;
  }

  .id-add-form {
    display: flex;
    gap: 8px;
    margin-top: 8px;
  }

  .identifiers-list {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 4px;
  }

  .identifier-chip {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 8px;
    background: var(--bg);
    border: 1px solid var(--ring);
    border-radius: 4px;
    font-size: 12px;
  }

  .id-key {
    color: var(--muted);
  }

  .id-value {
    color: var(--ink);
    font-family: monospace;
  }

  .remove-btn {
    width: 16px;
    height: 16px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: none;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    padding: 0;
    margin-left: 4px;
  }

  .remove-btn:hover {
    color: var(--danger);
  }

  .remove-btn .material-icons-outlined {
    font-size: 14px;
  }

  .empty-text {
    font-size: 12px;
    color: var(--muted);
    font-style: italic;
  }

  .toggle-switch {
    display: flex;
    align-items: center;
    gap: 10px;
    cursor: pointer;
    font-size: 14px;
  }

  .toggle-switch input {
    position: absolute;
    opacity: 0;
    width: 0;
    height: 0;
  }

  .toggle-slider {
    position: relative;
    width: 40px;
    height: 22px;
    background: var(--ring);
    border-radius: 11px;
    transition: background 0.2s;
    flex-shrink: 0;
  }

  .toggle-slider::after {
    content: '';
    position: absolute;
    width: 18px;
    height: 18px;
    left: 2px;
    top: 2px;
    background: var(--bg);
    border-radius: 50%;
    transition: transform 0.2s;
  }

  .toggle-switch input:checked + .toggle-slider {
    background: var(--accent);
  }

  .toggle-switch input:checked + .toggle-slider::after {
    transform: translateX(18px);
  }

  .toggle-text {
    color: var(--ink);
  }

  .form-footer {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 24px 20px;
    border-top: 1px solid var(--ring);
    background: var(--card);
    margin-top: auto;
  }

  .form-footer.fixed-footer {
    position: sticky;
    bottom: 0;
    left: 0;
    right: 0;
    width: 100%;
    padding: 16px 40px;
    background: linear-gradient(180deg, rgba(17, 20, 25, 0.85), rgba(21, 24, 30, 0.95));
    backdrop-filter: blur(20px) saturate(1.8);
    -webkit-backdrop-filter: blur(20px) saturate(1.8);
    border-top: 1px solid var(--ring);
    z-index: 1001;
    display: flex;
    align-items: center;
    box-shadow: 0 -10px 40px rgba(0, 0, 0, 0.3);
  }

  :global([data-theme='light']) .form-footer.fixed-footer {
    background: linear-gradient(180deg, rgba(239, 230, 219, 0.85), rgba(245, 239, 231, 0.95));
    box-shadow: 0 -10px 40px rgba(0, 0, 0, 0.05);
  }

  .fixed-footer-inner {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
  }

  .footer-content {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
  }

  .spacer {
    flex: 1;
  }

  /* Merge Section Styles */
  .merge-section {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .merge-search {
    position: relative;
  }

  .search-loading {
    position: absolute;
    right: 12px;
    top: 50%;
    transform: translateY(-50%);
    font-size: 12px;
    color: var(--muted);
  }

  .merge-results {
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 150px;
    overflow-y: auto;
    border: 1px solid var(--ring);
    border-radius: 8px;
    padding: 4px;
  }

  .merge-result {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 12px;
    background: var(--bg);
    border: none;
    border-radius: 6px;
    cursor: pointer;
    text-align: left;
  }

  .merge-result:hover {
    background: var(--bg-2);
  }

  .merge-name {
    font-size: 14px;
    color: var(--ink);
  }

  .merge-books {
    font-size: 12px;
    color: var(--muted);
  }

  .merge-confirm-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1100;
  }

  .merge-confirm-modal {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    padding: 20px;
    max-width: 400px;
    width: 90%;
  }

  .merge-confirm-modal h3 {
    margin: 0 0 12px 0;
    font-size: 16px;
  }

  .merge-confirm-modal p {
    margin: 0 0 8px 0;
    font-size: 14px;
    color: var(--ink);
  }

  .merge-confirm-modal .warning {
    color: var(--danger);
    font-size: 13px;
  }

  .merge-confirm-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    margin-top: 16px;
  }
</style>
