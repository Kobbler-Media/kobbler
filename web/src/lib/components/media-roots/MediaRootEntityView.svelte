<script lang="ts">
  import type { MediaRootFile, MediaRootFileLite, EntityGroupSummary } from '$lib/types';
  import { untrack } from 'svelte';
  import MetaLine from '$lib/components/shared/MetaLine.svelte';
  import {
    listEntityGroups,
    listMediaRootFilesLite,
    bulkResolveMediaRootFiles,
    bulkRematchMediaRootFiles,
    bulkIgnoreMediaRootFiles
  } from '$lib/api/client';
  import { showMessage } from '$lib/context/app.svelte';
  import { getJobsState } from '$lib/stores/jobs.svelte';
  import MediaRootFileRow from './MediaRootFileRow.svelte';

  type EntityStatusFilter = 'all' | 'needs_action' | 'added';

  interface Props {
    rootId: string;
    mediaType: 'tv' | 'movie' | 'book' | 'audiobook' | 'sport';
    onResolve?: (file: MediaRootFileLite) => void;
    onInspect?: (file: MediaRootFileLite) => void;
    onGroupResolve?: (groupKey: string, rootId: string) => void;
    onReassign?: (groupKey: string, rootId: string) => void;
    /** Called with selected file IDs to open MatchResolver for split/move operations. */
    onResolveSelected?: (fileIds: string[], rootId: string) => void;
    onUpdate?: () => void;
  }

  let {
    rootId,
    mediaType,
    onResolve,
    onInspect,
    onGroupResolve,
    onReassign,
    onResolveSelected,
    onUpdate
  }: Props = $props();

  let statusFilter = $state<EntityStatusFilter>('all');

  let groups = $state<EntityGroupSummary[]>([]);
  let ungroupedCount = $state(0);
  let loading = $state(true);
  let loadGeneration = 0;

  // Expanded group state: group_key -> loaded files
  let expandedFiles = $state<Map<string, MediaRootFileLite[]>>(new Map());
  let loadingGroups = $state<Set<string>>(new Set());

  // For TV: track expanded seasons within a group: "groupKey:season" -> files
  let expandedSeasons = $state<Map<string, MediaRootFileLite[]>>(new Map());
  let loadingSeasons = $state<Set<string>>(new Set());

  let addingGroups = $state<Set<string>>(new Set());
  let rematchingGroups = $state<Set<string>>(new Set());

  // File selection state per group: groupKey -> Set<fileId>
  let selectedFiles = $state<Map<string, Set<string>>>(new Map());

  function toggleFileSelection(groupKey: string, fileId: string) {
    const current = selectedFiles.get(groupKey) ?? new Set<string>();
    const next = new Set(current);
    if (next.has(fileId)) {
      next.delete(fileId);
    } else {
      next.add(fileId);
    }
    selectedFiles = new Map([...selectedFiles, [groupKey, next]]);
  }

  function toggleSelectAll(groupKey: string) {
    const files = expandedFiles.get(groupKey) ?? [];
    const current = selectedFiles.get(groupKey) ?? new Set<string>();
    if (current.size === files.length) {
      // Deselect all
      selectedFiles = new Map([...selectedFiles, [groupKey, new Set<string>()]]);
    } else {
      // Select all
      selectedFiles = new Map([...selectedFiles, [groupKey, new Set(files.map((f) => f.id))]]);
    }
  }

  function getSelectedIds(groupKey: string): string[] {
    return [...(selectedFiles.get(groupKey) ?? [])];
  }

  function clearSelection(groupKey: string) {
    selectedFiles = new Map([...selectedFiles, [groupKey, new Set<string>()]]);
  }

  async function handleIgnoreSelected(groupKey: string) {
    const ids = getSelectedIds(groupKey);
    if (!ids.length) return;
    try {
      await bulkIgnoreMediaRootFiles(ids);
      showMessage(`Ignored ${ids.length} ${ids.length === 1 ? 'file' : 'files'}`);
      clearSelection(groupKey);
      const files = expandedFiles.get(groupKey);
      if (files) {
        const idSet = new Set(ids);
        expandedFiles = new Map([
          ...expandedFiles,
          [
            groupKey,
            files.map((f) => (idSet.has(f.id) ? { ...f, match_status: 'ignored' as const } : f))
          ]
        ]);
      }
      scheduleGroupRefresh();
      onUpdate?.();
    } catch {
      showMessage('Ignore failed');
    }
  }

  async function handleRematchSelected(groupKey: string) {
    const ids = getSelectedIds(groupKey);
    if (!ids.length) return;
    try {
      await bulkRematchMediaRootFiles(ids);
      showMessage(`Rematching ${ids.length} ${ids.length === 1 ? 'file' : 'files'}`);
      clearSelection(groupKey);
      const files = expandedFiles.get(groupKey);
      if (files) {
        const idSet = new Set(ids);
        expandedFiles = new Map([
          ...expandedFiles,
          [
            groupKey,
            files.map((f) => (idSet.has(f.id) ? { ...f, match_status: 'pending' as const } : f))
          ]
        ]);
      }
      scheduleGroupRefresh();
      onUpdate?.();
    } catch {
      showMessage('Rematch failed');
    }
  }
  let pendingAddJobs = $state<Map<string, { jobId: string; fileIds: string[] }>>(new Map());
  const jobStore = getJobsState();

  async function loadGroups() {
    const gen = ++loadGeneration;
    loading = true;
    try {
      const resp = await listEntityGroups(rootId);
      if (gen !== loadGeneration) return;
      groups = resp.groups;
      ungroupedCount = resp.ungrouped_count;
    } catch {
      showMessage('Failed to load entity groups');
    } finally {
      if (gen === loadGeneration) loading = false;
    }
  }

  export function refresh() {
    // Only refresh group summaries (counts); preserve expanded state and scroll position
    loadGroups();
  }

  $effect(() => {
    const _id = rootId;
    untrack(() => loadGroups());
  });

  async function handleGroupToggle(groupKey: string, open: boolean) {
    if (!open || expandedFiles.has(groupKey)) return;

    loadingGroups = new Set([...loadingGroups, groupKey]);
    try {
      const resp = await listMediaRootFilesLite(rootId, {
        group_key: groupKey,
        limit: 5000
      });
      expandedFiles = new Map([...expandedFiles, [groupKey, resp.items]]);
    } catch {
      showMessage('Failed to load group files');
    } finally {
      loadingGroups = new Set([...loadingGroups].filter((k) => k !== groupKey));
    }
  }

  async function handleSeasonToggle(groupKey: string, season: number, open: boolean) {
    const key = `${groupKey}:${season}`;
    if (!open || expandedSeasons.has(key)) return;

    loadingSeasons = new Set([...loadingSeasons, key]);
    try {
      const resp = await listMediaRootFilesLite(rootId, {
        group_key: groupKey,
        season,
        limit: 5000
      });
      expandedSeasons = new Map([...expandedSeasons, [key, resp.items]]);
    } catch {
      showMessage('Failed to load season files');
    } finally {
      loadingSeasons = new Set([...loadingSeasons].filter((k) => k !== key));
    }
  }

  function fileLabel(file: MediaRootFileLite): string {
    const interp = file.interpretation;
    if (!interp) return file.file_name;
    if (mediaType === 'tv' || mediaType === 'sport') {
      const episodes = interp.episodes as number[] | undefined;
      const ep = episodes?.[0];
      const title = interp.episode_title as string | undefined;
      const parts: string[] = [];
      if (ep != null) parts.push(`E${String(ep).padStart(2, '0')}`);
      if (title) parts.push(title);
      return parts.length ? parts.join(' - ') : file.file_name;
    }
    if (mediaType === 'book' || mediaType === 'audiobook') {
      const title = interp.title as string | undefined;
      const authors = interp.authors as string[] | undefined;
      const idx = interp.series_index as number | undefined;
      const parts: string[] = [];
      if (idx != null) parts.push(`#${idx}`);
      if (title) parts.push(title);
      if (authors?.length) parts.push(`(${authors.join(', ')})`);
      return parts.length ? parts.join(' ') : file.file_name;
    }
    return file.file_name;
  }

  function handleStatusChange(
    fileId: string,
    newStatus: MediaRootFile['match_status'],
    groupKey: string
  ) {
    // Update the file in the expanded files map
    const files = expandedFiles.get(groupKey);
    if (files) {
      expandedFiles = new Map([
        ...expandedFiles,
        [groupKey, files.map((f) => (f.id === fileId ? { ...f, match_status: newStatus } : f))]
      ]);
    }

    // Also check season maps for TV
    for (const [key, seasonFiles] of expandedSeasons) {
      if (key.startsWith(`${groupKey}:`)) {
        const updated = seasonFiles.map((f) =>
          f.id === fileId ? { ...f, match_status: newStatus } : f
        );
        if (updated !== seasonFiles) {
          expandedSeasons = new Map([...expandedSeasons, [key, updated]]);
        }
      }
    }
  }

  // Debounced group refresh after status changes
  let refreshTimer: ReturnType<typeof setTimeout> | null = null;
  function scheduleGroupRefresh() {
    if (refreshTimer) clearTimeout(refreshTimer);
    refreshTimer = setTimeout(async () => {
      try {
        const resp = await listEntityGroups(rootId);
        groups = resp.groups;
        ungroupedCount = resp.ungrouped_count;
      } catch {
        // ignore
      }
    }, 500);
  }

  function handleStatusChangeAndRefresh(
    fileId: string,
    newStatus: MediaRootFile['match_status'],
    groupKey: string
  ) {
    handleStatusChange(fileId, newStatus, groupKey);
    scheduleGroupRefresh();
  }

  function fileCountLabel(count: number): string {
    if (mediaType === 'tv' || mediaType === 'sport')
      return count === 1 ? '1 episode' : `${count} episodes`;
    if (mediaType === 'movie') return count === 1 ? '1 file' : `${count} files`;
    if (mediaType === 'audiobook') return count === 1 ? '1 file' : `${count} files`;
    return count === 1 ? '1 book' : `${count} books`;
  }

  function needsAction(group: EntityGroupSummary): boolean {
    return group.review_count > 0 || group.unmatched_count > 0 || group.ambiguous_count > 0;
  }

  // Add confirmation state
  let confirmingAdd = $state<string | null>(null);

  function handleAddGroupClick(group: EntityGroupSummary) {
    confirmingAdd = group.group_key;
  }

  function cancelAdd() {
    confirmingAdd = null;
  }

  async function confirmAddGroup(groupKey: string) {
    confirmingAdd = null;
    addingGroups = new Set([...addingGroups, groupKey]);
    try {
      // Fetch review file IDs for this group on demand
      const resp = await listMediaRootFilesLite(rootId, {
        group_key: groupKey,
        status: 'review',
        limit: 5000
      });
      const ids = resp.items.map((f) => f.id);
      if (!ids.length) {
        addingGroups = new Set([...addingGroups].filter((k) => k !== groupKey));
        return;
      }
      const { job_id } = await bulkResolveMediaRootFiles(ids);
      pendingAddJobs = new Map([...pendingAddJobs, [groupKey, { jobId: job_id, fileIds: ids }]]);
    } catch {
      showMessage('Add failed');
      addingGroups = new Set([...addingGroups].filter((k) => k !== groupKey));
    }
  }

  // Watch job store for completion of bulk resolve jobs
  $effect(() => {
    if (pendingAddJobs.size === 0) return;
    for (const [groupKey, { jobId, fileIds }] of pendingAddJobs) {
      const job = jobStore.jobs.find((j) => j.id === jobId);
      if (!job) continue;

      if (job.status === 'completed') {
        const r = (job.result ?? {}) as Record<string, unknown>;
        const resolved = typeof r.resolved === 'number' ? r.resolved : 0;
        const failed = typeof r.failed === 'number' ? r.failed : 0;
        if (resolved > 0) {
          showMessage(`Added ${resolved} ${resolved === 1 ? 'file' : 'files'}`);
          untrack(() => {
            const idSet = new Set(fileIds);
            // Update expanded files if present
            const files = expandedFiles.get(groupKey);
            if (files) {
              expandedFiles = new Map([
                ...expandedFiles,
                [
                  groupKey,
                  files.map((f) => (idSet.has(f.id) ? { ...f, match_status: 'added' as const } : f))
                ]
              ]);
            }
            scheduleGroupRefresh();
          });
          onUpdate?.();
        }
        if (failed > 0) showMessage(`${failed} failed to add`);
        addingGroups = new Set([...addingGroups].filter((k) => k !== groupKey));
        pendingAddJobs = new Map([...pendingAddJobs].filter(([k]) => k !== groupKey));
      } else if (job.status === 'failed') {
        showMessage(job.error ?? 'Add failed');
        addingGroups = new Set([...addingGroups].filter((k) => k !== groupKey));
        pendingAddJobs = new Map([...pendingAddJobs].filter(([k]) => k !== groupKey));
      }
    }
  });

  async function handleRematchGroup(groupKey: string) {
    rematchingGroups = new Set([...rematchingGroups, groupKey]);
    try {
      const resp = await listMediaRootFilesLite(rootId, {
        group_key: groupKey,
        status: 'review',
        limit: 5000
      });
      const ids = resp.items.map((f) => f.id);
      if (!ids.length) {
        rematchingGroups = new Set([...rematchingGroups].filter((k) => k !== groupKey));
        return;
      }
      await bulkRematchMediaRootFiles(ids);
      showMessage(`Rematching ${ids.length} ${ids.length === 1 ? 'file' : 'files'}`);
      // Update expanded files
      const files = expandedFiles.get(groupKey);
      if (files) {
        const idSet = new Set(ids);
        expandedFiles = new Map([
          ...expandedFiles,
          [
            groupKey,
            files.map((f) => (idSet.has(f.id) ? { ...f, match_status: 'pending' as const } : f))
          ]
        ]);
      }
      scheduleGroupRefresh();
      onUpdate?.();
    } catch {
      showMessage('Rematch failed');
    } finally {
      rematchingGroups = new Set([...rematchingGroups].filter((k) => k !== groupKey));
    }
  }

  // Series-level bulk operations
  async function handleIgnoreSeries(series: SeriesNest) {
    const allIds: string[] = [];
    for (const book of series.books) {
      const resp = await listMediaRootFilesLite(rootId, {
        group_key: book.group_key,
        limit: 5000
      });
      allIds.push(...resp.items.map((f) => f.id));
    }
    if (!allIds.length) return;
    try {
      await bulkIgnoreMediaRootFiles(allIds);
      showMessage(`Ignored ${allIds.length} ${allIds.length === 1 ? 'file' : 'files'}`);
      scheduleGroupRefresh();
      onUpdate?.();
    } catch {
      showMessage('Ignore failed');
    }
  }

  async function handleRematchSeries(series: SeriesNest) {
    const allIds: string[] = [];
    for (const book of series.books) {
      const resp = await listMediaRootFilesLite(rootId, {
        group_key: book.group_key,
        limit: 5000
      });
      allIds.push(...resp.items.map((f) => f.id));
    }
    if (!allIds.length) return;
    try {
      await bulkRematchMediaRootFiles(allIds);
      showMessage(`Rematching ${allIds.length} ${allIds.length === 1 ? 'file' : 'files'}`);
      scheduleGroupRefresh();
      onUpdate?.();
    } catch {
      showMessage('Rematch failed');
    }
  }

  function handleReassignSeries(series: SeriesNest) {
    if (!onResolveSelected) return;
    const allIds: string[] = [];
    for (const book of series.books) {
      const files = expandedFiles.get(book.group_key);
      if (files) {
        allIds.push(...files.map((f) => f.id));
      }
    }
    if (allIds.length > 0) {
      onResolveSelected(allIds, rootId);
    }
  }

  function handleSelectAllSeries(series: SeriesNest) {
    let totalFiles = 0;
    let totalSelected = 0;
    for (const book of series.books) {
      const files = expandedFiles.get(book.group_key) ?? [];
      const sel = selectedFiles.get(book.group_key) ?? new Set<string>();
      totalFiles += files.length;
      totalSelected += sel.size;
    }
    const selectAll = totalSelected < totalFiles;
    const next = new Map(selectedFiles);
    for (const book of series.books) {
      const files = expandedFiles.get(book.group_key) ?? [];
      if (selectAll) {
        next.set(book.group_key, new Set(files.map((f) => f.id)));
      } else {
        next.set(book.group_key, new Set<string>());
      }
    }
    selectedFiles = next;
  }

  function getSeriesSelectedCount(series: SeriesNest): number {
    let count = 0;
    for (const book of series.books) {
      count += (selectedFiles.get(book.group_key) ?? new Set()).size;
    }
    return count;
  }

  function getSeriesFileCount(series: SeriesNest): number {
    let count = 0;
    for (const book of series.books) {
      count += (expandedFiles.get(book.group_key) ?? []).length;
    }
    return count;
  }

  // Build virtual group entry for ungrouped files
  const allGroupsUnfiltered = $derived.by(() => {
    const result = [...groups];
    if (ungroupedCount > 0) {
      result.push({
        group_key: '__ungrouped__',
        label: 'Ungrouped',
        year: null,
        file_count: ungroupedCount,
        review_count: 0,
        unmatched_count: ungroupedCount,
        ambiguous_count: 0,
        added_count: 0,
        poster_url: null,
        subgroups: [],
        group_status: 'mixed' as const,
        authors: null,
        series_name: null,
        series_index: null,
        provider: null,
        provider_id: null,
        confidence: null,
        cover_url: null
      });
    }
    return result;
  });

  const allGroups = $derived.by(() => {
    if (statusFilter === 'all') return allGroupsUnfiltered;
    if (statusFilter === 'needs_action') {
      return allGroupsUnfiltered.filter(
        (g) => g.review_count > 0 || g.unmatched_count > 0 || g.ambiguous_count > 0
      );
    }
    // 'added' — only fully resolved groups
    return allGroupsUnfiltered.filter(
      (g) =>
        g.added_count > 0 &&
        g.review_count === 0 &&
        g.unmatched_count === 0 &&
        g.ambiguous_count === 0
    );
  });

  let searchQuery = $state('');

  const allGroupsFiltered = $derived.by(() => {
    const q = searchQuery.trim().toLowerCase();
    if (!q) return allGroups;
    return allGroups.filter(
      (g) =>
        g.label.toLowerCase().includes(q) || (g.series_name?.toLowerCase().includes(q) ?? false)
    );
  });

  interface SeriesNest {
    seriesName: string;
    books: EntityGroupSummary[];
    totalFiles: number;
    reviewCount: number;
    ambiguousCount: number;
    unmatchedCount: number;
    addedCount: number;
    groupStatus: 'review' | 'ambiguous' | 'added' | 'mixed';
    authors: string[] | null;
    coverUrl: string | null;
  }

  function computeGroupStatus(
    review: number,
    ambiguous: number,
    unmatched: number,
    added: number
  ): 'review' | 'ambiguous' | 'added' | 'mixed' {
    const total = review + ambiguous + unmatched + added;
    if (total === 0) return 'mixed';
    if (added === total) return 'added';
    if (review === total) return 'review';
    if (ambiguous > 0 || unmatched > 0) return 'ambiguous';
    return 'mixed';
  }

  const seriesNested = $derived.by(() => {
    if (mediaType !== 'book' && mediaType !== 'audiobook') return null;

    const seriesMap = new Map<string, EntityGroupSummary[]>();
    const standalone: EntityGroupSummary[] = [];

    for (const g of allGroupsFiltered) {
      if (g.series_name) {
        const existing = seriesMap.get(g.series_name) ?? [];
        existing.push(g);
        seriesMap.set(g.series_name, existing);
      } else {
        standalone.push(g);
      }
    }

    const seriesGroups: SeriesNest[] = [];
    for (const [name, books] of seriesMap) {
      books.sort((a, b) => (a.series_index ?? 999) - (b.series_index ?? 999));
      if (books.length === 1) {
        standalone.push(books[0]);
        continue;
      }
      const reviewCount = books.reduce((s, b) => s + b.review_count, 0);
      const ambiguousCount = books.reduce((s, b) => s + b.ambiguous_count, 0);
      const unmatchedCount = books.reduce((s, b) => s + b.unmatched_count, 0);
      const addedCount = books.reduce((s, b) => s + b.added_count, 0);
      seriesGroups.push({
        seriesName: name,
        books,
        totalFiles: books.reduce((s, b) => s + b.file_count, 0),
        reviewCount,
        ambiguousCount,
        unmatchedCount,
        addedCount,
        groupStatus: computeGroupStatus(reviewCount, ambiguousCount, unmatchedCount, addedCount),
        authors: books[0].authors,
        coverUrl: books[0].cover_url || books[0].poster_url
      });
    }

    seriesGroups.sort((a, b) => a.seriesName.localeCompare(b.seriesName));
    return { seriesGroups, standalone };
  });

  const needsActionCount = $derived(
    allGroupsUnfiltered.filter(
      (g) => g.review_count > 0 || g.unmatched_count > 0 || g.ambiguous_count > 0
    ).length
  );
  const addedOnlyCount = $derived(
    allGroupsUnfiltered.filter(
      (g) =>
        g.added_count > 0 &&
        g.review_count === 0 &&
        g.unmatched_count === 0 &&
        g.ambiguous_count === 0
    ).length
  );
