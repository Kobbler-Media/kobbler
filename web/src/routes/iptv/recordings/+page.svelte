<script lang="ts">
  import { page } from '$app/state';
  import { updateUrlParams } from '#lib/utils/url-state.js';
  import { ExpandableSidebar } from '@koblibs/ui';
  import ErrorBanner from '#lib/components/shared/ErrorBanner.svelte';
  import VideoPlayer from '#lib/components/VideoPlayer/VideoPlayer.svelte';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { usePlayback } from '#lib/composables/usePlayback.svelte.js';
  import * as api from '#lib/api/client.js';
  import type { IptvRecording, IptvSeriesRule, IptvChannel } from '#lib/types.js';

  type SortValue = 'date' | 'status' | 'size';

  const VALID_STATUSES = ['scheduled', 'recording', 'completed', 'failed'] as const;

  let recordings = $state<IptvRecording[]>([]);
  let seriesRules = $state<IptvSeriesRule[]>([]);
  let loading = $state(true);
  let error = $state('');
  let deletingIds = $state(new Set<string>());

  // URL-persisted filters
  const statusFilter = $derived(
    VALID_STATUSES.includes(page.url.searchParams.get('status') as (typeof VALID_STATUSES)[number])
      ? page.url.searchParams.get('status')!
      : undefined
  );

  const sort = $derived(
    (['date', 'status', 'size'] as SortValue[]).includes(
      page.url.searchParams.get('sort') as SortValue
    )
      ? (page.url.searchParams.get('sort') as SortValue)
      : 'date'
  );

  // Sorted recordings
  const sortedRecordings = $derived.by(() => {
    const recs = [...recordings];
    if (sort === 'status') {
      const order: Record<string, number> = { recording: 0, scheduled: 1, completed: 2, failed: 3 };
      recs.sort((a, b) => (order[a.status] ?? 9) - (order[b.status] ?? 9));
    } else if (sort === 'size') {
      recs.sort((a, b) => (b.file_size_bytes ?? 0) - (a.file_size_bytes ?? 0));
    } else {
      recs.sort((a, b) => new Date(b.start_time).getTime() - new Date(a.start_time).getTime());
    }
    return recs;
  });

  // New recording form
  let showNewRecording = $state(false);
  let newChannelId = $state('');
  let newChannelName = $state('');
  let channelSearch = $state('');
  let channelResults = $state<IptvChannel[]>([]);
  let showChannelDropdown = $state(false);
  let channelSearchTimeout: ReturnType<typeof setTimeout> | undefined;
  let newTitle = $state('');
  let newStartTime = $state('');
  let newEndTime = $state('');
  let saving = $state(false);

  function handleChannelSearch(query: string) {
    channelSearch = query;
    newChannelId = '';
    newChannelName = '';
    clearTimeout(channelSearchTimeout);
    if (query.length < 2) {
      channelResults = [];
      showChannelDropdown = false;
      return;
    }
    channelSearchTimeout = setTimeout(async () => {
      try {
        const res = await api.getIptvChannels({ search: query, enabled: true, limit: 10 });
        channelResults = res.items;
        showChannelDropdown = channelResults.length > 0;
      } catch {
        channelResults = [];
        showChannelDropdown = false;
      }
    }, 250);
  }

  function selectChannel(ch: IptvChannel) {
    newChannelId = ch.id;
    newChannelName = ch.name;
    channelSearch = ch.name;
    showChannelDropdown = false;
    channelResults = [];
  }

  // New rule form
  let showNewRule = $state(false);
  let rulePattern = $state('');
  let ruleChannelId = $state('');
  let ruleNewOnly = $state(true);
  let ruleKeepCount = $state<number | undefined>(undefined);
  let ruleSaving = $state(false);

  // Editing rule
  let editingRuleId = $state<string | null>(null);
  let editRulePattern = $state('');
  let editRuleChannelId = $state('');
  let editRuleNewOnly = $state(true);
  let editRuleKeepCount = $state<number | undefined>(undefined);

  // Confirmation dialogs
  let confirmDeleteRecId = $state<string | null>(null);
  let confirmDeleteRuleId = $state<string | null>(null);

  // Sidebar
  let sidebarOpen = $state(false);
  let selectedRecording = $state<IptvRecording | null>(null);

  // Playback
  const playback = usePlayback({
    mediaType: 'recording',
    refreshWatchState: async () => {}
  });

  async function loadRecordings() {
    loading = true;
    error = '';
    try {
      const [recs, rules] = await Promise.all([
        api.getIptvRecordings(statusFilter),
        api.getIptvSeriesRules()
      ]);
      recordings = recs;
      seriesRules = rules;
    } catch (e) {
      console.error('Failed to load recordings:', e);
      error = 'Failed to load recordings. Please check your connection and try again.';
    } finally {
      loading = false;
    }
  }

  function handleStatusFilter(status: string | undefined) {
    updateUrlParams({ status: status ?? null });
  }

  function handleSortChange(newSort: SortValue) {
    updateUrlParams({ sort: newSort === 'date' ? null : newSort });
  }

  // Reload when URL-derived filter changes
  $effect(() => {
    statusFilter;
    loadRecordings();
  });

  async function handleConfirmDeleteRecording() {
    if (!confirmDeleteRecId) return;
    const id = confirmDeleteRecId;
    confirmDeleteRecId = null;
    deletingIds = new Set([...deletingIds, id]);
    try {
      await api.deleteIptvRecording(id);
      recordings = recordings.filter((r) => r.id !== id);
      if (selectedRecording?.id === id) {
        sidebarOpen = false;
        selectedRecording = null;
      }
      showMessage('Recording deleted');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete recording');
    } finally {
      const next = new Set(deletingIds);
      next.delete(id);
      deletingIds = next;
    }
  }

  async function handleCreateRecording() {
    if (!newChannelId || !newTitle || !newStartTime || !newEndTime) return;
    saving = true;
    try {
      await api.createIptvRecording({
        channel_id: newChannelId,
        title: newTitle,
        start_time: new Date(newStartTime).toISOString(),
        end_time: new Date(newEndTime).toISOString()
      });
      showMessage('Recording scheduled');
      showNewRecording = false;
      newChannelId = '';
      newChannelName = '';
      channelSearch = '';
      newTitle = '';
      newStartTime = '';
      newEndTime = '';
      loadRecordings();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to create recording');
    } finally {
      saving = false;
    }
  }

  async function handleCreateRule() {
    if (!rulePattern) return;
    ruleSaving = true;
    try {
      await api.createIptvSeriesRule({
        title_pattern: rulePattern,
        channel_id: ruleChannelId || undefined,
        record_new_only: ruleNewOnly,
        keep_count: ruleKeepCount
      });
      showMessage('Series rule created');
      showNewRule = false;
      rulePattern = '';
      ruleChannelId = '';
      ruleNewOnly = true;
      ruleKeepCount = undefined;
      loadRecordings();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to create rule');
    } finally {
      ruleSaving = false;
    }
  }

  async function handleConfirmDeleteRule() {
    if (!confirmDeleteRuleId) return;
    const id = confirmDeleteRuleId;
    confirmDeleteRuleId = null;
    try {
      await api.deleteIptvSeriesRule(id);
      seriesRules = seriesRules.filter((r) => r.id !== id);
      showMessage('Rule deleted');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete rule');
    }
  }

  function startEditRule(rule: IptvSeriesRule) {
    editingRuleId = rule.id;
    editRulePattern = rule.title_pattern;
    editRuleChannelId = rule.channel_id ?? '';
    editRuleNewOnly = rule.record_new_only;
    editRuleKeepCount = rule.keep_count ?? undefined;
  }

  function cancelEditRule() {
    editingRuleId = null;
  }

  // Count recordings per rule (by pattern matching)
  function ruleMatchCount(rule: IptvSeriesRule): number {
    return recordings.filter((r) =>
      r.title.toLowerCase().includes(rule.title_pattern.toLowerCase())
    ).length;
  }

  function handleSelectRecording(rec: IptvRecording) {
    selectedRecording = rec;
    sidebarOpen = true;
  }

  function handleCloseSidebar() {
    sidebarOpen = false;
    selectedRecording = null;
  }

  function statusBadgeClass(status: string): string {
    switch (status) {
      case 'scheduled':
        return 'badge-info';
      case 'recording':
        return 'badge-recording';
      case 'completed':
        return 'badge-success';
      case 'failed':
        return 'badge-error';
      default:
        return '';
    }
  }

  function formatDateTime(dateStr: string): string {
    return new Date(dateStr).toLocaleString(undefined, {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit'
    });
  }

  function formatSize(bytes: number | null): string {
    if (!bytes) return '';
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
    return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
  }
