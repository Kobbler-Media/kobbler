<script lang="ts">
  import type { MediaRootFile } from '$lib/types';
  import { listMediaRootFiles, ignoreMediaRootFile } from '$lib/api/client';
  import { showMessage } from '$lib/context/app.svelte';

  interface Props {
    rootId: string;
    mediaType: 'tv' | 'movie' | 'book' | 'audiobook' | 'sport';
  }

  interface FileGroup {
    key: string;
    title: string;
    year: number | null;
    files: MediaRootFile[];
  }

  let { rootId, mediaType }: Props = $props();

  let files = $state<MediaRootFile[]>([]);
  let loading = $state(false);
  let ignoringGroup = $state<string | null>(null);

  async function loadFiles() {
    loading = true;
    try {
      const [addedResp, reviewResp, identifiedResp] = await Promise.all([
        listMediaRootFiles(rootId, { status: 'added', limit: 200 }),
        listMediaRootFiles(rootId, { status: 'review', limit: 200 }),
        listMediaRootFiles(rootId, { status: 'identified', limit: 200 })
      ]);
      const added = addedResp.items;
      const review = reviewResp.items;
      const identified = identifiedResp.items;
      const seen = new Set<string>();
      const merged: MediaRootFile[] = [];
      for (const f of [...added, ...review, ...identified]) {
        if (!seen.has(f.id)) {
          seen.add(f.id);
          merged.push(f);
        }
      }
      files = merged;
    } catch (e) {
      console.error('Failed to load scan results:', e);
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    void rootId;
    loadFiles();
  });

  function groupKey(file: MediaRootFile): { key: string; title: string; year: number | null } {
    const interp = file.interpretation;
    if (!interp) return { key: '__unknown__', title: 'Unknown', year: null };
    if (mediaType === 'tv') {
      const name = (interp.show_name as string) ?? 'Unknown Show';
      const year = (interp.show_year as number) ?? null;
      return { key: `tv:${name.toLowerCase()}`, title: name, year };
    }
    if (mediaType === 'book' || mediaType === 'audiobook') {
      const title = (interp.series_name as string) ?? (interp.title as string) ?? 'Unknown Book';
      return { key: `book:${title.toLowerCase()}`, title, year: null };
    }
    const title = (interp.movie_title as string) ?? 'Unknown Movie';
    const year = (interp.movie_year as number) ?? null;
    return { key: `movie:${title.toLowerCase()}`, title, year };
  }

  const groups = $derived.by(() => {
    const map = new Map<string, FileGroup>();
    for (const file of files) {
      const { key, title, year } = groupKey(file);
      const existing = map.get(key);
      if (existing) {
        existing.files.push(file);
        if (year && !existing.year) existing.year = year;
      } else {
        map.set(key, { key, title, year, files: [file] });
      }
    }
    return [...map.values()].sort((a, b) => a.title.localeCompare(b.title));
  });

  function handleAdd(group: FileGroup) {
    showMessage(`Add "${group.title}" to library (not yet implemented)`);
  }

  async function handleIgnoreGroup(group: FileGroup) {
    ignoringGroup = group.key;
    try {
      await Promise.all(group.files.map((f) => ignoreMediaRootFile(f.id)));
      files = files.filter((f) => !group.files.some((gf) => gf.id === f.id));
      showMessage(`Ignored ${group.files.length} file(s) for "${group.title}"`);
    } catch (e) {
      showMessage('Failed to ignore files');
      console.error('Ignore failed:', e);
    } finally {
      ignoringGroup = null;
    }
  }
</script>

<div class="scan-results">
  {#if loading}
    <div class="status">Loading scan results...</div>
  {:else if groups.length === 0}
    <div class="status">No identified media waiting for review.</div>
  {:else}
    <div class="group-grid">
      {#each groups as group (group.key)}
        {@const busy = ignoringGroup === group.key}
        <div class="group-card" class:busy>
          <div class="group-header">
            <h4 class="group-title">{group.title}</h4>
            <div class="group-meta">
              {#if group.year}
                <span class="year">{group.year}</span>
              {/if}
              <span class="file-count"
                >{group.files.length} file{group.files.length !== 1 ? 's' : ''}</span
              >
            </div>
          </div>
          <div class="group-actions">
            <button
              type="button"
              class="btn btn-add"
              disabled={busy}
              onclick={() => handleAdd(group)}>Add to Library</button
            >
            <button
              type="button"
              class="btn btn-ignore"
              disabled={busy}
              onclick={() => handleIgnoreGroup(group)}>{busy ? 'Ignoring...' : 'Ignore'}</button
            >
          </div>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .scan-results {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .status {
    text-align: center;
    padding: 32px;
    color: var(--muted);
    font-size: 13px;
  }

  .group-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: 12px;
  }

  .group-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    transition: border-color 0.15s;
  }
  .group-card:hover {
    border-color: var(--accent);
  }
  .group-card.busy {
    opacity: 0.6;
    pointer-events: none;
  }

  .group-header {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .group-title {
    margin: 0;
    font-size: 14px;
    font-weight: 700;
    color: var(--ink);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .group-meta {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--muted);
  }

  .year {
    padding: 1px 6px;
    border-radius: 4px;
    font-weight: 600;
    background: rgba(255, 255, 255, 0.06);
    font-variant-numeric: tabular-nums;
  }

  .file-count {
    font-variant-numeric: tabular-nums;
  }

  .group-actions {
    display: flex;
    gap: 8px;
  }

  .btn {
    flex: 1;
    padding: 7px 12px;
    border-radius: 8px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    border: none;
    transition: all 0.15s;
  }

  .btn-add {
    background: var(--accent);
    color: #1d130c;
  }
  .btn-add:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
  }

  .btn-ignore {
    background: transparent;
    border: 1px solid var(--ring);
    color: var(--muted);
  }
  .btn-ignore:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.04);
    color: var(--ink);
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
