<script lang="ts">
  import { SearchSelect, InfiniteScroll } from '@koblibs/ui';
  import type { ChatLogsQuery, ChatTurnLogEntry } from '#lib/api/client.js';
  import type { UserSummary } from '#lib/types.js';
  import ChatTurnLogRow from '#lib/components/chat-logs/ChatTurnLogRow.svelte';
  import ChatTurnLogDetail from '#lib/components/chat-logs/ChatTurnLogDetail.svelte';

  interface Props {
    logs: ChatTurnLogEntry[];
    total: number;
    loading: boolean;
    error: string | null;
    users: UserSummary[];
    filters: ChatLogsQuery;
    onChangeFilters: (next: Partial<ChatLogsQuery>) => void;
    onLoadMore: () => void;
    onCopyText: (text: string, label: string) => Promise<void>;
  }

  let {
    logs,
    total,
    loading,
    error,
    users,
    filters,
    onChangeFilters,
    onLoadMore,
    onCopyText
  }: Props = $props();

  let expandedId = $state<string | null>(null);

  const transportOptions = [
    { value: '', label: 'All transports' },
    { value: 'web', label: 'Web' },
    { value: 'google_chat', label: 'Google Chat' }
  ];

  const statusOptions = [
    { value: '', label: 'All turns' },
    { value: 'ok', label: 'Succeeded' },
    { value: 'failed', label: 'Failed' }
  ];

  const stageOptions = [
    { value: '', label: 'Any stage' },
    { value: 'extract', label: 'Extract failed' },
    { value: 'resolve', label: 'Resolve failed' },
    { value: 'action', label: 'Action failed' }
  ];

  const userOptions = $derived([
    { value: '', label: 'All users' },
    ...users.map((u) => ({ value: u.id, label: u.display_name || u.email }))
  ]);

  const failedCount = $derived(logs.filter((l) => !l.succeeded).length);
</script>

<div class="chat-logs-page page-container">
  <div class="section-header">
    <h2>Chat Turn Logs</h2>
    <div class="section-row">
      <span>{total} total</span>
      {#if failedCount > 0}
        <span class="failed-count">{failedCount} failed on this page</span>
      {/if}
    </div>
  </div>

  <div class="filter-row">
    <input
      type="search"
      placeholder="Search your own messages"
      title="Free-text search over the user message. Only matches turns you had yourself — other people's message text is private."
      value={filters.q ?? ''}
      oninput={(e) => onChangeFilters({ q: e.currentTarget.value || undefined })}
    />
    <SearchSelect
      items={transportOptions}
      value={filters.transport ?? ''}
      onchange={(v: string) => onChangeFilters({ transport: v || undefined })}
    />
    <SearchSelect
      items={statusOptions}
      value={filters.status ?? ''}
      onchange={(v: string) => onChangeFilters({ status: (v as 'ok' | 'failed') || undefined })}
    />
    <SearchSelect
      items={stageOptions}
      value={filters.stage ?? ''}
      onchange={(v: string) =>
        onChangeFilters({ stage: (v as 'extract' | 'resolve' | 'action') || undefined })}
    />
    <SearchSelect
      items={userOptions}
      value={filters.user_id ?? ''}
      onchange={(v: string) => onChangeFilters({ user_id: v || undefined })}
    />
    <label class="date-field">
      <span>From</span>
      <input
        type="date"
        value={filters.since ? filters.since.slice(0, 10) : ''}
        onchange={(e) =>
          onChangeFilters({
            since: e.currentTarget.value ? `${e.currentTarget.value}T00:00:00Z` : undefined
          })}
      />
    </label>
    <label class="date-field">
      <span>To</span>
      <input
        type="date"
        value={filters.until ? filters.until.slice(0, 10) : ''}
        onchange={(e) =>
          onChangeFilters({
            until: e.currentTarget.value ? `${e.currentTarget.value}T23:59:59Z` : undefined
          })}
      />
    </label>
  </div>

  {#if loading && logs.length === 0}
    <div class="loading-card">Loading turn logs…</div>
  {:else if error}
    <div class="error">{error}</div>
  {:else if logs.length === 0}
    <div class="empty-state">
      <h3>No turn logs</h3>
      <p class="muted">
        One row lands here per chat turn, whether it succeeded or failed. Nothing matches these
        filters yet.
      </p>
    </div>
  {:else}
    <div class="turn-list">
      {#each logs as log (log.id)}
        <ChatTurnLogRow
          {log}
          expanded={expandedId === log.id}
          onToggle={() => (expandedId = expandedId === log.id ? null : log.id)}
        />
        {#if expandedId === log.id}
          <ChatTurnLogDetail {log} {onCopyText} />
        {/if}
      {/each}
    </div>
    <InfiniteScroll hasMore={logs.length < total} {loading} {onLoadMore} />
  {/if}
</div>

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

  .section-row .failed-count {
    color: var(--danger);
  }

  .filter-row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-bottom: 16px;
    align-items: center;
  }

  .filter-row input[type='search'] {
    padding: 8px 12px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    font-size: 13px;
    flex: 1;
    min-width: 180px;
  }

  .date-field {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .date-field input {
    padding: 6px 8px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    font-size: 12px;
  }

  .loading-card,
  .error {
    padding: 24px;
    text-align: center;
    border-radius: 16px;
    border: 1px solid var(--ring);
    background: var(--card);
  }

  .error {
    color: var(--danger);
    border-color: var(--danger);
  }

  .empty-state {
    padding: 24px;
    border-radius: 16px;
    border: 1px dashed var(--ring);
    background: var(--card);
    text-align: center;
  }

  .empty-state h3 {
    margin: 0 0 8px;
    font-size: 16px;
  }

  .empty-state p {
    margin: 0;
  }

  .turn-list {
    display: grid;
    gap: 8px;
  }

  .muted {
    color: var(--muted);
    font-size: 12px;
  }
</style>