</script>

<svelte:head>
  <title>Recordings — Live TV — Kobbler</title>
</svelte:head>

{#if error}
  <div class="error-wrapper">
    <ErrorBanner message={error} onRetry={loadRecordings} onDismiss={() => (error = '')} />
  </div>
{/if}

<div class="recordings-page">
  <div class="page-header">
    <div class="header-left">
      <a href="/iptv" class="back-btn" aria-label="Back to Live TV">
        <span class="material-icons-outlined">arrow_back</span>
      </a>
      <h1>Recordings</h1>
    </div>

    <button class="add-btn" type="button" onclick={() => (showNewRecording = !showNewRecording)}>
      <span class="material-icons-outlined" style="font-size: 16px;">add</span>

      New Recording
    </button>
  </div>

  <!-- New recording form -->
  {#if showNewRecording}
    <div class="form-card">
      <h3>Schedule Recording</h3>
      <form
        onsubmit={(e) => {
          e.preventDefault();
          handleCreateRecording();
        }}
      >
        <div class="form-row">
          <div class="form-field channel-search-field">
            <label for="rec-channel">Channel</label>
            <input
              id="rec-channel"
              type="text"
              value={channelSearch}
              oninput={(e) => handleChannelSearch(e.currentTarget.value)}
              onfocus={() => {
                if (channelResults.length > 0) showChannelDropdown = true;
              }}
              onblur={() =>
                setTimeout(() => {
                  showChannelDropdown = false;
                }, 200)}
              placeholder="Search channels..."
              autocomplete="off"
              required
            />
            {#if newChannelName}
              <span class="selected-channel">{newChannelName}</span>
            {/if}
            {#if showChannelDropdown}
              <ul class="channel-dropdown">
                {#each channelResults as ch (ch.id)}
                  <li>
                    <button type="button" onmousedown={() => selectChannel(ch)}>
                      {#if ch.icon_url}
                        <img src={ch.icon_url} alt="" class="ch-icon" />
                      {/if}
                      <span class="ch-name">{ch.name}</span>
                      {#if ch.category}
                        <span class="ch-cat">{ch.category}</span>
                      {/if}
                    </button>
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
          <div class="form-field">
            <label for="rec-title">Title</label>
            <input
              id="rec-title"
              type="text"
              bind:value={newTitle}
              placeholder="Recording title"
              required
            />
          </div>
        </div>
        <div class="form-row">
          <div class="form-field">
            <label for="rec-start">Start Time</label>
            <input id="rec-start" type="datetime-local" bind:value={newStartTime} required />
          </div>
          <div class="form-field">
            <label for="rec-end">End Time</label>
            <input id="rec-end" type="datetime-local" bind:value={newEndTime} required />
          </div>
        </div>
        <div class="form-actions">
          <button type="button" class="btn-sec" onclick={() => (showNewRecording = false)}
            >Cancel</button
          >

          <button type="submit" class="btn-primary" disabled={saving}
            >{saving ? 'Scheduling...' : 'Schedule'}</button
          >
        </div>
      </form>
    </div>
  {/if}

  <!-- Filters row -->
  <div class="filters-row">
    <div class="filter-pills">
      {#each [undefined, ...VALID_STATUSES] as status (status ?? 'all')}
        <button
          class="filter-pill"
          class:active={statusFilter === status}
          type="button"
          onclick={() => handleStatusFilter(status)}
        >
          {status ?? 'All'}
        </button>
      {/each}
    </div>

    <div class="sort-controls">
      <span class="sort-label">Sort:</span>
      {#each [['date', 'Date'], ['status', 'Status'], ['size', 'Size']] as [val, label] (val)}
        <button
          class="sort-pill"
          class:active={sort === val}
          type="button"
          onclick={() => handleSortChange(val as SortValue)}
        >
          {label}
        </button>
      {/each}
    </div>
  </div>

  <!-- Recordings list -->
  {#if loading && recordings.length === 0}
    <div class="skeleton-list">
      {#each Array(5) as _, i (i)}
        <div class="skeleton-item">
          <div class="skeleton-line wide"></div>
          <div class="skeleton-line narrow"></div>
        </div>
      {/each}
    </div>
  {:else if sortedRecordings.length === 0}
    <div class="empty-state">
      <span class="material-icons-outlined" style="font-size: 40px; opacity: 0.2;"
        >fiber_manual_record</span
      >
      <p>No recordings</p>
    </div>
  {:else}
    <div class="recordings-list">
      {#each sortedRecordings as rec (rec.id)}
        <div
          class="recording-item"
          class:deleting={deletingIds.has(rec.id)}
          role="button"
          tabindex="0"
          onclick={() => handleSelectRecording(rec)}
          onkeydown={(e) => {
            if (e.key === 'Enter') handleSelectRecording(rec);
          }}
        >
          <div class="rec-info">
            <div class="rec-title-row">
              <span class="rec-title">{rec.title}</span>
              <span class="status-badge {statusBadgeClass(rec.status)}">{rec.status}</span>
            </div>
            <div class="rec-meta">
              <span>{formatDateTime(rec.start_time)} - {formatDateTime(rec.end_time)}</span>
              {#if rec.file_size_bytes}
                <span class="rec-size">{formatSize(rec.file_size_bytes)}</span>
              {/if}
            </div>
            {#if rec.error_message}
              <span class="rec-error">{rec.error_message}</span>
            {/if}
          </div>
          <button
            class="delete-btn"
            type="button"
            disabled={deletingIds.has(rec.id)}
            onclick={(e) => {
              e.stopPropagation();
              confirmDeleteRecId = rec.id;
            }}
            aria-label="Delete recording"
          >
            <span class="material-icons-outlined" style="font-size: 16px;">delete</span>
          </button>
        </div>
      {/each}
    </div>
  {/if}

  <!-- Series Rules section -->
  <div class="rules-section">
    <div class="rules-header">
      <h2>Series Rules</h2>

      <button class="add-btn small" type="button" onclick={() => (showNewRule = !showNewRule)}>
        <span class="material-icons-outlined" style="font-size: 14px;">add</span>

        Add Rule
      </button>
    </div>

    {#if showNewRule}
      <div class="form-card">
        <form
          onsubmit={(e) => {
            e.preventDefault();
            handleCreateRule();
          }}
        >
          <div class="form-field">
            <label for="rule-pattern">Title Pattern</label>
            <input
              id="rule-pattern"
              type="text"
              bind:value={rulePattern}
              placeholder="e.g. News*"
              required
            />
          </div>
          <div class="form-row">
            <div class="form-field">
              <label for="rule-channel">Channel ID (optional)</label>
              <input
                id="rule-channel"
                type="text"
                bind:value={ruleChannelId}
                placeholder="Any channel"
              />
            </div>
            <div class="form-field">
              <label for="rule-keep">Keep Count</label>
              <input
                id="rule-keep"
                type="number"
                bind:value={ruleKeepCount}
                placeholder="All"
                min="1"
              />
            </div>
          </div>
          <label class="checkbox-label">
            <input type="checkbox" bind:checked={ruleNewOnly} />
            Record new episodes only
          </label>
          <div class="form-actions">
            <button type="button" class="btn-sec" onclick={() => (showNewRule = false)}
              >Cancel</button
            >

            <button type="submit" class="btn-primary" disabled={ruleSaving}
              >{ruleSaving ? 'Creating...' : 'Create Rule'}</button
            >
          </div>
        </form>
      </div>
    {/if}

    {#if seriesRules.length === 0}
      <p class="empty-text">No series rules configured</p>
    {:else}
      <div class="rules-list">
        {#each seriesRules as rule (rule.id)}
          {#if editingRuleId === rule.id}
            <div class="rule-edit-card">
              <div class="form-field">
                <label for="edit-rule-pattern-{rule.id}">Title Pattern</label>
                <input id="edit-rule-pattern-{rule.id}" type="text" bind:value={editRulePattern} />
              </div>
              <div class="form-row">
                <div class="form-field">
                  <label for="edit-rule-ch-{rule.id}">Channel ID</label>
                  <input
                    id="edit-rule-ch-{rule.id}"
                    type="text"
                    bind:value={editRuleChannelId}
                    placeholder="Any"
                  />
                </div>
                <div class="form-field">
                  <label for="edit-rule-keep-{rule.id}">Keep Count</label>
                  <input
                    id="edit-rule-keep-{rule.id}"
                    type="number"
                    bind:value={editRuleKeepCount}
                    placeholder="All"
                    min="1"
                  />
                </div>
              </div>
              <label class="checkbox-label">
                <input type="checkbox" bind:checked={editRuleNewOnly} />
                Record new episodes only
              </label>
              <div class="form-actions">
                <button type="button" class="btn-sec" onclick={cancelEditRule}>Cancel</button>
                <button type="button" class="btn-primary" onclick={cancelEditRule}>Save</button>
              </div>
            </div>
          {:else}
            <div class="rule-item">
              <div class="rule-info">
                <div class="rule-title-row">
                  <span class="rule-pattern">{rule.title_pattern}</span>
                  {#if ruleMatchCount(rule) > 0}
                    <span class="match-count">{ruleMatchCount(rule)} recordings</span>
                  {/if}
                </div>
                <div class="rule-meta">
                  {#if rule.keep_count}
                    <span>Keep {rule.keep_count}</span>
                  {/if}
                  {#if rule.record_new_only}
                    <span>New only</span>
                  {/if}
                  <span class="rule-status" class:disabled={!rule.enabled}>
                    {rule.enabled ? 'Enabled' : 'Disabled'}
                  </span>
                </div>
              </div>
              <div class="rule-actions">
                <button
                  class="icon-btn-sm"
                  type="button"
                  onclick={() => startEditRule(rule)}
                  aria-label="Edit rule"
                >
                  <span class="material-icons-outlined" style="font-size: 14px;">edit</span>
                </button>
                <button
                  class="icon-btn-sm danger"
                  type="button"
                  onclick={() => (confirmDeleteRuleId = rule.id)}
                  aria-label="Delete rule"
                >
                  <span class="material-icons-outlined" style="font-size: 14px;">delete</span>
                </button>
              </div>
            </div>
          {/if}
        {/each}
      </div>
    {/if}
  </div>
</div>

<!-- Recording detail sidebar -->
<ExpandableSidebar
  open={sidebarOpen}
  title={selectedRecording?.title ?? 'Recording Details'}
  onClose={handleCloseSidebar}
>
  {#if selectedRecording}
    <div class="sidebar-content">
      <div class="sidebar-field">
        <span class="sidebar-label">Status</span>
        <span class="status-badge {statusBadgeClass(selectedRecording.status)}"
          >{selectedRecording.status}</span
        >
      </div>

      <div class="sidebar-field">
        <span class="sidebar-label">Time</span>
        <span class="sidebar-value"
          >{formatDateTime(selectedRecording.start_time)} - {formatDateTime(
            selectedRecording.end_time
          )}</span
        >
      </div>

      <div class="sidebar-field">
        <span class="sidebar-label">Channel</span>
        <span class="sidebar-value">{selectedRecording.channel_id}</span>
      </div>

      {#if selectedRecording.file_path}
        <div class="sidebar-field">
          <span class="sidebar-label">File</span>
          <span class="sidebar-value file-path">{selectedRecording.file_path}</span>
        </div>
      {/if}

      {#if selectedRecording.file_size_bytes}
        <div class="sidebar-field">
          <span class="sidebar-label">Size</span>
          <span class="sidebar-value">{formatSize(selectedRecording.file_size_bytes)}</span>
        </div>
      {/if}

      {#if selectedRecording.error_message}
        <div class="sidebar-error">
          <span class="material-icons-outlined" style="font-size: 14px;">error</span>
          {selectedRecording.error_message}
        </div>
      {/if}

      <div class="sidebar-actions">
        {#if selectedRecording.status === 'completed'}
          <button
            type="button"
            class="btn-primary"
            disabled={playback.startingFileId === selectedRecording.id}
            onclick={() => playback.play(selectedRecording!.id)}
          >
            <span class="material-icons-outlined" style="font-size: 14px;">play_arrow</span>
            {playback.startingFileId === selectedRecording.id ? 'Starting...' : 'Play'}
          </button>
        {/if}
        <button
          type="button"
          class="btn-danger"
          onclick={() => (confirmDeleteRecId = selectedRecording!.id)}
        >
          <span class="material-icons-outlined" style="font-size: 14px;">delete</span>
          Delete
        </button>
      </div>
    </div>
  {/if}
</ExpandableSidebar>

<!-- Confirm delete recording dialog -->
{#if confirmDeleteRecId}
  <div class="modal-backdrop" role="presentation" onclick={() => (confirmDeleteRecId = null)}>
    <div class="modal-card" role="presentation" onclick={(e) => e.stopPropagation()}>
      <h3>Delete Recording</h3>
      <p>
        Are you sure you want to delete this recording? The recording file and thumbnails will be
        permanently removed from disk.
      </p>
      <div class="modal-actions">
        <button type="button" class="btn-sec" onclick={() => (confirmDeleteRecId = null)}
          >Cancel</button
        >

        <button type="button" class="btn-danger" onclick={handleConfirmDeleteRecording}
          >Delete</button
        >
      </div>
    </div>
  </div>
{/if}

<!-- Confirm delete rule dialog -->
{#if confirmDeleteRuleId}
  <div class="modal-backdrop" role="presentation" onclick={() => (confirmDeleteRuleId = null)}>
    <div class="modal-card" role="presentation" onclick={(e) => e.stopPropagation()}>
      <h3>Delete Series Rule</h3>
      <p>Are you sure you want to delete this series rule?</p>
      <div class="modal-actions">
        <button type="button" class="btn-sec" onclick={() => (confirmDeleteRuleId = null)}
          >Cancel</button
        >

        <button type="button" class="btn-danger" onclick={handleConfirmDeleteRule}>Delete</button>
      </div>
    </div>
  </div>
{/if}

{#if playback.isActive}
  <VideoPlayer
    sessionResponse={playback.session!}
    creationParams={{
      media_type: 'recording',
      file_id: playback.playingFileId!,
      capabilities: playback.capabilities!
    }}
    mediaInfo={{ title: selectedRecording?.title ?? 'Recording' }}
    onClose={playback.close}
  />
{/if}

<style>
  .error-wrapper {
    padding: 12px 24px 0;
  }

  .recordings-page {
    max-width: 900px;
    margin: 0 auto;
    padding: 24px;
  }

  .page-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 20px;
    gap: 12px;
  }

  .header-left {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .back-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    text-decoration: none;
    transition: all 0.15s;
  }

  .back-btn:hover {
    border-color: var(--accent);
    color: var(--accent);
  }

  .back-btn :global(.material-icons-outlined) {
    font-size: 18px;
  }

  .page-header h1 {
    font-size: 24px;
    font-weight: 700;
    margin: 0;
  }

  .add-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    border-radius: 8px;
    border: 1px solid var(--accent);
    background: var(--accent);
    color: var(--bg);
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: opacity 0.15s;
  }

  .add-btn:hover {
    opacity: 0.9;
  }

  .add-btn.small {
    padding: 5px 12px;
    font-size: 12px;
  }

  /* Filters */
  .filters-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 16px;
    flex-wrap: wrap;
  }

  .filter-pills {
    display: flex;
    gap: 4px;
    overflow-x: auto;
  }

  .filter-pill {
    padding: 5px 14px;
    border-radius: 16px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    text-transform: capitalize;
    white-space: nowrap;
    transition: all 0.15s;
  }

  .filter-pill:hover {
    border-color: var(--accent);
  }

  .filter-pill.active {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--bg);
  }

  .sort-controls {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .sort-label {
    font-size: 12px;
    color: var(--muted);
    margin-right: 4px;
  }

  .sort-pill {
    padding: 4px 10px;
    border-radius: 6px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
  }

  .sort-pill:hover {
    color: var(--ink);
  }

  .sort-pill.active {
    background: var(--bg-2);
    color: var(--ink);
    border-color: var(--ink);
  }

  /* Skeleton */
  .skeleton-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .skeleton-item {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 14px 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 10px;
  }

  .skeleton-line {
    height: 12px;
    border-radius: 4px;
    background: var(--ring);
    animation: skeleton-pulse 1.5s ease-in-out infinite;
  }

  .skeleton-line.wide {
    width: 60%;
  }
  .skeleton-line.narrow {
    width: 35%;
  }

  @keyframes skeleton-pulse {
    0%,
    100% {
      opacity: 0.4;
    }
    50% {
      opacity: 0.7;
    }
  }

  /* Recordings list */
  .recordings-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .recording-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 10px;
    transition: all 0.15s;
    cursor: pointer;
    width: 100%;
    text-align: left;
    font: inherit;
    color: inherit;
  }

  .recording-item:hover {
    border-color: var(--accent);
  }

  .recording-item.deleting {
    opacity: 0.5;
    pointer-events: none;
  }

  .rec-info {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
    flex: 1;
  }

  .rec-title-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .rec-title {
    font-size: 14px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .status-badge {
    font-size: 10px;
    padding: 2px 8px;
    border-radius: 10px;
    font-weight: 600;
    text-transform: uppercase;
    flex-shrink: 0;
  }

  .badge-info {
    background: rgba(59, 130, 246, 0.15);
    color: #3b82f6;
  }

  .badge-recording {
    background: rgba(239, 68, 68, 0.15);
    color: #ef4444;
    animation: pulse-recording 2s ease-in-out infinite;
  }

  .badge-success {
    background: rgba(34, 197, 94, 0.15);
    color: #22c55e;
  }

  .badge-error {
    background: rgba(239, 68, 68, 0.15);
    color: #ef4444;
  }

  @keyframes pulse-recording {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.6;
    }
  }

  .rec-meta {
    font-size: 12px;
    color: var(--muted);
    display: flex;
    gap: 12px;
  }

  .rec-size {
    color: var(--accent);
  }

  .rec-error {
    font-size: 12px;
    color: var(--danger, #ef4444);
  }

  .delete-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    flex-shrink: 0;
    transition: all 0.15s;
  }

  .delete-btn:hover:not(:disabled) {
    color: var(--danger, #ef4444);
    background: rgba(239, 68, 68, 0.1);
  }

  .delete-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  /* Rules */
  .rules-section {
    margin-top: 40px;
  }

  .rules-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 12px;
  }

  .rules-header h2 {
    font-size: 18px;
    font-weight: 600;
    margin: 0;
  }

  .rules-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .rule-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 14px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 8px;
  }

  .rule-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .rule-title-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .rule-pattern {
    font-size: 14px;
    font-weight: 600;
    font-family: 'SF Mono', 'Fira Code', monospace;
  }

  .match-count {
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 10px;
    background: var(--accent-dim);
    color: var(--accent);
    font-weight: 600;
  }

  .rule-meta {
    display: flex;
    gap: 8px;
    font-size: 12px;
    color: var(--muted);
  }

  .rule-status.disabled {
    color: var(--danger, #ef4444);
  }

  .rule-actions {
    display: flex;
    gap: 4px;
  }

  .icon-btn-sm {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
  }

  .icon-btn-sm:hover {
    background: var(--bg-2);
    color: var(--ink);
  }

  .icon-btn-sm.danger:hover {
    color: var(--danger, #ef4444);
    background: rgba(239, 68, 68, 0.1);
  }

  .rule-edit-card {
    padding: 14px;
    background: var(--card);
    border: 1px solid var(--accent);
    border-radius: 8px;
  }

  /* Forms */
  .form-card {
    padding: 16px 20px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    margin-bottom: 16px;
  }

  .form-card h3 {
    font-size: 15px;
    font-weight: 600;
    margin: 0 0 12px;
  }

  .form-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    flex: 1;
    margin-bottom: 12px;
  }

  .form-field label {
    font-size: 12px;
    font-weight: 600;
    color: var(--muted);
  }

  .form-field input {
    padding: 8px 12px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--bg);
    color: var(--ink);
    font-size: 14px;
    outline: none;
  }

  .form-field input:focus {
    border-color: var(--accent);
  }

  .form-row {
    display: flex;
    gap: 12px;
  }

  .channel-search-field {
    position: relative;
  }

  .selected-channel {
    display: block;
    font-size: 11px;
    color: var(--success, #22c55e);
    margin-top: 2px;
  }

  .channel-dropdown {
    position: absolute;
    top: 100%;
    left: 0;
    right: 0;
    background: var(--surface-2, #1e1e2e);
    border: 1px solid var(--border, #333);
    border-radius: 8px;
    margin-top: 4px;
    padding: 4px;
    list-style: none;
    z-index: 100;
    max-height: 240px;
    overflow-y: auto;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
  }

  .channel-dropdown li button {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 10px;
    border: none;
    background: none;
    color: var(--text-1, #e0e0e0);
    font-size: 13px;
    border-radius: 6px;
    cursor: pointer;
    text-align: left;
  }

  .channel-dropdown li button:hover {
    background: var(--surface-3, #2a2a3e);
  }

  .ch-icon {
    width: 20px;
    height: 20px;
    border-radius: 4px;
    object-fit: contain;
    flex-shrink: 0;
  }

  .ch-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .ch-cat {
    font-size: 11px;
    color: var(--text-3, #888);
    flex-shrink: 0;
  }

  .checkbox-label {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    cursor: pointer;
    margin-bottom: 12px;
  }

  .form-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .btn-primary {
    padding: 8px 20px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    border: 1px solid var(--accent);
    background: var(--accent);
    color: var(--bg);
    transition: all 0.15s;
  }

  .btn-primary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-sec {
    padding: 8px 20px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    transition: all 0.15s;
  }

  .btn-danger {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    border: 1px solid var(--danger, #ef4444);
    background: transparent;
    color: var(--danger, #ef4444);
    transition: all 0.15s;
  }

  .btn-danger:hover {
    background: rgba(239, 68, 68, 0.1);
  }

  /* Sidebar */
  .sidebar-content {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 4px 0;
  }

  .sidebar-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .sidebar-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .sidebar-value {
    font-size: 14px;
    color: var(--ink);
  }

  .file-path {
    font-family: 'SF Mono', 'Fira Code', monospace;
    font-size: 12px;
    word-break: break-all;
  }

  .sidebar-error {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    padding: 8px 12px;
    background: rgba(239, 68, 68, 0.08);
    border-radius: 6px;
    font-size: 13px;
    color: var(--danger, #ef4444);
  }

  .sidebar-actions {
    margin-top: 8px;
    padding-top: 12px;
    border-top: 1px solid var(--ring);
  }

  /* Modal */
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    z-index: 100;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px;
  }

  .modal-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 24px;
    max-width: 400px;
    width: 100%;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.3);
  }

  .modal-card h3 {
    font-size: 16px;
    font-weight: 600;
    margin: 0 0 8px;
  }

  .modal-card p {
    font-size: 14px;
    color: var(--muted);
    margin: 0 0 20px;
    line-height: 1.5;
  }

  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    padding: 48px;
    color: var(--muted);
  }

  .empty-state p {
    margin: 0;
  }

  .empty-text {
    color: var(--muted);
    font-size: 13px;
  }

  @media (max-width: 480px) {
    .form-row {
      flex-direction: column;
    }

    .filters-row {
      flex-direction: column;
      align-items: flex-start;
    }
  }
</style>
