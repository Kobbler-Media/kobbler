<script lang="ts">
  import type { MediaRootFile, MediaRootFileLite, DirectoryEntry } from '$lib/types';
  import { untrack } from 'svelte';
  import {
    listMediaRootFilesLite,
    listMediaRootDirectories,
    directoryAction
  } from '$lib/api/client';
  import { showMessage } from '$lib/context/app.svelte';
  import MediaRootFileRow from './MediaRootFileRow.svelte';

  interface Props {
    rootId: string;
    rootPath: string;
    onResolve?: (file: MediaRootFileLite) => void;
    onInspect?: (file: MediaRootFileLite) => void;
    onResolveSelected?: (fileIds: string[], rootId: string) => void;
  }

  let { rootId, rootPath: _rootPath, onResolve, onInspect, onResolveSelected }: Props = $props();

  interface DirState {
    entries: DirectoryEntry[];
    files: MediaRootFileLite[];
    loaded: boolean;
    loading: boolean;
    expanded: boolean;
  }

  // Map from directory path ('' for root) to its state
  let dirStates = $state<Map<string, DirState>>(new Map());
  let topLoading = $state(true);

  async function loadDirectory(dirPath: string) {
    const existing = dirStates.get(dirPath);
    if (existing?.loaded) {
      // Toggle expansion
      dirStates.set(dirPath, { ...existing, expanded: !existing.expanded });
      dirStates = new Map(dirStates);
      return;
    }

    // Mark as loading
    dirStates.set(dirPath, {
      entries: [],
      files: [],
      loaded: false,
      loading: true,
      expanded: true
    });
    dirStates = new Map(dirStates);

    try {
      const parent = dirPath || undefined;
      const [entries, fileResp] = await Promise.all([
        listMediaRootDirectories(rootId, parent),
        listMediaRootFilesLite(rootId, { directory: dirPath, limit: 5000 })
      ]);

      dirStates.set(dirPath, {
        entries: entries.sort((a, b) => a.name.localeCompare(b.name)),
        files: fileResp.items.sort((a, b) => a.file_name.localeCompare(b.file_name)),
        loaded: true,
        loading: false,
        expanded: true
      });
      dirStates = new Map(dirStates);
    } catch {
      showMessage('Failed to load directory');
      dirStates.set(dirPath, {
        entries: [],
        files: [],
        loaded: false,
        loading: false,
        expanded: false
      });
      dirStates = new Map(dirStates);
    }
  }

  async function loadTopLevel() {
    topLoading = true;
    try {
      const [entries, fileResp] = await Promise.all([
        listMediaRootDirectories(rootId),
        listMediaRootFilesLite(rootId, { directory: '', limit: 5000 })
      ]);

      dirStates = new Map();
      dirStates.set('', {
        entries: entries.sort((a, b) => a.name.localeCompare(b.name)),
        files: fileResp.items.sort((a, b) => a.file_name.localeCompare(b.file_name)),
        loaded: true,
        loading: false,
        expanded: true
      });
      dirStates = new Map(dirStates);
    } catch {
      showMessage('Failed to load directories');
    } finally {
      topLoading = false;
    }
  }

  export function refresh() {
    dirStates = new Map();
    loadTopLevel();
  }

  $effect(() => {
    const _id = rootId;
    untrack(() => loadTopLevel());
  });

  function toggleDir(dirPath: string) {
    const existing = dirStates.get(dirPath);
    if (existing?.loaded) {
      dirStates.set(dirPath, { ...existing, expanded: !existing.expanded });
      dirStates = new Map(dirStates);
    } else {
      loadDirectory(dirPath);
    }
  }

  function handleStatusChange(fileId: string, newStatus: MediaRootFile['match_status']) {
    for (const [key, state] of dirStates) {
      const idx = state.files.findIndex((f) => f.id === fileId);
      if (idx !== -1) {
        const updatedFiles = [...state.files];
        updatedFiles[idx] = { ...updatedFiles[idx], match_status: newStatus };
        dirStates.set(key, { ...state, files: updatedFiles });
        dirStates = new Map(dirStates);
        break;
      }
    }
  }

  function getState(path: string): DirState | undefined {
    return dirStates.get(path);
  }

  let actionLoading = $state<string | null>(null);

  async function handleDirectoryAction(dirPath: string, action: 'ignore' | 'rematch') {
    actionLoading = `${dirPath}:${action}`;
    try {
      const result = await directoryAction(rootId, dirPath, action);
      showMessage(`${action === 'ignore' ? 'Ignored' : 'Rematched'} ${result.count} files`);
      // Reload the directory to reflect changes
      const state = dirStates.get(dirPath);
      if (state) {
        dirStates.set(dirPath, { ...state, loaded: false });
        dirStates = new Map(dirStates);
      }
      loadDirectory(dirPath);
    } catch {
      showMessage(`Failed to ${action} directory files`);
    } finally {
      actionLoading = null;
    }
  }

  function handleResolveAll(dirPath: string) {
    if (!onResolveSelected) return;
    const state = dirStates.get(dirPath);
    if (!state?.files.length) return;
    const fileIds = state.files.map((f) => f.id);
    onResolveSelected(fileIds, rootId);
  }
