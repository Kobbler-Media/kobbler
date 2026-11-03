<script lang="ts">
  import { Modal } from '@koblibs/ui';
  import type { IndexerRelease, LibrarySearchResponse } from '#lib/types.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { untrack } from 'svelte';

  interface Props {
    /** Display name for the entity */
    entityTitle: string;
    /** Free text search — query comes from the manual input.
     * `scope` is "any" when the user has turned off category filtering. */
    searchFn: (query: string, scope?: string) => Promise<LibrarySearchResponse>;
    /** Metadata-based search — no query needed, uses identifiers */
    smartSearchFn?: () => Promise<LibrarySearchResponse>;
    /** Download a specific release from the results */
    queueFn: (release: IndexerRelease, source: string) => Promise<void>;
    /** Auto-want: let kobload handle everything */
    wantFn?: () => Promise<void>;
    /** Pre-populated text for manual search */
    initialQuery?: string;
    /** What the default scope covers, for the toggle's hint (e.g. "book", "movie"). */
    scopeLabel?: string;
    /** Called when modal should close */
    onClose: () => void;
  }

  let {
    entityTitle,
    searchFn,
    smartSearchFn,
    queueFn,
    wantFn,
    initialQuery = '',
    scopeLabel = 'this type',
    onClose
  }: Props = $props();

  type SearchMode = 'recommended' | 'manual';

  let mode = $state<SearchMode>(untrack(() => smartSearchFn) ? 'recommended' : 'manual');
  let manualQuery = $state(untrack(() => initialQuery || entityTitle));
  /** Off = scoped to book categories. On = no category filter, for releases
   * mis-filed under Other/Misc. */
  let anyCategory = $state(false);

  // Recommended tab state
  let recResults = $state<IndexerRelease[]>([]);
  let recLoading = $state(false);
  let recError = $state<string | null>(null);
  let recLoaded = $state(false);

  // Manual tab state
  let manResults = $state<IndexerRelease[]>([]);
  let manLoading = $state(false);
  let manError = $state<string | null>(null);
  let manSearched = $state(false);

  let downloading = $state<string | null>(null);
  let wanting = $state(false);

  // Derived display state
  let activeResults = $derived(mode === 'recommended' ? recResults : manResults);
  let activeLoading = $derived(mode === 'recommended' ? recLoading : manLoading);
  let activeError = $derived(mode === 'recommended' ? recError : manError);
  let showScores = $derived(mode === 'recommended');
  let hasSearched = $derived(mode === 'recommended' ? recLoaded : manSearched);

  async function loadRecommended() {
    if (!smartSearchFn || recLoaded) return;
    recLoading = true;
    recError = null;
    try {
      const response = await smartSearchFn();
      recResults = response.results;
    } catch (e) {
      recError = e instanceof Error ? e.message : 'Search failed';
    } finally {
      recLoading = false;
      recLoaded = true;
    }
  }

  async function handleManualSearch() {
    if (!manualQuery.trim()) return;
    manLoading = true;
    manError = null;
    manResults = [];
    try {
      const response = await searchFn(manualQuery.trim(), anyCategory ? 'any' : undefined);
      manResults = response.results;
    } catch (e) {
      manError = e instanceof Error ? e.message : 'Search failed';
    } finally {
      manLoading = false;
      manSearched = true;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') handleManualSearch();
  }

  async function handleGrab(release: IndexerRelease) {
    if (downloading) return;
    downloading = release.guid ?? release.title;
    try {
      await queueFn(release, release.indexer ?? 'manual');
      showMessage('Download queued');
      onClose();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Download failed');
    } finally {
      downloading = null;
    }
  }

  async function handleAutoFind() {
    if (!wantFn || wanting) return;
    wanting = true;
    try {
      await wantFn();
      showMessage('Marked as wanted — searching for releases');
      onClose();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to mark as wanted');
    } finally {
      wanting = false;
    }
  }

  function switchToManual() {
    mode = 'manual';
  }

  function getScore(release: IndexerRelease): number | null {
    const score = (release as Record<string, unknown>).score;
    if (typeof score === 'number') return score;
    return null;
  }

  function formatScore(score: number): string {
    return `${Math.round(score * 100)}%`;
  }

  function scoreClass(score: number): string {
    if (score >= 0.8) return 'score-high';
    if (score >= 0.5) return 'score-mid';
    return 'score-low';
  }

  function formatSize(bytes: number | undefined): string {
    if (!bytes) return 'Unknown size';
    const mb = bytes / 1024 / 1024;
    if (mb < 1024) return `${mb.toFixed(1)} MB`;
    return `${(mb / 1024).toFixed(2)} GB`;
  }

  function formatAge(dateStr: string | undefined): string {
    if (!dateStr) return '';
    const days = Math.floor((Date.now() - Date.parse(dateStr)) / 86400000);
    if (days < 1) return 'today';
    if (days === 1) return '1d ago';
    if (days < 30) return `${days}d ago`;
    if (days < 365) return `${Math.floor(days / 30)}mo ago`;
    return `${Math.floor(days / 365)}y ago`;
  }

  function formatDate(iso: string): string {
    try {
      return new Date(iso).toLocaleDateString();
    } catch {
      return iso;
    }
  }

  // Auto-fire recommended search on mount
  $effect(() => {
    if (smartSearchFn && !recLoaded) {
      loadRecommended();
    }
  });
</script>

<Modal {onClose} title="Search Releases — {entityTitle}" width="600px" overflowVisible>
  <div class="modal-content">
    <!-- Tab bar + search input area -->
    <div class="search-header">
      {#if smartSearchFn}
        <div class="mode-tabs">
          <button
            type="button"
            class="tab-btn"
            class:active={mode === 'recommended'}
            onclick={() => (mode = 'recommended')}
          >
            Recommended
          </button>
          <button
            type="button"
            class="tab-btn"
            class:active={mode === 'manual'}
            onclick={() => (mode = 'manual')}
          >
            Manual Search
          </button>
        </div>
      {/if}

      {#if mode === 'recommended'}
        <div class="search-context">
          <span class="material-icons-outlined context-icon">auto_awesome</span>
          <span>Matched using metadata &amp; identifiers</span>
        </div>
      {:else}
        <div class="search-row">
          <input
            type="text"
            bind:value={manualQuery}
            placeholder="Search for releases..."
            onkeydown={handleKeydown}
            class="search-input"
          />
          <button
            class="btn btn-primary"
            onclick={handleManualSearch}
            disabled={manLoading || !manualQuery.trim()}
          >
            {manLoading ? 'Searching...' : 'Search'}
          </button>
        </div>
        <label class="scope-toggle">
          <input type="checkbox" bind:checked={anyCategory} onchange={handleManualSearch} />
          <span>Search all categories</span>
          <span class="scope-hint">
            {anyCategory
              ? 'No category filter — includes releases filed under Other/Misc'
              : `Scoped to ${scopeLabel} categories`}
          </span>
        </label>
      {/if}
    </div>

    <!-- Results -->
    <div class="results-section">
      {#if activeLoading}
        <div class="empty-state">
          <span class="material-icons-outlined spin">refresh</span>
          <span>{mode === 'recommended' ? 'Searching with metadata...' : 'Searching...'}</span>
        </div>
      {:else if activeError}
        <div class="error-message">
          <span>{activeError}</span>
          {#if mode === 'recommended'}
            <button class="btn btn-sm" onclick={loadRecommended}>Retry</button>
          {/if}
        </div>
      {:else if activeResults.length === 0 && hasSearched}
        <div class="empty-state">
          <span
            >No releases found{mode === 'recommended'
              ? ' matching metadata'
              : ` for "${manualQuery}"`}.</span
          >
          {#if mode === 'recommended' && smartSearchFn}
            <button class="link-btn" onclick={switchToManual}>Try a manual search</button>
          {/if}
        </div>
      {:else if activeResults.length === 0}
        <div class="empty-state">
          {#if mode === 'manual'}
            <span>Type a search query and press Enter</span>
          {/if}
        </div>
      {:else}
        <div class="results-list">
          {#each activeResults as release, i (release.guid ?? release.title + ':' + i)}
            {@const isDownloading = downloading === (release.guid ?? release.title)}
            {@const score = showScores ? getScore(release) : null}
            <div
              class="release-item"
              class:previously-downloaded={release.previously_downloaded}
              class:rejected-release={release.rejected_release}
            >
              <div class="release-info">
                <div class="release-title-row">
                  <span class="release-title">{release.title}</span>
                  {#if score !== null}
                    <span class="score-pill {scoreClass(score)}">{formatScore(score)}</span>
                  {/if}
                </div>
                <div class="release-meta">
                  <span class="release-size">{formatSize(release.size)}</span>
                  {#if release.indexer}
                    <span class="release-indexer">{release.indexer}</span>
                  {/if}
                  {#if release.protocol === 'torrent'}
                    {#if release.seeders != null}
                      <span class="release-seeders">{release.seeders} seeders</span>
                    {/if}
                  {:else if release.grabs != null}
                    <span class="release-grabs">{release.grabs} grabs</span>
                  {/if}
                  {#if release.publish_date}
                    <span class="release-age">{formatAge(release.publish_date)}</span>
                  {/if}
                </div>
                {#if release.rejected_release}
                  <div class="rejected-release-badge">
                    Previously rejected — {release.rejected_release.reason === 'user_replaced'
                      ? 'replaced by user'
                      : release.rejected_release.reason}
                  </div>
                {/if}
                {#if release.previously_downloaded}
                  <div class="previously-downloaded-badge">
                    Previously downloaded on {formatDate(
                      release.previously_downloaded.downloaded_at
                    )}
                    for "{release.previously_downloaded.book_title}"
                  </div>
                {/if}
              </div>
              <button
                class="btn btn-grab"
                onclick={() => handleGrab(release)}
                disabled={isDownloading || !!downloading}
              >
                {isDownloading ? 'Queuing...' : 'Grab'}
              </button>
            </div>
          {/each}
        </div>
      {/if}
    </div>

    <!-- Auto-find section -->
    {#if wantFn}
      <div class="auto-section">
        <div class="divider-or"><span>or</span></div>
        <button class="btn btn-auto" onclick={handleAutoFind} disabled={wanting}>
          <span class="material-icons-outlined">auto_fix_high</span>
          {wanting ? 'Finding...' : 'Auto-find best match'}
        </button>
        <div class="auto-desc">Kobload will find and download the best release automatically</div>
      </div>
    {/if}
  </div>
</Modal>

<style>
  .modal-content {
    display: flex;
    flex-direction: column;
    max-height: calc(90vh - 100px);
  }

  .search-header {
    flex-shrink: 0;
    margin-bottom: 12px;
  }

  /* ── Tabs ── */
  .mode-tabs {
    display: inline-flex;
    padding: 3px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    margin-bottom: 10px;
  }

  .tab-btn {
    padding: 5px 12px;
    border-radius: 6px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    color: var(--muted);
    border: none;
    background: none;
    white-space: nowrap;
    transition:
      background 0.15s,
      color 0.15s;
  }

  .tab-btn:hover {
    background: rgba(255, 255, 255, 0.05);
    color: var(--ink);
  }

  .tab-btn.active {
    background: var(--bg-2);
    color: var(--ink);
  }

  /* ── Search context (recommended) ── */
  .search-context {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .context-icon {
    font-size: 14px;
    color: var(--accent);
  }

  /* ── Manual search row ── */
  .search-row {
    display: flex;
    gap: 8px;
  }

  .scope-toggle {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 8px;
    font-size: 12px;
    color: var(--ink);
    cursor: pointer;
  }

  .scope-toggle input {
    cursor: pointer;
  }

  .scope-hint {
    color: var(--muted);
  }

  .search-input {
    flex: 1;
    padding: 8px 12px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--background, var(--bg));
    color: var(--ink);
    font-size: 14px;
  }

  .search-input:focus {
    outline: none;
    border-color: var(--accent);
  }

  /* ── Results ── */
  .results-section {
    max-height: 400px;
    overflow-y: auto;
    min-height: 80px;
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    color: var(--muted);
    padding: 32px;
    font-size: 13px;
  }

  .link-btn {
    background: none;
    border: none;
    color: var(--accent);
    cursor: pointer;
    font-size: 12px;
    text-decoration: underline;
    padding: 0;
  }

  .link-btn:hover {
    opacity: 0.8;
  }

  .error-message {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--destructive);
    padding: 8px;
    margin-bottom: 12px;
    border: 1px solid var(--destructive);
    border-radius: 8px;
    font-size: 13px;
  }

  .results-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .release-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--card);
  }

  .release-item.previously-downloaded {
    border-color: var(--primary);
    background: color-mix(in srgb, var(--primary) 5%, var(--card));
  }

  .release-item.rejected-release {
    border-color: var(--destructive, #dc3545);
    background: color-mix(in srgb, var(--destructive, #dc3545) 5%, var(--card));
    opacity: 0.7;
  }

  .release-info {
    flex: 1;
    min-width: 0;
  }

  .release-title-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .release-title {
    font-size: 13px;
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
    min-width: 0;
  }

  .release-meta {
    display: flex;
    gap: 12px;
    margin-top: 4px;
    font-size: 12px;
    color: var(--muted);
  }

  .release-seeders {
    color: var(--primary);
  }

  /* ── Score pills ── */
  .score-pill {
    flex-shrink: 0;
    display: inline-block;
    padding: 2px 8px;
    border-radius: 10px;
    font-size: 11px;
    font-weight: 600;
    line-height: 1.3;
  }

  .score-high {
    background: color-mix(in srgb, var(--primary) 15%, transparent);
    color: var(--primary);
  }

  .score-mid {
    background: color-mix(in srgb, var(--accent) 15%, transparent);
    color: var(--accent);
  }

  .score-low {
    background: color-mix(in srgb, var(--destructive) 15%, transparent);
    color: var(--destructive);
  }

  /* ── Badges ── */
  .previously-downloaded-badge {
    margin-top: 6px;
    padding: 4px 8px;
    background: color-mix(in srgb, var(--primary) 15%, transparent);
    border-radius: 4px;
    font-size: 11px;
    color: var(--primary);
  }

  .rejected-release-badge {
    margin-top: 6px;
    padding: 4px 8px;
    background: color-mix(in srgb, var(--destructive, #dc3545) 15%, transparent);
    border-radius: 4px;
    font-size: 11px;
    color: var(--destructive, #dc3545);
    font-weight: 500;
  }

  /* ── Buttons ── */
  .btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 8px 12px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    transition:
      background 0.15s,
      border-color 0.15s;
    white-space: nowrap;
  }

  .btn:hover:not(:disabled) {
    background: var(--bg-2);
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-primary {
    background: var(--primary);
    color: #fff;
    border-color: var(--primary);
  }

  .btn-primary:hover:not(:disabled) {
    opacity: 0.9;
  }

  .btn-sm {
    padding: 4px 10px;
    font-size: 12px;
  }

  .btn-grab {
    padding: 6px 12px;
    font-size: 12px;
    flex-shrink: 0;
  }

  /* ── Auto-find section ── */
  .auto-section {
    flex-shrink: 0;
    margin-top: 16px;
  }

  .divider-or {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 12px;
    color: var(--muted);
    font-size: 12px;
  }

  .divider-or::before,
  .divider-or::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--ring);
  }

  .btn-auto {
    width: 100%;
    justify-content: center;
    padding: 10px 16px;
    border-color: var(--accent);
    color: var(--accent);
    background: transparent;
    font-size: 13px;
    font-weight: 600;
  }

  .btn-auto:hover:not(:disabled) {
    background: var(--accent);
    color: #1d130c;
  }

  .btn-auto .material-icons-outlined {
    font-size: 16px;
  }

  .auto-desc {
    font-size: 12px;
    color: var(--muted);
    text-align: center;
    margin-top: 6px;
  }

  /* ── Loading spinner ── */
  .spin {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
