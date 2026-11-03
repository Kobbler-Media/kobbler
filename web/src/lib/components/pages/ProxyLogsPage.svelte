<script lang="ts">
  import type { LoadState } from '#lib/types.js';
  import {
    clearProxyLogs,
    getProxyLogsExportUrl,
    type ProxyLogEntry,
    type ProxyLogsQuery
  } from '#lib/api/client.js';
  import { on, off, onReconnect, offReconnect } from '#lib/services/eventstream.svelte.js';
  import { InfiniteScroll } from '@koblibs/ui';
  import { createDelayedLoading } from '#lib/utils/delayed-loading.svelte.js';
  import ProxyLogRow from '#lib/components/proxy/ProxyLogRow.svelte';
  import ProxyLogDetail from '#lib/components/proxy/ProxyLogDetail.svelte';
  import ProxyLogDiffModal from '#lib/components/proxy/ProxyLogDiffModal.svelte';

  type LoadProxyLogsHandler = (query: ProxyLogsQuery, append: boolean) => Promise<void>;
  type CopyToClipboardHandler = (text: string, label: string) => Promise<void>;

  interface Props {
    proxyLogs: ProxyLogEntry[];
    total: number;
    offset: number;
    loading: boolean;
    loadState: LoadState;
    onLoadProxyLogs: LoadProxyLogsHandler;
    onCopyText: CopyToClipboardHandler;
  }

  let {
    proxyLogs,
    total,
    offset,
    loading: _loading,
    loadState,
    onLoadProxyLogs,
    onCopyText
  }: Props = $props();

  const initialLoadingState = createDelayedLoading(false);
  $effect(() => {
    initialLoadingState.loading = loadState.status === 'loading';
  });

  let isPageVisible = $state(true);
  $effect(() => {
    const handleVisibility = () => {
      isPageVisible = document.visibilityState === 'visible';
    };
    document.addEventListener('visibilitychange', handleVisibility);
    return () => document.removeEventListener('visibilitychange', handleVisibility);
  });

  // Local filter state
  let filterText = $state('');
  let alertFilterEnabled = $state(false);
  let expandedLogId = $state<string | null>(null);
  let fullscreenDiffLog = $state<ProxyLogEntry | null>(null);

  const METHODS = new Set(['GET', 'POST', 'PUT', 'DELETE']);
  const SEVERITIES: Record<string, string> = {
    info: 'info',
    warn: 'warning',
    warning: 'warning',
    error: 'error'
  };

  /** Parse filter text into method, severity, and path terms. */
  function parseFilter(text: string): { path: string; method?: string; severity?: string } {
    const pathTerms: string[] = [];
    let method: string | undefined;
    let severity: string | undefined;

    for (const term of text.split(/\s+/).filter(Boolean)) {
      const upper = term.toUpperCase();
      const lower = term.toLowerCase();
      if (!method && METHODS.has(upper)) {
        method = upper;
      } else if (!severity && lower in SEVERITIES) {
        severity = SEVERITIES[lower];
      } else {
        pathTerms.push(term);
      }
    }

    return { path: pathTerms.join(' '), method, severity };
  }

  const loadLogs = async (newOffset = 0) => {
    const parsed = parseFilter(filterText);
    await onLoadProxyLogs(
      {
        offset: newOffset,
        path: parsed.path || undefined,
        method: parsed.method,
        severity: parsed.severity,
        has_alert: alertFilterEnabled || undefined
      },
      newOffset > 0
    );
  };

  const loadMore = () => {
    loadLogs(offset + proxyLogs.length);
  };

  const handleClear = async () => {
    if (confirm('Clear all proxy logs?')) {
      await clearProxyLogs();
      await loadLogs();
    }
  };

  // Auto-refresh via SSE
  let proxyLogCallback: ((data: any) => void) | null = null;
  let reconnectHandler: (() => void) | null = null;

  $effect(() => {
    proxyLogCallback = on('proxy_log_created', () => {
      if (isPageVisible && offset === 0) loadLogs(0);
    });

    reconnectHandler = onReconnect(() => {
      if (isPageVisible && offset === 0) loadLogs(0);
    });

    return () => {
      if (proxyLogCallback) {
        off('proxy_log_created', proxyLogCallback);
        proxyLogCallback = null;
      }
      if (reconnectHandler) {
        offReconnect(reconnectHandler);
        reconnectHandler = null;
      }
    };
  });