</script>

{#snippet dirEntries(entries: DirectoryEntry[], depth: number)}
  {#each entries as entry (entry.path)}
    {@const state = getState(entry.path)}
    {@const expanded = state?.expanded ?? false}
    <div class="dir-entry">
      <div class="dir-summary-row" style="padding-left: {depth * 16 + 8}px">
        <button class="dir-summary" onclick={() => toggleDir(entry.path)} type="button">
          <span class="dir-chevron" class:expanded></span>
          <span class="material-icons-outlined dir-icon">folder</span>
          <span class="dir-name">{entry.name}</span>
          <span class="dir-status-badges">
            {#if entry.review_count > 0}
              <span class="dir-badge dir-badge-review">{entry.review_count} review</span>
            {/if}
            {#if entry.ambiguous_count > 0}
              <span class="dir-badge dir-badge-ambiguous">{entry.ambiguous_count} ambiguous</span>
            {/if}
            {#if entry.unmatched_count > 0}
              <span class="dir-badge dir-badge-unmatched">{entry.unmatched_count} unmatched</span>
            {/if}
            {#if entry.added_count > 0}
              <span class="dir-badge dir-badge-added">{entry.added_count} added</span>
            {/if}
          </span>
          <span class="dir-count">{entry.file_count}</span>
        </button>
        {#if expanded}
          <div class="dir-actions">
            {#if onResolveSelected}
              <button
                class="dir-action-btn dir-action-resolve"
                type="button"
                disabled={actionLoading !== null || !getState(entry.path)?.files.length}
                onclick={(e) => {
                  e.stopPropagation();
                  handleResolveAll(entry.path);
                }}
              >
                Resolve All
              </button>
            {/if}
            <button
              class="dir-action-btn"
              type="button"
              disabled={actionLoading !== null}
              onclick={(e) => {
                e.stopPropagation();
                handleDirectoryAction(entry.path, 'rematch');
              }}
            >
              {actionLoading === `${entry.path}:rematch` ? 'Rematching...' : 'Rematch All'}
            </button>
            <button
              class="dir-action-btn dir-action-ignore"
              type="button"
              disabled={actionLoading !== null}
              onclick={(e) => {
                e.stopPropagation();
                handleDirectoryAction(entry.path, 'ignore');
              }}
            >
              {actionLoading === `${entry.path}:ignore` ? 'Ignoring...' : 'Ignore All'}
            </button>
          </div>
        {/if}
      </div>

      {#if expanded}
        <div class="dir-contents">
          {#if state?.loading}
            <div class="loading-row" style="padding-left: {(depth + 1) * 16 + 8}px">Loading...</div>
          {:else if state?.loaded}
            {#each state.files as file (file.id)}
              <div style="padding-left: {(depth + 1) * 16 + 8}px">
                <MediaRootFileRow
                  {file}
                  {onResolve}
                  {onInspect}
                  onStatusChange={handleStatusChange}
                />
              </div>
            {/each}
            {#if state.entries.length > 0}
              {@render dirEntries(state.entries, depth + 1)}
            {/if}
          {/if}
        </div>
      {/if}
    </div>
  {/each}
{/snippet}

<div class="directory-view">
  {#if topLoading}
    <div class="empty">Loading directories...</div>
  {:else}
    {@const rootState = getState('')}
    {#if rootState}
      {#each rootState.files as file (file.id)}
        <div style="padding-left: 8px">
          <MediaRootFileRow {file} {onResolve} {onInspect} onStatusChange={handleStatusChange} />
        </div>
      {/each}
      {#if rootState.entries.length > 0}
        {@render dirEntries(rootState.entries, 0)}
      {:else if rootState.files.length === 0}
        <div class="empty">No files in this media root.</div>
      {/if}
    {:else}
      <div class="empty">No files in this media root.</div>
    {/if}
  {/if}
</div>

<style>
  .directory-view {
    display: flex;
    flex-direction: column;
    font-size: 13px;
  }

  .dir-entry {
    border-bottom: 1px solid var(--ring);
  }

  .dir-summary-row {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .dir-summary {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
    cursor: pointer;
    user-select: none;
    flex: 1;
    min-width: 0;
    border: none;
    background: none;
    font: inherit;
    color: inherit;
    text-align: left;
  }

  .dir-summary:hover {
    background: rgba(255, 255, 255, 0.03);
  }

  .dir-status-badges {
    display: flex;
    gap: 4px;
    margin-left: 8px;
  }

  .dir-badge {
    font-size: 10px;
    padding: 1px 5px;
    border-radius: 3px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .dir-badge-review {
    background: rgba(34, 197, 94, 0.15);
    color: #22c55e;
  }

  .dir-badge-ambiguous {
    background: rgba(245, 158, 11, 0.15);
    color: #f59e0b;
  }

  .dir-badge-unmatched {
    background: rgba(239, 68, 68, 0.15);
    color: #ef4444;
  }

  .dir-badge-added {
    background: rgba(148, 163, 184, 0.15);
    color: #94a3b8;
  }

  .dir-actions {
    display: flex;
    gap: 4px;
    padding-right: 8px;
    flex-shrink: 0;
  }

  .dir-action-btn {
    font-size: 11px;
    padding: 2px 8px;
    border: 1px solid var(--ring);
    border-radius: 4px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    white-space: nowrap;
  }

  .dir-action-btn:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.05);
    color: var(--ink);
  }

  .dir-action-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .dir-action-resolve:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }

  .dir-action-ignore:hover:not(:disabled) {
    border-color: #ef4444;
    color: #ef4444;
  }

  .dir-chevron {
    display: inline-block;
    width: 0;
    height: 0;
    border-left: 5px solid var(--muted);
    border-top: 4px solid transparent;
    border-bottom: 4px solid transparent;
    transition: transform 0.15s;
    flex-shrink: 0;
  }

  .dir-chevron.expanded {
    transform: rotate(90deg);
  }

  .dir-icon {
    font-size: 16px;
    color: var(--accent);
  }

  .dir-name {
    font-weight: 600;
    color: var(--ink);
  }

  .dir-count {
    font-size: 11px;
    color: var(--muted);
    margin-left: auto;
    font-variant-numeric: tabular-nums;
  }

  .loading-row {
    padding: 6px 8px;
    color: var(--muted);
    font-size: 12px;
  }

  .empty {
    padding: 32px;
    text-align: center;
    color: var(--muted);
    font-size: 13px;
  }
</style>
