<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import type { MediaRoot, MediaRootFile } from '#lib/types.js';
  import { mediaTypeChip, mediaRootStatusChip } from '#lib/utils/chip-mappings.js';
  import IconChip from '#lib/components/shared/IconChip.svelte';
  import { on, off } from '#lib/services/eventstream.svelte.js';
  import {
    listMediaRoots,
    getMediaRootFile,
    listMediaRootFilesLite,
    createMediaRoot,
    updateMediaRoot,
    deleteMediaRoot,
    triggerMediaRootScan,
    triggerWriteSidecars,
    triggerWriteImages,
    detectMediaType,
    ApiError
  } from '#lib/api/client.js';
  import type { DetectMediaTypeResponse } from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { formatDistanceToNow, formatDate } from '#lib/utils/format.js';
  import { ExpandableSidebar } from '@koblibs/ui';
  import MediaRootDetailSidebar from '#lib/components/media-roots/MediaRootDetailSidebar.svelte';
  import FileList from '#lib/components/media-roots/FileList.svelte';
  import MediaRootEntityView from '#lib/components/media-roots/MediaRootEntityView.svelte';
  import MatchResolver from '#lib/components/media-roots/MatchResolver.svelte';
  import EntityReassignModal from '#lib/components/media-roots/EntityReassignModal.svelte';
  import ScanResults from '#lib/components/media-roots/ScanResults.svelte';
  import ReorganizePreview from '#lib/components/media-roots/ReorganizePreview.svelte';
  import DirectoryBrowser from '#lib/components/media-roots/DirectoryBrowser.svelte';
  import { SearchSelect } from '@koblibs/ui';
  import NamingTemplateEditor from '#lib/components/media-roots/NamingTemplateEditor.svelte';

  const mediaTypeOptions = [
    { value: 'tv', label: 'TV Shows' },
    { value: 'movie', label: 'Movies' },
    { value: 'book', label: 'Books' },
    { value: 'audiobook', label: 'Audiobooks' },
    { value: 'sport', label: 'Sports' }
  ];

  let loading = $state(false);
  let roots = $state<MediaRoot[]>([]);
  let showNewForm = $state(false);
  let selectedId = $state<string | null>(null);
  let sidebarExpanded = $state(false);
  let viewMode = $state<'detail' | 'files' | 'scan-results'>('detail');
  let fileViewMode = $state<'list' | 'entity'>('list');
  let showReorganize = $state(false);

  // New form
  let newPath = $state('');
  let newName = $state('');
  let newMediaType = $state<'tv' | 'movie' | 'book' | 'audiobook' | 'sport'>('tv');
  let newAutoAdd = $state(true);
  let newWriteSidecars = $state(true);
  let newWriteImages = $state(true);
  let newNamingTemplate = $state('');

  // Edit state
  let editMode = $state(false);
  let saving = $state(false);
  let editPath = $state('');
  let editName = $state('');
  let editAutoAdd = $state(true);
  let editWriteSidecars = $state(false);
  let editWriteImages = $state(false);
  let editNamingTemplate = $state('');
  let editMaxConcurrentIo = $state(1);
  let editScanIntervalSeconds = $state<number | null>(null);

  // Directory browser
  let showBrowseNew = $state(false);
  let showBrowseEdit = $state(false);

  // Media type detection
  let detecting = $state(false);
  let detectionResult = $state<DetectMediaTypeResponse | null>(null);
  let detectTimeout: ReturnType<typeof setTimeout> | null = null;
  let loadRootsTimer: ReturnType<typeof setTimeout> | null = null;

  async function autoDetectMediaType(path: string) {
    if (!path) {
      detectionResult = null;
      return;
    }
    detecting = true;
    try {
      const result = await detectMediaType(path);
      detectionResult = result;
      if (result.suggested_type) {
        newMediaType = result.suggested_type;
      }
    } catch {
      detectionResult = null;
    } finally {
      detecting = false;
    }
  }

  function onPathInput() {
    if (detectTimeout) clearTimeout(detectTimeout);
    detectTimeout = setTimeout(() => autoDetectMediaType(newPath), 500);
  }

  // SSE event handlers
  let jobCallback: ReturnType<typeof on> | null = null;

  // Match resolver
  let resolveFile = $state<MediaRootFile | null>(null);
  let resolveAdditionalFileIds = $state<string[]>([]);

  // Reassign modal state
  let reassignFileIds = $state<string[]>([]);
  let reassignGroupLabel = $state('');
  let reassignOpen = $state(false);

  const selectedRoot = $derived(selectedId ? roots.find((r) => r.id === selectedId) : null);

  async function loadRoots() {
    loading = true;
    try {
      roots = await listMediaRoots();
    } catch {
      showMessage('Failed to load media roots');
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    loadRoots();

    // Reload roots when a media_root job completes or fails (debounced to avoid
    // spamming the API — scans emit one job event per file processed)
    jobCallback = on('job', (data: { entity_type?: string; status?: string }) => {
      if (
        data.entity_type === 'media_root' &&
        (data.status === 'completed' || data.status === 'failed')
      ) {
        if (loadRootsTimer) clearTimeout(loadRootsTimer);
        loadRootsTimer = setTimeout(() => loadRoots(), 500);
      }
    });
  });

  onDestroy(() => {
    if (jobCallback) off('job', jobCallback);
    if (loadRootsTimer) clearTimeout(loadRootsTimer);
  });

  async function handleCreate() {
    loading = true;
    try {
      await createMediaRoot({
        path: newPath,
        name: newName || undefined,
        media_type: newMediaType,
        auto_add: newAutoAdd,
        write_sidecars: newWriteSidecars,
        write_images: newWriteImages,
        naming_template: newNamingTemplate || null
      });
      showMessage('Media root created');
      newPath = '';
      newName = '';
      newMediaType = 'tv';
      newAutoAdd = true;
      newWriteSidecars = true;
      newWriteImages = true;
      newNamingTemplate = '';
      detectionResult = null;
      showNewForm = false;
      await loadRoots();
    } catch {
      showMessage('Create failed');
    } finally {
      loading = false;
    }
  }

  function enterEditMode() {
    if (!selectedRoot) return;
    editPath = selectedRoot.path;
    editName = selectedRoot.name;
    editAutoAdd = selectedRoot.auto_add;
    editWriteSidecars = selectedRoot.write_sidecars;
    editWriteImages = selectedRoot.write_images;
    editNamingTemplate = selectedRoot.naming_template || '';
    editMaxConcurrentIo = selectedRoot.max_concurrent_io ?? 1;
    editScanIntervalSeconds = selectedRoot.scan_interval_seconds ?? null;
    editMode = true;
  }

  async function saveEdit() {
    if (!selectedRoot) return;
    saving = true;
    try {
      await updateMediaRoot(selectedRoot.id, {
        path: editPath !== selectedRoot.path ? editPath : undefined,
        name: editName !== selectedRoot.name ? editName : undefined,
        auto_add: editAutoAdd,
        write_sidecars: editWriteSidecars,
        write_images: editWriteImages,
        naming_template: editNamingTemplate || null,
        max_concurrent_io:
          editMaxConcurrentIo !== selectedRoot.max_concurrent_io ? editMaxConcurrentIo : undefined,
        scan_interval_seconds:
          editScanIntervalSeconds !== (selectedRoot.scan_interval_seconds ?? null)
            ? (editScanIntervalSeconds ?? 0)
            : undefined
      });
      showMessage('Media root updated');
      editMode = false;
      await loadRoots();
    } catch {
      showMessage('Update failed');
    } finally {
      saving = false;
    }
  }

  async function handleToggleEnabled() {
    if (!selectedRoot) return;
    try {
      await updateMediaRoot(selectedRoot.id, { enabled: !selectedRoot.enabled });
      showMessage(selectedRoot.enabled ? 'Disabled' : 'Enabled');
      await loadRoots();
    } catch {
      showMessage('Update failed');
    }
  }

  async function handleDelete() {
    if (!selectedRoot) return;
    try {
      await deleteMediaRoot(selectedRoot.id);
      showMessage('Media root deleted');
      closeSidebar();
      await loadRoots();
    } catch {
      showMessage('Delete failed');
    }
  }

  async function handleScan(id: string) {
    try {
      await triggerMediaRootScan(id);
      // Optimistically update the local root's scan_status
      roots = roots.map((r) => (r.id === id ? { ...r, scan_status: 'scanning' as const } : r));
      showMessage('Scan started');
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) {
        showMessage('Scan already pending');
      } else {
        showMessage('Scan failed');
      }
    }
  }

  async function handleWriteSidecars(id: string) {
    try {
      await triggerWriteSidecars(id);
      showMessage('Write sidecars job started');
    } catch {
      showMessage('Write sidecars failed');
    }
  }

  async function handleWriteImages(id: string) {
    try {
      await triggerWriteImages(id);
      showMessage('Write images job started');
    } catch {
      showMessage('Write images failed');
    }
  }

  function handleReorganize(_id: string) {
    showReorganize = true;
  }

  async function handleGroupResolve(groupKey: string, rootIdParam: string) {
    try {
      const [resp, ambResp] = await Promise.all([
        listMediaRootFilesLite(rootIdParam, {
          group_key: groupKey,
          status: 'unmatched',
          limit: 5000
        }),
        listMediaRootFilesLite(rootIdParam, {
          group_key: groupKey,
          status: 'ambiguous',
          limit: 5000
        })
      ]);
      const allLite = [...resp.items, ...ambResp.items];
      if (allLite.length === 0) return;

      const firstFull = await getMediaRootFile(allLite[0].id);
      resolveAdditionalFileIds = allLite.slice(1).map((f) => f.id);
      resolveFile = firstFull;
    } catch {
      showMessage('Could not load file details');
    }
  }

  async function handleBulkResolve(fileIds: string[]) {
    if (fileIds.length === 0) return;
    try {
      const firstFull = await getMediaRootFile(fileIds[0]);
      resolveAdditionalFileIds = fileIds.slice(1);
      resolveFile = firstFull;
    } catch {
      showMessage('Could not load file details');
    }
  }

  function handleBulkReassign(fileIds: string[], label: string) {
    if (fileIds.length === 0) return;
    reassignFileIds = fileIds;
    reassignGroupLabel = label;
    reassignOpen = true;
  }

  // Component ref for refreshing entity view after resolve
  let entityViewRef = $state<MediaRootEntityView | null>(null);

  function closeSidebar() {
    selectedId = null;
    sidebarExpanded = false;
    editMode = false;
    viewMode = 'detail';
    fileViewMode = 'list';
    showReorganize = false;
  }

  function effectiveStatus(root: MediaRoot): string {
    if (root.scan_status === 'error' && root.scan_error?.startsWith('Directory not found:')) {
      return 'unavailable';
    }
    return root.scan_status;
  }

  function formatLastScan(d: string | null) {
    if (!d) return 'Never';
    return formatDistanceToNow(d);
  }