</script>

<div class="proxy-logs-page page-container">
  <div class="section-header">
    <h2>Proxy Logs</h2>
    <div class="section-row">
      <span>{total} total</span>
    </div>
  </div>
  <div class="filter-row">
    <input
      type="text"
      placeholder="Filter: GET error sync !init"
      title="Space-separated terms. GET/POST/PUT/DELETE filter by method. info/warn/error filter by severity. Other terms filter path (prefix with ! to exclude)."
      bind:value={filterText}
      oninput={() => loadLogs()}
    />
    <button
      class="icon-btn"
      class:active={alertFilterEnabled}
      class:alert-active={alertFilterEnabled}
      type="button"
      onclick={() => {
        alertFilterEnabled = !alertFilterEnabled;
        loadLogs();
      }}
      title={alertFilterEnabled ? 'Showing alerts only — click to show all' : 'Show alerts only'}
    >
      <span class="material-icons-outlined">report</span>
    </button>
    <a class="icon-btn" href={getProxyLogsExportUrl(true)} title="Export as ZIP">
      <span class="material-icons-outlined">download</span>
    </a>
    <button class="icon-btn clear-btn" type="button" onclick={handleClear} title="Clear all logs">
      <span class="material-icons-outlined">delete_sweep</span>
    </button>
  </div>
  {#if initialLoadingState.showLoading && proxyLogs.length === 0}
    <div class="loading-card">Loading proxy logs...</div>
  {:else if loadState.status === 'error'}
    <div class="error">{loadState.error}</div>
  {:else if proxyLogs.length === 0}
    <div class="empty-state">
      <h3>No proxy logs yet</h3>
      <p class="muted">Proxy logs will appear here as the proxy handles requests.</p>
    </div>
  {:else}
    <div class="proxy-logs-list">
      {#each proxyLogs as log (log.id)}
        <ProxyLogRow
          {log}
          expanded={expandedLogId === log.id}
          onToggle={() => (expandedLogId = expandedLogId === log.id ? null : log.id)}
        />
        {#if expandedLogId === log.id}
          <ProxyLogDetail
            {log}
            {onCopyText}
            onOpenFullscreenDiff={(l) => (fullscreenDiffLog = l)}
          />
        {/if}
      {/each}
    </div>
    <InfiniteScroll
      hasMore={offset + proxyLogs.length < total}
      loading={_loading}
      onLoadMore={loadMore}
    />
  {/if}

  {#if fullscreenDiffLog}
    <ProxyLogDiffModal log={fullscreenDiffLog} onClose={() => (fullscreenDiffLog = null)} />
  {/if}

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

    .filter-row {
      display: flex;
      gap: 8px;
      margin-bottom: 16px;
      align-items: center;
    }

    .filter-row input {
      padding: 8px 12px;
      border-radius: 10px;
      border: 1px solid var(--ring);
      background: var(--card);
      color: var(--ink);
      font-size: 13px;
      flex: 1;
      min-width: 120px;
    }

    .proxy-logs-list {
      display: grid;
      gap: 8px;
    }

    .muted {
      color: var(--muted);
      font-size: 12px;
    }

    /* Icon button overrides for this page */
    .icon-btn.alert-active {
      background: #ef4444;
      color: white;
      border-color: #ef4444;
    }

    .icon-btn.alert-active:hover {
      opacity: 0.9;
    }

    .clear-btn:hover {
      color: #ef4444;
      border-color: #ef4444;
    }
  </style>
</div>