</script>

{#snippet bookGroup(group: EntityGroupSummary)}
  {@const hasAction = needsAction(group)}
  {@const isAdding = addingGroups.has(group.group_key)}
  {@const isRematching = rematchingGroups.has(group.group_key)}
  {@const isLoadingFiles = loadingGroups.has(group.group_key)}
  <details
    class="entity-group"
    ontoggle={(e) => {
      const det = e.currentTarget as HTMLDetailsElement;
      handleGroupToggle(group.group_key, det.open);
    }}
  >
    <summary class="group-header" class:has-review={hasAction}>
      {#if group.cover_url || group.poster_url}
        <img class="group-poster" src={group.cover_url || group.poster_url} alt="" />
      {:else}
        <span class="material-icons-outlined group-icon">
          {mediaType === 'tv'
            ? 'live_tv'
            : mediaType === 'sport'
              ? 'sports_motorsports'
              : mediaType === 'movie'
                ? 'movie'
                : mediaType === 'audiobook'
                  ? 'headphones'
                  : 'menu_book'}
        </span>
      {/if}
      <span class="group-info">
        <span class="group-label">
          {group.label}{#if group.year}&nbsp;({group.year}){/if}
        </span>
        {#if group.series_name}
          <span class="group-series">
            {group.series_name}{#if group.series_index != null}&nbsp;#{group.series_index}{/if}
          </span>
        {/if}
        {#if group.authors?.length}
          <span class="group-authors">{group.authors.join(', ')}</span>
        {/if}
        <span class="group-meta">
          <span class="status-badge status-{group.group_status}">
            {group.group_status === 'review'
              ? 'Ready to add'
              : group.group_status === 'ambiguous'
                ? 'Needs review'
                : group.group_status === 'added'
                  ? 'Added'
                  : 'Mixed'}
          </span>
          <span class="group-count">{fileCountLabel(group.file_count)}</span>
          {#if group.review_count > 0 && group.review_count < group.file_count}
            <span class="review-count">{group.review_count} review</span>
          {/if}
          {#if group.ambiguous_count > 0}
            <span class="ambiguous-count">{group.ambiguous_count} ambiguous</span>
          {/if}
          {#if group.unmatched_count > 0}
            <span class="unmatched-count">{group.unmatched_count} unmatched</span>
          {/if}
          {#if group.provider}
            <span class="provider-badge">{group.provider}</span>
          {/if}
          {#if group.confidence != null}
            <span class="confidence" class:low={group.confidence < 0.7}
              >{Math.round(group.confidence * 100)}%</span
            >
          {/if}
        </span>
      </span>
      <span class="group-actions">
        {#if (group.unmatched_count > 0 || group.ambiguous_count > 0) && onGroupResolve}
          <button
            type="button"
            class="btn-resolve"
            title="Resolve {group.unmatched_count + group.ambiguous_count} files"
            disabled={isAdding || isRematching}
            onclick={(e) => {
              e.stopPropagation();
              onGroupResolve?.(group.group_key, rootId);
            }}
          >
            <span class="material-icons-outlined">manage_search</span>
            Resolve ({group.unmatched_count + group.ambiguous_count})
          </button>
        {/if}
        {#if group.review_count > 0}
          <button
            type="button"
            class="btn-add"
            title="Add all {group.review_count} review files"
            disabled={isAdding || isRematching}
            onclick={(e) => {
              e.stopPropagation();
              handleAddGroupClick(group);
            }}
          >
            {#if isAdding}
              <span class="material-icons-outlined spin">sync</span>
            {:else}
              <span class="material-icons-outlined">library_add</span>
            {/if}
            Add ({group.review_count})
          </button>
          <button
            type="button"
            class="btn-rematch"
            title="Rematch all review files"
            disabled={isAdding || isRematching}
            onclick={(e) => {
              e.stopPropagation();
              handleRematchGroup(group.group_key);
            }}
          >
            {#if isRematching}
              <span class="material-icons-outlined spin">sync</span>
            {:else}
              <span class="material-icons-outlined">refresh</span>
            {/if}
          </button>
        {/if}
        {#if onReassign && group.group_key !== '__ungrouped__'}
          <button
            type="button"
            class="btn-reassign"
            title="Reassign all files to a different entity"
            onclick={(e) => {
              e.stopPropagation();
              onReassign?.(group.group_key, rootId);
            }}
          >
            <span class="material-icons-outlined">drive_file_move</span>
          </button>
        {/if}
      </span>
    </summary>
    <div class="group-body">
      {#if isLoadingFiles}
        <div class="loading-files">Loading files...</div>
      {:else if (mediaType === 'tv' || mediaType === 'sport') && group.subgroups.length > 0}
        {#each group.subgroups as sub (sub.season)}
          {@const seasonKey = `${group.group_key}:${sub.season}`}
          {@const seasonFiles = expandedSeasons.get(seasonKey)}
          {@const isLoadingSeason = loadingSeasons.has(seasonKey)}
          {@const subIsAdding = addingGroups.has(seasonKey)}
          {@const subIsRematching = rematchingGroups.has(seasonKey)}
          <details
            class="subgroup"
            ontoggle={(e) => {
              const det = e.currentTarget as HTMLDetailsElement;
              handleSeasonToggle(group.group_key, sub.season, det.open);
            }}
          >
            <summary class="subgroup-header">
              <span class="subgroup-label">{sub.label}</span>
              <span class="subgroup-count">{sub.file_count}</span>
              {#if sub.review_count > 0}
                <span class="subgroup-actions">
                  <button
                    type="button"
                    class="btn-add-sm"
                    title="Add {sub.review_count} files"
                    disabled={subIsAdding || subIsRematching}
                    onclick={(e) => {
                      e.stopPropagation();
                      addingGroups = new Set([...addingGroups, seasonKey]);
                      listMediaRootFilesLite(rootId, {
                        group_key: group.group_key,
                        season: sub.season,
                        status: 'review',
                        limit: 5000
                      })
                        .then(async (resp) => {
                          const ids = resp.items.map((f) => f.id);
                          if (!ids.length) return;
                          const { job_id } = await bulkResolveMediaRootFiles(ids);
                          pendingAddJobs = new Map([
                            ...pendingAddJobs,
                            [seasonKey, { jobId: job_id, fileIds: ids }]
                          ]);
                        })
                        .catch(() => {
                          showMessage('Add failed');
                          addingGroups = new Set([...addingGroups].filter((k) => k !== seasonKey));
                        });
                    }}
                  >
                    {#if subIsAdding}
                      <span class="material-icons-outlined spin">sync</span>
                    {:else}
                      <span class="material-icons-outlined">library_add</span>
                    {/if}
                    {sub.review_count}
                  </button>
                  <button
                    type="button"
                    class="btn-rematch-sm"
                    title="Rematch review files"
                    disabled={subIsAdding || subIsRematching}
                    onclick={async (e) => {
                      e.stopPropagation();
                      rematchingGroups = new Set([...rematchingGroups, seasonKey]);
                      try {
                        const resp = await listMediaRootFilesLite(rootId, {
                          group_key: group.group_key,
                          season: sub.season,
                          status: 'review',
                          limit: 5000
                        });
                        const ids = resp.items.map((f) => f.id);
                        if (ids.length) {
                          await bulkRematchMediaRootFiles(ids);
                          showMessage(
                            `Rematching ${ids.length} ${ids.length === 1 ? 'file' : 'files'}`
                          );
                          if (seasonFiles) {
                            const idSet = new Set(ids);
                            expandedSeasons = new Map([
                              ...expandedSeasons,
                              [
                                seasonKey,
                                seasonFiles.map((f) =>
                                  idSet.has(f.id) ? { ...f, match_status: 'pending' as const } : f
                                )
                              ]
                            ]);
                          }
                          scheduleGroupRefresh();
                          onUpdate?.();
                        }
                      } catch {
                        showMessage('Rematch failed');
                      } finally {
                        rematchingGroups = new Set(
                          [...rematchingGroups].filter((k) => k !== seasonKey)
                        );
                      }
                    }}
                  >
                    {#if subIsRematching}
                      <span class="material-icons-outlined spin">sync</span>
                    {:else}
                      <span class="material-icons-outlined">refresh</span>
                    {/if}
                  </button>
                </span>
              {/if}
            </summary>
            <div class="subgroup-body">
              {#if isLoadingSeason}
                <div class="loading-files">Loading...</div>
              {:else if seasonFiles}
                {#each seasonFiles as file (file.id)}
                  <MediaRootFileRow
                    {file}
                    label={fileLabel(file)}
                    {onResolve}
                    {onInspect}
                    onStatusChange={(id, status) =>
                      handleStatusChangeAndRefresh(id, status, group.group_key)}
                  />
                {/each}
              {/if}
            </div>
          </details>
        {/each}
      {:else}
        {@const groupFiles = expandedFiles.get(group.group_key) ?? []}
        {@const groupSelected = selectedFiles.get(group.group_key) ?? new Set()}
        {#if groupFiles.length > 0}
          <div class="selection-toolbar">
            <label class="select-all-label">
              <input
                type="checkbox"
                checked={groupSelected.size === groupFiles.length && groupFiles.length > 0}
                indeterminate={groupSelected.size > 0 && groupSelected.size < groupFiles.length}
                onchange={() => toggleSelectAll(group.group_key)}
              />
              {groupSelected.size > 0 ? `${groupSelected.size} selected` : 'Select all'}
            </label>
            {#if groupSelected.size > 0}
              {#if onResolveSelected}
                <button
                  type="button"
                  class="btn-toolbar"
                  onclick={() => {
                    onResolveSelected?.(getSelectedIds(group.group_key), rootId);
                    clearSelection(group.group_key);
                  }}
                >
                  <span class="material-icons-outlined">manage_search</span>
                  Resolve Selected
                </button>
              {/if}
              <button
                type="button"
                class="btn-toolbar"
                onclick={() => handleIgnoreSelected(group.group_key)}
              >
                <span class="material-icons-outlined">visibility_off</span>
                Ignore
              </button>
              <button
                type="button"
                class="btn-toolbar"
                onclick={() => handleRematchSelected(group.group_key)}
              >
                <span class="material-icons-outlined">refresh</span>
                Rematch
              </button>
            {/if}
          </div>
        {/if}
        {#each groupFiles as file (file.id)}
          <div class="file-row-with-checkbox">
            <input
              type="checkbox"
              class="file-checkbox"
              checked={groupSelected.has(file.id)}
              onchange={() => toggleFileSelection(group.group_key, file.id)}
            />
            <MediaRootFileRow
              {file}
              label={fileLabel(file)}
              {onResolve}
              {onInspect}
              onStatusChange={(id, status) =>
                handleStatusChangeAndRefresh(id, status, group.group_key)}
            />
          </div>
        {/each}
      {/if}
    </div>
  </details>
{/snippet}

<div class="entity-view">
  {#if loading}
    <div class="empty">Loading groups...</div>
  {:else if allGroupsUnfiltered.length === 0}
    <div class="empty">No files to display.</div>
  {:else}
    <div class="entity-toolbar">
      <div class="entity-filters">
        <button
          type="button"
          class="filter-tab"
          class:active={statusFilter === 'all'}
          onclick={() => (statusFilter = 'all')}
        >
          All ({allGroupsUnfiltered.length})
        </button>
        <button
          type="button"
          class="filter-tab"
          class:active={statusFilter === 'needs_action'}
          onclick={() => (statusFilter = 'needs_action')}
        >
          Needs Action ({needsActionCount})
        </button>
        <button
          type="button"
          class="filter-tab"
          class:active={statusFilter === 'added'}
          onclick={() => (statusFilter = 'added')}
        >
          Added ({addedOnlyCount})
        </button>
      </div>
      <input
        class="entity-search"
        type="text"
        placeholder="Filter entities..."
        bind:value={searchQuery}
      />
    </div>
    {#if allGroupsFiltered.length === 0}
      <div class="empty">No entities match this filter.</div>
    {/if}
    {#if seriesNested}
      {#each seriesNested.seriesGroups as series (series.seriesName)}
        {@const seriesHasAction =
          series.reviewCount > 0 || series.unmatchedCount > 0 || series.ambiguousCount > 0}
        <details class="entity-group series-group">
          <summary class="group-header" class:has-review={seriesHasAction}>
            {#if series.coverUrl}
              <img class="group-poster" src={series.coverUrl} alt="" />
            {:else}
              <span class="material-icons-outlined group-icon">
                {mediaType === 'audiobook' ? 'headphones' : 'menu_book'}
              </span>
            {/if}
            <span class="group-info">
              <span class="group-label">{series.seriesName}</span>
              {#if series.authors?.length}
                <span class="group-authors">{series.authors.join(', ')}</span>
              {/if}
              <span class="group-meta">
                <span class="status-badge status-{series.groupStatus}">
                  {series.groupStatus === 'review'
                    ? 'Ready to add'
                    : series.groupStatus === 'ambiguous'
                      ? 'Needs review'
                      : series.groupStatus === 'added'
                        ? 'Added'
                        : 'Mixed'}
                </span>
                <span class="group-count">
                  <MetaLine
                    items={[
                      `${series.books.length} ${series.books.length === 1 ? 'book' : 'books'}`,
                      `${series.totalFiles} ${series.totalFiles === 1 ? 'file' : 'files'}`
                    ]}
                  />
                </span>
                {#if series.reviewCount > 0 && series.reviewCount < series.totalFiles}
                  <span class="review-count">{series.reviewCount} review</span>
                {/if}
                {#if series.ambiguousCount > 0}
                  <span class="ambiguous-count">{series.ambiguousCount} ambiguous</span>
                {/if}
                {#if series.unmatchedCount > 0}
                  <span class="unmatched-count">{series.unmatchedCount} unmatched</span>
                {/if}
              </span>
            </span>
            <span class="group-actions">
              {#if onResolveSelected}
                <button
                  type="button"
                  class="btn-reassign"
                  title="Reassign all series files"
                  onclick={(e) => {
                    e.stopPropagation();
                    handleReassignSeries(series);
                  }}
                >
                  <span class="material-icons-outlined">drive_file_move</span>
                </button>
              {/if}
              <button
                type="button"
                class="btn-rematch"
                title="Rematch all files in series"
                onclick={(e) => {
                  e.stopPropagation();
                  handleRematchSeries(series);
                }}
              >
                <span class="material-icons-outlined">refresh</span>
              </button>
              <button
                type="button"
                class="btn-ignore-series"
                title="Ignore all files in series"
                onclick={(e) => {
                  e.stopPropagation();
                  handleIgnoreSeries(series);
                }}
              >
                <span class="material-icons-outlined">visibility_off</span>
              </button>
            </span>
          </summary>
          <div class="group-body">
            {#if getSeriesFileCount(series) > 0}
              <div class="selection-toolbar">
                <label class="select-all-label">
                  <input
                    type="checkbox"
                    checked={getSeriesSelectedCount(series) === getSeriesFileCount(series) &&
                      getSeriesFileCount(series) > 0}
                    indeterminate={getSeriesSelectedCount(series) > 0 &&
                      getSeriesSelectedCount(series) < getSeriesFileCount(series)}
                    onchange={() => handleSelectAllSeries(series)}
                  />
                  {getSeriesSelectedCount(series) > 0
                    ? `${getSeriesSelectedCount(series)} selected`
                    : 'Select all'}
                </label>
                {#if getSeriesSelectedCount(series) > 0 && onResolveSelected}
                  <button
                    type="button"
                    class="btn-toolbar"
                    onclick={() => {
                      const allIds: string[] = [];
                      for (const book of series.books) {
                        const sel = selectedFiles.get(book.group_key);
                        if (sel) allIds.push(...sel);
                      }
                      if (allIds.length > 0) {
                        onResolveSelected?.(allIds, rootId);
                        for (const book of series.books) clearSelection(book.group_key);
                      }
                    }}
                  >
                    <span class="material-icons-outlined">manage_search</span>
                    Resolve Selected
                  </button>
                {/if}
              </div>
            {/if}
            {#each series.books as group (group.group_key)}
              {@const hasAction = needsAction(group)}
              {@const isAdding = addingGroups.has(group.group_key)}
              {@const isRematching = rematchingGroups.has(group.group_key)}
              {@const isLoadingFiles = loadingGroups.has(group.group_key)}
              <details
                class="subgroup book-subgroup"
                ontoggle={(e) => {
                  const det = e.currentTarget as HTMLDetailsElement;
                  handleGroupToggle(group.group_key, det.open);
                }}
              >
                <summary class="subgroup-header" class:has-review={hasAction}>
                  <span class="book-index"
                    >{#if group.series_index != null}#{group.series_index}{/if}</span
                  >
                  <span class="subgroup-label">{group.label}</span>
                  <span class="subgroup-meta">
                    <span class="status-badge status-{group.group_status}">
                      {group.group_status === 'review'
                        ? 'Ready'
                        : group.group_status === 'ambiguous'
                          ? 'Review'
                          : group.group_status === 'added'
                            ? 'Added'
                            : 'Mixed'}
                    </span>
                    <span class="subgroup-count">{group.file_count}</span>
                  </span>
                  <span class="subgroup-actions">
                    {#if group.review_count > 0}
                      <button
                        type="button"
                        class="btn-add-sm"
                        title="Add {group.review_count} files"
                        disabled={isAdding || isRematching}
                        onclick={(e) => {
                          e.stopPropagation();
                          handleAddGroupClick(group);
                        }}
                      >
                        {#if isAdding}
                          <span class="material-icons-outlined spin">sync</span>
                        {:else}
                          <span class="material-icons-outlined">library_add</span>
                        {/if}
                        {group.review_count}
                      </button>
                    {/if}
                    {#if (group.unmatched_count > 0 || group.ambiguous_count > 0) && onGroupResolve}
                      <button
                        type="button"
                        class="btn-resolve-sm"
                        title="Resolve"
                        disabled={isAdding || isRematching}
                        onclick={(e) => {
                          e.stopPropagation();
                          onGroupResolve?.(group.group_key, rootId);
                        }}
                      >
                        <span class="material-icons-outlined">manage_search</span>
                      </button>
                    {/if}
                    <button
                      type="button"
                      class="btn-rematch-sm"
                      title="Rematch"
                      disabled={isAdding || isRematching}
                      onclick={(e) => {
                        e.stopPropagation();
                        handleRematchGroup(group.group_key);
                      }}
                    >
                      {#if isRematching}
                        <span class="material-icons-outlined spin">sync</span>
                      {:else}
                        <span class="material-icons-outlined">refresh</span>
                      {/if}
                    </button>
                    {#if onReassign && group.group_key !== '__ungrouped__'}
                      <button
                        type="button"
                        class="btn-reassign-sm"
                        title="Reassign"
                        onclick={(e) => {
                          e.stopPropagation();
                          onReassign?.(group.group_key, rootId);
                        }}
                      >
                        <span class="material-icons-outlined">drive_file_move</span>
                      </button>
                    {/if}
                  </span>
                </summary>
                <div class="subgroup-body">
                  {#if isLoadingFiles}
                    <div class="loading-files">Loading...</div>
                  {:else}
                    {@const groupFiles = expandedFiles.get(group.group_key) ?? []}
                    {@const groupSelected = selectedFiles.get(group.group_key) ?? new Set()}
                    {#if groupFiles.length > 0}
                      <div class="selection-toolbar">
                        <label class="select-all-label">
                          <input
                            type="checkbox"
                            checked={groupSelected.size === groupFiles.length &&
                              groupFiles.length > 0}
                            indeterminate={groupSelected.size > 0 &&
                              groupSelected.size < groupFiles.length}
                            onchange={() => toggleSelectAll(group.group_key)}
                          />
                          {groupSelected.size > 0 ? `${groupSelected.size} selected` : 'Select all'}
                        </label>
                        {#if groupSelected.size > 0}
                          {#if onResolveSelected}
                            <button
                              type="button"
                              class="btn-toolbar"
                              onclick={() => {
                                onResolveSelected?.(getSelectedIds(group.group_key), rootId);
                                clearSelection(group.group_key);
                              }}
                            >
                              <span class="material-icons-outlined">manage_search</span>
                              Resolve
                            </button>
                          {/if}
                          <button
                            type="button"
                            class="btn-toolbar"
                            onclick={() => handleIgnoreSelected(group.group_key)}
                          >
                            <span class="material-icons-outlined">visibility_off</span>
                            Ignore
                          </button>
                          <button
                            type="button"
                            class="btn-toolbar"
                            onclick={() => handleRematchSelected(group.group_key)}
                          >
                            <span class="material-icons-outlined">refresh</span>
                            Rematch
                          </button>
                        {/if}
                      </div>
                    {/if}
                    {#each groupFiles as file (file.id)}
                      <div class="file-row-with-checkbox">
                        <input
                          type="checkbox"
                          class="file-checkbox"
                          checked={groupSelected.has(file.id)}
                          onchange={() => toggleFileSelection(group.group_key, file.id)}
                        />
                        <MediaRootFileRow
                          {file}
                          label={fileLabel(file)}
                          {onResolve}
                          {onInspect}
                          onStatusChange={(id, status) =>
                            handleStatusChangeAndRefresh(id, status, group.group_key)}
                        />
                      </div>
                    {/each}
                  {/if}
                </div>
              </details>
            {/each}
          </div>
        </details>
      {/each}
      <!-- Standalone books (no series or single-book series) -->
      {#each seriesNested.standalone as group (group.group_key)}
        {@render bookGroup(group)}
      {/each}
    {:else}
      <!-- Non-book media types: existing flat/TV rendering -->
      {#each allGroupsFiltered as group (group.group_key)}
        {@render bookGroup(group)}
      {/each}
    {/if}
  {/if}
</div>

{#if confirmingAdd}
  {@const group = allGroupsFiltered.find((g) => g.group_key === confirmingAdd)}
  {#if group}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="confirm-overlay" onclick={cancelAdd}>
      <div class="confirm-dialog" onclick={(e) => e.stopPropagation()}>
        <div class="confirm-header">
          {#if group.cover_url || group.poster_url}
            <img class="confirm-cover" src={group.cover_url || group.poster_url} alt="" />
          {/if}
          <div class="confirm-info">
            <div class="confirm-title">{group.label}</div>
            {#if group.series_name}
              <div class="confirm-series">
                {group.series_name}{#if group.series_index != null}
                  #{group.series_index}{/if}
              </div>
            {/if}
            {#if group.authors?.length}
              <div class="confirm-authors">by {group.authors.join(', ')}</div>
            {/if}
          </div>
        </div>
        <div class="confirm-summary">
          Add {group.review_count}
          {group.review_count === 1 ? 'file' : 'files'} as "{group.label}"{#if group.series_name}
            in series "{group.series_name}"{/if}?
        </div>
        <div class="confirm-actions">
          <button type="button" class="btn-cancel" onclick={cancelAdd}>Cancel</button>
          <button
            type="button"
            class="btn-confirm"
            onclick={() => confirmAddGroup(group.group_key)}
          >
            <span class="material-icons-outlined">library_add</span>
            Add {group.review_count}
            {group.review_count === 1 ? 'file' : 'files'}
          </button>
        </div>
      </div>
    </div>
  {/if}
{/if}

<style>
  .entity-view {
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: 13px;
  }

  .entity-toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 8px;
  }

  .entity-search {
    flex: 0 0 auto;
    width: 180px;
    padding: 5px 10px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--card);
    color: var(--ink);
    font-size: 12px;
    outline: none;
  }

  .entity-search::placeholder {
    color: var(--muted);
  }

  .entity-search:focus {
    border-color: var(--accent);
  }

  .entity-filters {
    display: flex;
    gap: 2px;
    flex: 1;
    background: var(--card);
    padding: 3px;
    border-radius: 8px;
    border: 1px solid var(--ring);
  }

  .filter-tab {
    padding: 5px 10px;
    border-radius: 6px;
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
    color: var(--muted);
    border: none;
    background: none;
    white-space: nowrap;
    transition: background 0.15s;
  }

  .filter-tab:hover {
    background: rgba(255, 255, 255, 0.05);
    color: var(--ink);
  }

  .filter-tab.active {
    background: var(--bg-2);
    color: var(--ink);
  }

  .entity-group {
    border: 1px solid var(--ring);
    border-radius: 8px;
    overflow: hidden;
  }

  .entity-group + .entity-group {
    margin-top: 6px;
  }

  .group-header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    cursor: pointer;
    user-select: none;
    background: var(--card);
    font-weight: 600;
    list-style: none;
  }

  .group-header::-webkit-details-marker {
    display: none;
  }

  .group-header:hover {
    background: color-mix(in srgb, var(--card) 80%, var(--ink) 4%);
  }

  .group-header.has-review {
    padding: 8px 12px;
  }

  .group-poster {
    width: 32px;
    height: 46px;
    border-radius: 4px;
    object-fit: cover;
    flex-shrink: 0;
  }

  .group-icon {
    font-size: 18px;
    color: var(--accent);
    flex-shrink: 0;
  }

  .group-info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .group-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .group-meta {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 11px;
    font-weight: 400;
  }

  .group-series {
    font-size: 11px;
    font-weight: 400;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .group-authors {
    font-size: 11px;
    font-weight: 400;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .group-count {
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .status-badge {
    padding: 1px 6px;
    border-radius: 4px;
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.02em;
  }

  .status-review {
    color: #22c55e;
    border: 1px solid rgba(34, 197, 94, 0.3);
  }

  .status-ambiguous {
    color: var(--warning);
    border: 1px solid rgba(249, 115, 22, 0.3);
  }

  .status-added {
    color: var(--muted);
    border: 1px solid var(--ring);
  }

  .status-mixed {
    color: var(--info, #3b82f6);
    border: 1px solid rgba(59, 130, 246, 0.3);
  }

  .review-count {
    color: #22c55e;
    font-weight: 600;
  }

  .ambiguous-count {
    color: var(--warning);
    font-weight: 600;
  }

  .unmatched-count {
    color: var(--warning);
    font-weight: 600;
  }

  .provider-badge {
    padding: 0 4px;
    border-radius: 3px;
    background: rgba(255, 255, 255, 0.06);
    color: var(--muted);
    font-size: 10px;
    text-transform: lowercase;
  }

  .confidence {
    font-variant-numeric: tabular-nums;
    color: var(--muted);
    font-size: 10px;
  }

  .confidence.low {
    color: var(--warning);
  }

  .group-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .btn-resolve {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 5px 12px;
    border-radius: 6px;
    border: 1px solid var(--warning);
    background: transparent;
    color: var(--warning);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
    transition:
      opacity 0.15s,
      transform 0.15s;
  }

  .btn-resolve :global(.material-icons-outlined) {
    font-size: 15px;
  }

  .btn-resolve:hover:not(:disabled) {
    background: rgba(249, 115, 22, 0.12);
  }

  .btn-resolve:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-add {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 5px 12px;
    border-radius: 6px;
    border: none;
    background: var(--accent);
    color: #1d130c;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
    transition:
      opacity 0.15s,
      transform 0.15s;
  }

  .btn-add :global(.material-icons-outlined) {
    font-size: 15px;
  }

  .btn-add:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.2);
  }

  .btn-add:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-rematch {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border-radius: 6px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
  }

  .btn-rematch :global(.material-icons-outlined) {
    font-size: 15px;
  }

  .btn-rematch:hover:not(:disabled) {
    background: var(--bg);
    color: var(--ink);
  }

  .btn-rematch:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-reassign {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border-radius: 6px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
  }

  .btn-reassign :global(.material-icons-outlined) {
    font-size: 15px;
  }

  .btn-reassign:hover {
    background: var(--bg);
    color: var(--ink);
    border-color: var(--accent);
  }

  .group-body {
    border-top: 1px solid var(--ring);
  }

  .subgroup {
    border-bottom: 1px solid color-mix(in srgb, var(--ring) 50%, transparent);
  }

  .subgroup-header {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px 6px 28px;
    cursor: pointer;
    user-select: none;
    font-size: 12px;
    font-weight: 600;
    color: var(--muted);
    list-style: none;
  }

  .subgroup-header::-webkit-details-marker {
    display: none;
  }

  .subgroup-header:hover {
    color: var(--ink);
  }

  .subgroup-label {
    flex: 1;
  }

  .subgroup-count {
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }

  .subgroup-actions {
    display: flex;
    align-items: center;
    gap: 3px;
  }

  .btn-add-sm {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 2px 8px;
    border-radius: 4px;
    border: none;
    background: var(--accent);
    color: #1d130c;
    font-size: 10px;
    font-weight: 700;
    cursor: pointer;
    white-space: nowrap;
    transition: opacity 0.15s;
  }

  .btn-add-sm :global(.material-icons-outlined) {
    font-size: 13px;
  }

  .btn-add-sm:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-rematch-sm {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: 4px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
  }

  .btn-rematch-sm :global(.material-icons-outlined) {
    font-size: 13px;
  }

  .btn-rematch-sm:hover:not(:disabled) {
    color: var(--ink);
  }

  .btn-rematch-sm:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .subgroup-body {
    padding-left: 16px;
  }

  .book-subgroup {
    border-bottom: 1px solid color-mix(in srgb, var(--ring) 50%, transparent);
  }

  .book-subgroup > .subgroup-header {
    padding: 8px 12px 8px 20px;
    gap: 8px;
  }

  .book-subgroup > .subgroup-header.has-review {
    padding: 6px 12px 6px 20px;
  }

  .book-index {
    font-size: 11px;
    font-weight: 700;
    color: var(--accent);
    min-width: 24px;
    flex-shrink: 0;
    font-variant-numeric: tabular-nums;
  }

  .subgroup-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: auto;
    flex-shrink: 0;
  }

  .btn-resolve-sm {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: 4px;
    border: 1px solid var(--warning);
    background: transparent;
    color: var(--warning);
    cursor: pointer;
    transition: all 0.15s;
  }

  .btn-resolve-sm :global(.material-icons-outlined) {
    font-size: 13px;
  }

  .btn-resolve-sm:hover:not(:disabled) {
    background: rgba(249, 115, 22, 0.12);
  }

  .btn-resolve-sm:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-reassign-sm {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: 4px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
  }

  .btn-reassign-sm :global(.material-icons-outlined) {
    font-size: 13px;
  }

  .btn-reassign-sm:hover {
    color: var(--ink);
    border-color: var(--accent);
  }

  .btn-ignore-series {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border-radius: 6px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
  }

  .btn-ignore-series :global(.material-icons-outlined) {
    font-size: 15px;
  }

  .btn-ignore-series:hover {
    border-color: #ef4444;
    color: #ef4444;
  }

  .selection-toolbar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    background: rgba(255, 255, 255, 0.03);
    border-bottom: 1px solid var(--ring);
    font-size: 12px;
  }

  .select-all-label {
    display: flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
    color: var(--muted);
    font-weight: 500;
    margin-right: auto;
  }

  .btn-toolbar {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 3px 10px;
    border-radius: 5px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
    transition: all 0.15s;
  }

  .btn-toolbar :global(.material-icons-outlined) {
    font-size: 14px;
  }

  .btn-toolbar:hover {
    background: var(--bg-2);
    border-color: var(--accent);
  }

  .file-row-with-checkbox {
    display: flex;
    align-items: center;
    gap: 0;
  }

  .file-checkbox {
    margin: 0 0 0 12px;
    flex-shrink: 0;
    cursor: pointer;
    accent-color: var(--accent);
  }

  .file-row-with-checkbox :global(.file-row) {
    flex: 1;
    min-width: 0;
  }

  .loading-files {
    padding: 12px 16px;
    color: var(--muted);
    font-size: 12px;
  }

  .empty {
    padding: 32px;
    text-align: center;
    color: var(--muted);
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  .spin {
    animation: spin 1s linear infinite;
  }

  .confirm-overlay {
    position: fixed;
    inset: 0;
    z-index: 1000;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .confirm-dialog {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    padding: 20px;
    max-width: 420px;
    width: 90%;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.4);
  }

  .confirm-header {
    display: flex;
    gap: 12px;
    margin-bottom: 16px;
  }

  .confirm-cover {
    width: 48px;
    height: 72px;
    border-radius: 6px;
    object-fit: cover;
    flex-shrink: 0;
  }

  .confirm-info {
    flex: 1;
    min-width: 0;
  }

  .confirm-title {
    font-weight: 700;
    font-size: 15px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .confirm-series {
    font-size: 12px;
    color: var(--muted);
    margin-top: 2px;
  }

  .confirm-authors {
    font-size: 12px;
    color: var(--muted);
    margin-top: 2px;
  }

  .confirm-summary {
    font-size: 13px;
    color: var(--ink);
    margin-bottom: 16px;
    line-height: 1.4;
  }

  .confirm-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }

  .btn-cancel {
    padding: 6px 14px;
    border-radius: 6px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
  }

  .btn-cancel:hover {
    background: var(--bg-2);
    color: var(--ink);
  }

  .btn-confirm {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 6px 14px;
    border-radius: 6px;
    border: none;
    background: var(--accent);
    color: #1d130c;
    font-size: 12px;
    font-weight: 700;
    cursor: pointer;
    transition: transform 0.15s;
  }

  .btn-confirm :global(.material-icons-outlined) {
    font-size: 15px;
  }

  .btn-confirm:hover {
    transform: translateY(-1px);
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.2);
  }
</style>
