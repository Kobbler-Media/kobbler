<script lang="ts">
  import { untrack } from 'svelte';
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import MediaRootDetailPage from '$lib/components/media-roots/MediaRootDetailPage.svelte';
  import MediaRootEditForm from '$lib/components/media-roots/MediaRootEditForm.svelte';
  import MatchResolver from '$lib/components/media-roots/MatchResolver.svelte';
  import EntityReassignModal from '$lib/components/media-roots/EntityReassignModal.svelte';
  import FileInfoModal from '$lib/components/media-roots/FileInfoModal.svelte';
  import ReorganizePreview from '$lib/components/media-roots/ReorganizePreview.svelte';
  import DirectoryBrowser from '$lib/components/media-roots/DirectoryBrowser.svelte';
  import ContentPageView from '$lib/components/ContentPageView.svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import * as api from '$lib/api/client';
  import type { MediaRoot, MediaRootFile, MediaRootFileLite } from '$lib/types';

  let root = $state<MediaRoot | null>(null);
  let loading = $state(true);
  let resolveFile = $state<MediaRootFile | null>(null);
  let resolveAdditionalFileIds = $state<string[]>([]);
  let inspectFile = $state<MediaRootFile | null>(null);
  let reorganizeRootId = $state<string | null>(null);
  let detailPage = $state<MediaRootDetailPage | null>(null);

  // Reassign modal state
  let reassignFileIds = $state<string[]>([]);
  let reassignGroupLabel = $state('');
  let reassignOpen = $state(false);

  // Edit state
  let editing = $state(false);
  let saving = $state(false);
  let editPath = $state('');
  let editName = $state('');
  let editAutoAdd = $state(true);
  let editWriteSidecars = $state(false);
  let editWriteImages = $state(false);
  let editNamingTemplate = $state('');
  let editMaxConcurrentIo = $state(1);
  let editScanIntervalSeconds = $state<number | null>(null);
  let showBrowseEdit = $state(false);

  const rootId = $derived($page.params.id);

  let loadGeneration = 0;

  async function loadRoot(id: string) {
    const gen = ++loadGeneration;
    loading = true;
    try {
      const r = await api.getMediaRoot(id);
      if (gen !== loadGeneration) return;
      root = r;
    } catch (e) {
      if (gen !== loadGeneration) return;
      showMessage(e instanceof Error ? e.message : 'Failed to load media root');
    } finally {
      if (gen === loadGeneration) loading = false;
    }
  }

  async function refreshRoot(id: string) {
    try {
      const r = await api.getMediaRoot(id);
      root = r;
    } catch {
      // silent
    }
  }

  async function handleScan(id: string) {
    try {
      await api.triggerMediaRootScan(id);
      showMessage('Scan started');
    } catch (e) {
      if (e instanceof api.ApiError && e.status === 409) {
        showMessage('Scan already pending');
      } else {
        showMessage(e instanceof Error ? e.message : 'Failed to start scan');
      }
    }
  }

  async function handleWriteSidecars(id: string) {
    try {
      await api.triggerWriteSidecars(id);
      showMessage('Write sidecars job started');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to start write sidecars');
    }
  }

  async function handleWriteImages(id: string) {
    try {
      await api.triggerWriteImages(id);
      showMessage('Write images job started');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to start write images');
    }
  }

  /** Fetch the full file (with match_candidates) and open the resolve modal. */
  async function handleResolve(file: MediaRootFileLite) {
    try {
      const full = await api.getMediaRootFile(file.id);
      resolveAdditionalFileIds = [];
      resolveFile = full;
    } catch {
      showMessage('Could not load file details');
    }
  }

  async function handleBulkResolve(fileIds: string[]) {
    if (fileIds.length === 0) return;
    try {
      const firstFull = await api.getMediaRootFile(fileIds[0]);
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

  /** Fetch the full file and open the resolve modal for a group of files. */
  async function handleGroupResolve(groupKey: string, rootIdParam: string) {
    try {
      // Fetch the resolvable files for this group
      const [resp, ambResp] = await Promise.all([
        api.listMediaRootFilesLite(rootIdParam, {
          group_key: groupKey,
          status: 'unmatched',
          limit: 5000
        }),
        api.listMediaRootFilesLite(rootIdParam, {
          group_key: groupKey,
          status: 'ambiguous',
          limit: 5000
        })
      ]);
      const allLite = [...resp.items, ...ambResp.items];
      if (allLite.length === 0) return;

      // Fetch the full file for the first one (needs match_candidates for the modal)
      const firstFull = await api.getMediaRootFile(allLite[0].id);
      resolveAdditionalFileIds = allLite.slice(1).map((f) => f.id);
      resolveFile = firstFull;
    } catch {
      showMessage('Could not load file details');
    }
  }

  /** Fetch all file IDs for a group and open the reassign modal. */
  async function handleReassign(groupKey: string, rootIdParam: string) {
    try {
      const resp = await api.listMediaRootFilesLite(rootIdParam, {
        group_key: groupKey,
        limit: 5000
      });
      if (resp.items.length === 0) return;
      const group = resp.items[0];
      const interp = group.interpretation;
      const label =
        interp?.show_name ??
        interp?.movie_title ??
        interp?.series_name ??
        interp?.title ??
        group.file_name;
      reassignFileIds = resp.items.map((f) => f.id);
      reassignGroupLabel = String(label);
      reassignOpen = true;
    } catch {
      showMessage('Could not load group files');
    }
  }

  async function handleReassigned() {
    const resolvedIds = [...reassignFileIds];
    reassignOpen = false;
    reassignFileIds = [];
    reassignGroupLabel = '';
    if (root) await refreshRoot(root.id);
    detailPage?.refreshActiveView(resolvedIds);
  }

  /** Fetch the full file and open the inspect modal. */
  async function handleInspect(file: MediaRootFileLite) {
    try {
      inspectFile = await api.getMediaRootFile(file.id);
    } catch {
      showMessage('Could not load file details');
    }
  }

  function handleReorganize(id: string) {
    reorganizeRootId = id;
  }

  async function handleMatchResolved() {
    const resolvedIds = resolveFile ? [resolveFile.id, ...resolveAdditionalFileIds] : [];
    resolveFile = null;
    resolveAdditionalFileIds = [];
    // Refresh root stats and soft-refresh the view for resolved files
    if (root) await refreshRoot(root.id);
    detailPage?.refreshActiveView(resolvedIds);
  }

  async function handleReorganizeClose() {
    reorganizeRootId = null;
    if (root) {
      await loadRoot(root.id);
      detailPage?.refreshActiveView();
    }
  }

  function enterEditMode() {
    if (!root) return;
    editPath = root.path;
    editName = root.name;
    editAutoAdd = root.auto_add;
    editWriteSidecars = root.write_sidecars;
    editWriteImages = root.write_images;
    editNamingTemplate = root.naming_template || '';
    editMaxConcurrentIo = root.max_concurrent_io ?? 1;
    editScanIntervalSeconds = root.scan_interval_seconds ?? null;
    editing = true;
  }

  async function saveEdit() {
    if (!root) return;
    saving = true;
    try {
      await api.updateMediaRoot(root.id, {
        path: editPath !== root.path ? editPath : undefined,
        name: editName !== root.name ? editName : undefined,
        auto_add: editAutoAdd,
        write_sidecars: editWriteSidecars,
        write_images: editWriteImages,
        naming_template: editNamingTemplate || null,
        max_concurrent_io:
          editMaxConcurrentIo !== root.max_concurrent_io ? editMaxConcurrentIo : undefined,
        // Sentinel: send 0 to clear override, a number to set it, omit to leave alone.
        scan_interval_seconds:
          editScanIntervalSeconds !== (root.scan_interval_seconds ?? null)
            ? (editScanIntervalSeconds ?? 0)
            : undefined
      });
      showMessage('Media root updated');
      editing = false;
      await loadRoot(root.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update media root');
    } finally {
      saving = false;
    }
  }

  async function handleDelete() {
    if (!root) return;
    if (!confirm(`Delete media root "${root.name || root.path}"? This cannot be undone.`)) return;
    try {
      await api.deleteMediaRoot(root.id);
      showMessage('Media root deleted');
      goto('/settings/media-roots');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete media root');
    }
  }

  async function handleToggleEnabled() {
    if (!root) return;
    try {
      await api.updateMediaRoot(root.id, { enabled: !root.enabled });
      showMessage(root.enabled ? 'Disabled' : 'Enabled');
      await loadRoot(root.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update media root');
    }
  }

  $effect(() => {
    const id = rootId;
    if (id) untrack(() => loadRoot(id));
  });
</script>

{#if root && editing}
  <ContentPageView
    title="Edit Media Root"
    onBack={() => (editing = false)}
    onClose={() => goto('/settings/media-roots')}
  >
    <div class="edit-container">
      <MediaRootEditForm
        {root}
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
      />
    </div>

    {#snippet footer()}
      <div class="edit-footer">
        <button type="button" class="btn btn-danger" onclick={handleDelete} disabled={saving}>
          <span class="material-icons-outlined">delete</span>
          Delete
        </button>
        <button type="button" class="btn btn-sec" onclick={handleToggleEnabled} disabled={saving}>
          {root?.enabled ? 'Disable' : 'Enable'}
        </button>
        <div class="spacer"></div>
        <button
          type="button"
          class="btn btn-sec"
          onclick={() => (editing = false)}
          disabled={saving}
        >
          Cancel
        </button>
        <button type="button" class="btn btn-primary" onclick={saveEdit} disabled={saving}>
          <span class="material-icons-outlined">save</span>
          {saving ? 'Saving...' : 'Save'}
        </button>
      </div>
    {/snippet}
  </ContentPageView>
{:else if root}
  <MediaRootDetailPage
    bind:this={detailPage}
    {root}
    {loading}
    onClose={() => goto('/settings/media-roots')}
    onScan={handleScan}
    onWriteSidecars={handleWriteSidecars}
    onWriteImages={handleWriteImages}
    onResolve={handleResolve}
    onInspect={handleInspect}
    onGroupResolve={handleGroupResolve}
    onReassign={handleReassign}
    onBulkResolve={handleBulkResolve}
    onBulkReassign={handleBulkReassign}
    onReorganize={handleReorganize}
    onUpdate={() => root && refreshRoot(root.id)}
    onEdit={enterEditMode}
  />
{:else if loading}
  <div class="loading-page">Loading...</div>
{:else}
  <div class="error-page">
    <p>Media root not found.</p>
    <button type="button" onclick={() => goto('/settings/media-roots')}>Back to Media Roots</button>
  </div>
{/if}

{#if inspectFile}
  <FileInfoModal file={inspectFile} onClose={() => (inspectFile = null)} />
{/if}

{#if resolveFile && root}
  <MatchResolver
    file={resolveFile}
    mediaType={root.media_type}
    open={true}
    additionalFileIds={resolveAdditionalFileIds.length > 0 ? resolveAdditionalFileIds : undefined}
    onClose={() => {
      resolveFile = null;
      resolveAdditionalFileIds = [];
    }}
    onResolved={handleMatchResolved}
  />
{/if}

{#if reorganizeRootId && root}
  <ReorganizePreview
    rootId={reorganizeRootId}
    rootName={root.name || root.path}
    rootPath={root.path}
    open={true}
    onClose={handleReorganizeClose}
  />
{/if}

{#if reassignOpen && root}
  <EntityReassignModal
    mediaType={root.media_type}
    fileIds={reassignFileIds}
    groupLabel={reassignGroupLabel}
    open={true}
    onClose={() => {
      reassignOpen = false;
      reassignFileIds = [];
      reassignGroupLabel = '';
    }}
    onReassigned={handleReassigned}
  />
{/if}

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
  .loading-page,
  .error-page {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 16px;
    padding: 48px;
    color: var(--muted);
    min-height: 300px;
  }

  .error-page button {
    padding: 8px 16px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    cursor: pointer;
    font-size: 13px;
  }

  .edit-container {
    max-width: 600px;
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