</script>

<div class="page-layout">
  <div class="library-main">
    <div class="section-header">
      <h2>Media Roots</h2>
      <div class="section-row">
        <span>{roots.length} roots</span>
        <button class="btn" type="button" onclick={() => (showNewForm = !showNewForm)}>
          {showNewForm ? 'Cancel' : 'Add Root'}
        </button>
      </div>
    </div>

    {#if showNewForm}
      <form
        class="new-form card"
        onsubmit={(e) => {
          e.preventDefault();
          handleCreate();
        }}
      >
        <h3>Add Media Root</h3>
        <div class="path-field">
          <label>
            Path
            <input placeholder="/mnt/media/tv" bind:value={newPath} oninput={onPathInput} />
          </label>
          <button
            class="btn alt btn-small browse-btn"
            type="button"
            onclick={() => (showBrowseNew = true)}
          >
            Browse
          </button>
        </div>
        <label>
          Name
          <input placeholder="TV Shows" bind:value={newName} />
        </label>
        <div class="field">
          <span class="field-label">Media Type</span>
          <SearchSelect
            items={mediaTypeOptions}
            value={newMediaType}
            placeholder="Select type..."
            onchange={(val) => {
              if (
                val === 'tv' ||
                val === 'movie' ||
                val === 'book' ||
                val === 'audiobook' ||
                val === 'sport'
              )
                newMediaType = val;
            }}
          />
          {#if detecting}
            <span class="detect-hint">Detecting media type...</span>
          {:else if detectionResult?.suggested_type}
            <span class="detect-hint">
              Detected: {detectionResult.suggested_type === 'tv'
                ? 'TV Shows'
                : detectionResult.suggested_type === 'movie'
                  ? 'Movies'
                  : detectionResult.suggested_type === 'audiobook'
                    ? 'Audiobooks'
                    : 'Books'}
              ({detectionResult.counts.video +
                detectionResult.counts.book +
                (detectionResult.counts.audio ?? 0)} media files sampled)
            </span>
          {/if}
        </div>

        <div class="settings-section">
          <span class="settings-heading">Behavior</span>
          <div class="setting-item">
            <label class="checkbox-label">
              <input type="checkbox" bind:checked={newAutoAdd} />
              Auto-add to library
            </label>
            <p class="setting-desc">
              Automatically add new media found during scans to the library. When off, new media
              goes to the scan results queue for manual review.
            </p>
          </div>
          <div class="setting-item">
            <label class="checkbox-label">
              <input type="checkbox" bind:checked={newWriteSidecars} />
              Write sidecar files
            </label>
            <p class="setting-desc">
              Write NFO and metadata.json files alongside media files for compatibility with other
              media managers.
            </p>
          </div>
          <div class="setting-item">
            <label class="checkbox-label">
              <input type="checkbox" bind:checked={newWriteImages} />
              Write image files
            </label>
            <p class="setting-desc">
              Download and save poster/fanart images to the media directory.
            </p>
          </div>
        </div>

        <NamingTemplateEditor
          value={newNamingTemplate}
          mediaType={newMediaType}
          onchange={(v) => (newNamingTemplate = v)}
        />

        <button class="btn" type="submit" disabled={loading || !newPath}>Add Root</button>
      </form>
    {/if}

    {#if roots.length === 0 && !showNewForm}
      <div class="empty-state full-width">
        <div class="empty-icon">
          <svg viewBox="0 0 24 24" width="48" height="48" fill="currentColor">
            <path
              d="M20 6h-8l-2-2H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V8c0-1.1-.9-2-2-2zm0 12H4V8h16v10z"
            ></path>
          </svg>
        </div>
        <h3>No media roots</h3>
        <p class="muted">Point Kobbler at your media folders to scan and manage your library.</p>
        <button class="btn" type="button" onclick={() => (showNewForm = true)}>Add Root</button>
      </div>
    {:else}
      <div class="directory-grid">
        {#each roots as root (root.id)}
          <button
            class="directory-card"
            class:selected={selectedId === root.id}
            class:disabled={!root.enabled}
            class:type-tv={root.media_type === 'tv'}
            class:type-movie={root.media_type === 'movie'}
            class:type-book={root.media_type === 'book' || root.media_type === 'audiobook'}
            class:type-sport={root.media_type === 'sport'}
            type="button"
            onclick={() => {
              if (window.matchMedia('(max-width: 768px)').matches) {
                goto(`/settings/media-roots/${root.id}`);
                return;
              }
              selectedId = selectedId === root.id ? null : root.id;
              viewMode = 'detail';
              fileViewMode = 'list';
            }}
          >
            <div class="dir-header">
              <span
                class="dir-icon"
                class:tv={root.media_type === 'tv'}
                class:movie={root.media_type === 'movie'}
                class:book={root.media_type === 'book' || root.media_type === 'audiobook'}
                class:sport={root.media_type === 'sport'}
              >
                <span class="material-icons-outlined"
                  >{root.media_type === 'tv'
                    ? 'tv'
                    : root.media_type === 'book'
                      ? 'menu_book'
                      : root.media_type === 'audiobook'
                        ? 'headphones'
                        : root.media_type === 'sport'
                          ? 'sports_soccer'
                          : 'movie'}</span
                >
              </span>
              <span class="dir-name">{root.name}</span>
              <IconChip chip={mediaTypeChip(root.media_type)} />
              {#if root.is_default}
                <span class="default-badge">Default</span>
              {/if}
            </div>
            <div class="dir-path muted">{root.path}</div>
            <div class="dir-status">
              <IconChip chip={mediaRootStatusChip(effectiveStatus(root))} />
              <span class="mode-badge">{root.auto_add ? 'auto' : 'selective'}</span>
              {#if root.error_count > 0}
                <span class="retry-badge">retry #{root.error_count}</span>
              {/if}
            </div>
            {#if root.stats}
              <div class="dir-stats muted">
                {root.stats.added} added, {root.stats.review} review, {root.stats.ambiguous} ambiguous,
                {root.stats.total_files} total
              </div>
            {/if}
            <div
              class="dir-meta muted"
              title={root.last_scan_at ? formatDate(root.last_scan_at) : ''}
            >
              Last scan: {formatLastScan(root.last_scan_at)}
            </div>
          </button>
        {/each}
      </div>
    {/if}
  </div>

  <ExpandableSidebar
    open={selectedId !== null}
    expanded={sidebarExpanded}
    title={selectedRoot?.name || 'Media Root'}
    showEdit={!!selectedRoot && !editMode}
    {editMode}
    onClose={closeSidebar}
    onExpand={() => selectedId && goto(`/settings/media-roots/${selectedId}`)}
    onToggleEdit={() => (editMode ? (editMode = false) : enterEditMode())}
  >
    {#if selectedRoot && viewMode === 'detail'}
      <MediaRootDetailSidebar
        root={selectedRoot}
        {loading}
        {editMode}
        {saving}
        {editPath}
        {editName}
        {editAutoAdd}
        {editWriteSidecars}
        {editWriteImages}
        {editNamingTemplate}
        {editMaxConcurrentIo}
        {editScanIntervalSeconds}
        onEditPathChange={(v) => (editPath = v)}
        onEditNameChange={(v) => (editName = v)}
        onEditAutoAddChange={(v) => (editAutoAdd = v)}
        onEditWriteSidecarsChange={(v) => (editWriteSidecars = v)}
        onEditWriteImagesChange={(v) => (editWriteImages = v)}
        onEditNamingTemplateChange={(v) => (editNamingTemplate = v)}
        onEditMaxConcurrentIoChange={(v) => (editMaxConcurrentIo = v)}
        onEditScanIntervalSecondsChange={(v) => (editScanIntervalSeconds = v)}
        onBrowsePath={() => (showBrowseEdit = true)}
        onScan={handleScan}
        onWriteSidecars={handleWriteSidecars}
        onWriteImages={handleWriteImages}
        onViewFiles={(_id) => {
          viewMode = 'files';
        }}
        onViewScanResults={(_id) => {
          viewMode = 'scan-results';
        }}
        onReorganize={handleReorganize}
        onUpdate={loadRoots}
      />
    {:else if selectedRoot && viewMode === 'files'}
      <div class="files-header">
        <button class="btn alt btn-small" type="button" onclick={() => (viewMode = 'detail')}>
          Back
        </button>
        <span class="files-title"
          >Files in {selectedRoot.name} <span class="files-path">{selectedRoot.path}</span></span
        >
      </div>
      <div class="file-view-tabs">
        <button
          type="button"
          class="file-view-tab"
          class:active={fileViewMode === 'list'}
          onclick={() => (fileViewMode = 'list')}
        >
          List
        </button>
        <button
          type="button"
          class="file-view-tab"
          class:active={fileViewMode === 'entity'}
          onclick={() => (fileViewMode = 'entity')}
        >
          By Entity
        </button>
      </div>
      {#if fileViewMode === 'list'}
        <FileList
          rootId={selectedRoot.id}
          onResolve={(f) => {
            resolveAdditionalFileIds = [];
            resolveFile = f;
          }}
          onBulkResolve={handleBulkResolve}
          onBulkReassign={handleBulkReassign}
        />
      {:else}
        <MediaRootEntityView
          bind:this={entityViewRef}
          rootId={selectedRoot.id}
          mediaType={selectedRoot.media_type}
          onResolve={async (f) => {
            try {
              resolveAdditionalFileIds = [];
              resolveFile = await getMediaRootFile(f.id);
            } catch {
              showMessage('Could not load file details');
            }
          }}
          onGroupResolve={handleGroupResolve}
          onResolveSelected={handleBulkResolve}
          onUpdate={loadRoots}
        />
      {/if}
    {:else if selectedRoot && viewMode === 'scan-results'}
      <div class="files-header">
        <button class="btn alt btn-small" type="button" onclick={() => (viewMode = 'detail')}>
          Back
        </button>
        <span class="files-title"
          >Scan Results for {selectedRoot.name}
          <span class="files-path">{selectedRoot.path}</span></span
        >
      </div>
      <ScanResults rootId={selectedRoot.id} mediaType={selectedRoot.media_type} />
    {:else}
      <div class="sidebar-empty">
        <div class="sidebar-empty-icon">
          <svg viewBox="0 0 24 24" width="32" height="32" fill="currentColor">
            <path
              d="M20 6h-8l-2-2H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V8c0-1.1-.9-2-2-2zm0 12H4V8h16v10z"
            ></path>
          </svg>
        </div>
        <div class="sidebar-empty-title">Select a Media Root</div>
        <p class="muted">Click a media root to view details and manage files.</p>
      </div>
    {/if}

    {#snippet footer()}
      {#if editMode && selectedRoot}
        <div class="footer-row">
          <div class="footer-left">
            <button
              class="btn btn-danger btn-small"
              type="button"
              onclick={handleDelete}
              disabled={loading || saving}>Delete</button
            >
            <button
              class="btn alt btn-small"
              type="button"
              onclick={handleToggleEnabled}
              disabled={loading || saving}
            >
              {selectedRoot.enabled ? 'Disable' : 'Enable'}
            </button>
          </div>
          <div class="footer-right">
            <button
              class="btn alt btn-small"
              type="button"
              onclick={() => (editMode = false)}
              disabled={saving}>Cancel</button
            >
            <button class="btn btn-small" type="button" onclick={saveEdit} disabled={saving}
              >{saving ? 'Saving...' : 'Save'}</button
            >
          </div>
        </div>
      {/if}
    {/snippet}
  </ExpandableSidebar>
</div>

{#if resolveFile && selectedRoot}
  <MatchResolver
    file={resolveFile}
    mediaType={selectedRoot.media_type}
    open={true}
    additionalFileIds={resolveAdditionalFileIds.length > 0 ? resolveAdditionalFileIds : undefined}
    onClose={() => {
      resolveFile = null;
      resolveAdditionalFileIds = [];
    }}
    onResolved={() => {
      resolveFile = null;
      resolveAdditionalFileIds = [];
      loadRoots();
      entityViewRef?.refresh();
    }}
  />
{/if}

{#if reassignOpen && selectedRoot}
  <EntityReassignModal
    mediaType={selectedRoot.media_type}
    fileIds={reassignFileIds}
    groupLabel={reassignGroupLabel}
    open={true}
    onClose={() => {
      reassignOpen = false;
      reassignFileIds = [];
      reassignGroupLabel = '';
    }}
    onReassigned={() => {
      reassignOpen = false;
      reassignFileIds = [];
      reassignGroupLabel = '';
      loadRoots();
      entityViewRef?.refresh();
    }}
  />
{/if}

{#if showReorganize && selectedRoot}
  <ReorganizePreview
    rootId={selectedRoot.id}
    rootName={selectedRoot.name}
    rootPath={selectedRoot.path}
    open={true}
    onClose={() => (showReorganize = false)}
  />
{/if}

<DirectoryBrowser
  open={showBrowseNew}
  initialPath={newPath || undefined}
  onSelect={(path) => {
    newPath = path;
    showBrowseNew = false;
    autoDetectMediaType(path);
  }}
  onClose={() => (showBrowseNew = false)}
/>

<DirectoryBrowser
  open={showBrowseEdit}
  initialPath={editPath || undefined}
  onSelect={(path) => {
    editPath = path;
    showBrowseEdit = false;
  }}
  onClose={() => (showBrowseEdit = false)}
/>

<style>
  .section-header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    margin-bottom: 16px;
  }
  .section-header h2 {
    margin: 0;
    font-size: 18px;
  }
  .section-row {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .section-row span {
    font-size: 12px;
    color: var(--muted);
  }

  .empty-state {
    padding: 24px;
    border-radius: 16px;
    border: 1px dashed var(--ring);
    background: var(--card);
    text-align: center;
  }
  .empty-state.full-width {
    padding: 48px 24px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
  }
  .empty-state h3 {
    margin: 0 0 8px;
    font-size: 16px;
  }
  .empty-state p {
    margin: 0 0 16px;
  }
  .empty-icon {
    color: var(--muted);
    opacity: 0.5;
    margin-bottom: 8px;
  }

  .card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 16px;
  }
  .new-form {
    display: grid;
    gap: 12px;
    margin-bottom: 16px;
  }
  .new-form h3 {
    margin: 0 0 8px;
    font-size: 16px;
  }
  .new-form label {
    display: grid;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
  }
  .new-form input {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
    font-family: inherit;
    resize: vertical;
  }
  .field {
    display: grid;
    gap: 4px;
  }
  .detect-hint {
    font-size: 11px;
    color: var(--muted);
    font-style: italic;
  }
  .field-label {
    font-size: 12px;
    color: var(--muted);
  }
  .settings-section {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 12px;
    border: 1px solid var(--ring);
    border-radius: 12px;
    background: var(--bg);
  }
  .settings-heading {
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }
  .setting-item {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .setting-desc {
    margin: 0;
    font-size: 11px;
    color: var(--muted);
    line-height: 1.5;
    padding-left: 24px;
  }
  .directory-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 14px;
  }
  .directory-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 14px;
    padding: 14px;
    padding-left: 18px;
    text-align: left;
    cursor: pointer;
    transition:
      border-color 0.15s,
      box-shadow 0.15s;
    display: grid;
    gap: 6px;
    border-left: 3px solid var(--accent);
  }
  .directory-card.type-tv {
    border-left-color: #3b82f6;
  }
  .directory-card.type-book {
    border-left-color: #8b5cf6;
  }
  .directory-card.type-movie {
    border-left-color: var(--accent);
  }
  .directory-card.type-sport {
    border-left-color: #10b981;
  }
  .directory-card:hover {
    border-color: var(--accent);
    border-left-color: var(--accent);
  }
  .directory-card.type-tv:hover {
    border-left-color: #3b82f6;
  }
  .directory-card.type-book:hover {
    border-left-color: #8b5cf6;
  }
  .directory-card.type-sport:hover {
    border-left-color: #10b981;
  }
  .directory-card.selected {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent);
  }
  .directory-card.disabled {
    opacity: 0.6;
  }

  .dir-header {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .dir-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--accent);
    flex-shrink: 0;
  }
  .dir-icon :global(.material-icons-outlined) {
    font-size: 20px;
  }
  .dir-icon.tv {
    color: #3b82f6;
  }
  .dir-icon.movie {
    color: var(--accent);
  }
  .dir-icon.book {
    color: #8b5cf6;
  }
  .dir-icon.sport {
    color: #10b981;
  }
  .dir-name {
    font-weight: 600;
    color: var(--ink);
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dir-path {
    font-size: 12px;
    word-break: break-all;
  }
  .dir-status {
    display: flex;
    gap: 8px;
    margin-top: 4px;
  }
  .dir-stats {
    font-size: 11px;
  }
  .dir-meta {
    font-size: 11px;
    margin-top: 4px;
  }

  .default-badge {
    font-size: 10px;
    font-weight: 700;
    padding: 2px 6px;
    border-radius: 4px;
    background: color-mix(in srgb, var(--accent) 20%, transparent);
    color: var(--accent);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .mode-badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 6px;
    background: var(--ring);
    color: var(--ink);
  }
  .retry-badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 6px;
    background: var(--warning, #f59e0b);
    color: white;
  }

  .btn {
    padding: 8px 16px;
    border-radius: 12px;
    border: none;
    background: var(--accent);
    color: white;
    font-weight: 600;
    font-size: 13px;
    cursor: pointer;
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .btn.alt {
    background: var(--card);
    border: 1px solid var(--ring);
    color: var(--ink);
  }
  .btn.alt:hover {
    background: var(--ring);
  }
  .btn-danger {
    background: var(--danger);
    color: white;
  }
  .btn-small {
    padding: 6px 12px;
    font-size: 12px;
  }

  .footer-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .footer-left,
  .footer-right {
    display: flex;
    gap: 8px;
  }

  .files-header {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 12px;
  }
  .files-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--ink);
  }

  .files-title .files-path {
    font-weight: 400;
    color: var(--muted);
    font-size: 12px;
    margin-left: 4px;
  }

  .sidebar-empty {
    text-align: center;
    padding: 24px 16px;
  }
  .sidebar-empty-icon {
    color: var(--muted);
    margin-bottom: 12px;
    opacity: 0.5;
  }
  .sidebar-empty-title {
    font-size: 14px;
    font-weight: 600;
    margin-bottom: 8px;
  }
  .sidebar-empty p {
    font-size: 12px;
    margin: 0;
  }

  .path-field {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 6px;
    align-items: end;
  }

  .path-field label {
    display: grid;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .browse-btn {
    align-self: end;
    height: fit-content;
  }

  .new-form label.checkbox-label {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--ink);
    cursor: pointer;
  }

  .file-view-tabs {
    display: flex;
    gap: 2px;
    background: var(--card);
    padding: 3px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    margin-bottom: 12px;
  }

  .file-view-tab {
    flex: 1;
    padding: 5px 10px;
    border-radius: 6px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    color: var(--muted);
    border: none;
    background: none;
    white-space: nowrap;
    transition: background 0.15s;
    text-align: center;
  }

  .file-view-tab:hover {
    background: rgba(255, 255, 255, 0.05);
    color: var(--ink);
  }

  .file-view-tab.active {
    background: var(--bg-2);
    color: var(--ink);
  }
</style>
