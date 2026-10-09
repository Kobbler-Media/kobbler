<script lang="ts">
  import { getProxyLogBlobUrl, type ProxyLogEntry } from '$lib/api/client';
  import { diffLines } from 'diff';
  import ProxyLogNotes from './ProxyLogNotes.svelte';
  import hljs from 'highlight.js/lib/core';
  import json from 'highlight.js/lib/languages/json';

  hljs.registerLanguage('json', json);

  interface DiffPart {
    value: string;
    added?: boolean;
    removed?: boolean;
  }

  type CopyToClipboardHandler = (text: string, label: string) => Promise<void>;

  interface Props {
    log: ProxyLogEntry;
    onCopyText: CopyToClipboardHandler;
    onOpenFullscreenDiff: (log: ProxyLogEntry) => void;
  }

  let { log, onCopyText, onOpenFullscreenDiff }: Props = $props();

  let diffViewEnabled = $state(false);

  const highlightJson = (str: string): string => {
    if (!str) return '';
    try {
      return hljs.highlight(str, { language: 'json' }).value;
    } catch {
      return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    }
  };

  const sortObjectKeys = (obj: unknown): unknown => {
    if (obj === null || typeof obj !== 'object') return obj;
    if (Array.isArray(obj)) return obj.map(sortObjectKeys);
    const sorted: Record<string, unknown> = {};
    for (const key of Object.keys(obj as Record<string, unknown>).sort()) {
      sorted[key] = sortObjectKeys((obj as Record<string, unknown>)[key]);
    }
    return sorted;
  };

  const formatJson = (str: string | null, sortKeys = false): string => {
    if (!str) return '';
    try {
      let obj = typeof str === 'string' ? JSON.parse(str) : str;
      if (sortKeys) obj = sortObjectKeys(obj);
      return JSON.stringify(obj, null, 2);
    } catch {
      return str;
    }
  };

  const computeDiff = (oldText: string | null, newText: string | null): DiffPart[] => {
    const a = oldText ?? '';
    const b = newText ?? '';
    if (a === b) return [{ value: a }];
    try {
      return diffLines(a, b);
    } catch {
      return [
        { value: a, removed: true },
        { value: b, added: true }
      ];
    }
  };

  const getFullProxyUrl = (log: ProxyLogEntry): string => {
    const host = log.request_headers?.['Host'] || log.request_headers?.['host'] || 'unknown';
    const proto = log.request_headers?.['X-Forwarded-Proto'] || 'http';
    const query = log.request_query ? `?${log.request_query}` : '';
    return `${proto}://${host}${log.request_path}${query}`;
  };

  const formatQueryParams = (query: string | null): { key: string; value: string }[] => {
    if (!query) return [];
    return query.split('&').map((param) => {
      const [key, ...rest] = param.split('=');
      return { key: decodeURIComponent(key), value: decodeURIComponent(rest.join('=')) };
    });
  };

  const copyLogToClipboard = async () => {
    const data = {
      id: log.id,
      request: {
        method: log.request_method,
        path: log.request_path,
        query: log.request_query,
        headers: log.request_headers,
        body: log.request_body
      },
      response: {
        status: log.response_status,
        headers: log.response_headers,
        body: log.response_body
      },
      upstream: log.upstream_url
        ? {
            url: log.upstream_url,
            status: log.upstream_status,
            headers: log.upstream_headers,
            body: log.upstream_body,
            duration_ms: log.upstream_duration_ms
          }
        : null,
      decision: log.decision,
      total_duration_ms: log.total_duration_ms,
      notes: log.notes,
      created_at: log.created_at
    };
    await onCopyText(JSON.stringify(data, null, 2), 'Log');
  };

  const generateUpstreamCurl = (): string | null => {
    if (!log.upstream_url) return null;
    let upstreamHost = '';
    try {
      const url = new URL(log.upstream_url);
      upstreamHost = url.host;
    } catch {
      const match = log.upstream_url.match(/^https?:\/\/([^/]+)/);
      if (match) upstreamHost = match[1];
    }
    const headers = log.request_headers || {};
    const lines: string[] = ['curl -v'];
    for (const [key, value] of Object.entries(headers)) {
      let headerValue = String(value);
      if (key.toLowerCase() === 'host' && upstreamHost) headerValue = upstreamHost;
      const escapedValue = headerValue.replace(/'/g, "'\\''");
      lines.push(`  -H '${key}: ${escapedValue}'`);
    }
    if (log.request_body) {
      const escapedBody = log.request_body.replace(/'/g, "'\\''");
      lines.push(`  -d '${escapedBody}'`);
    }
    lines.push(`  '${log.upstream_url}'`);
    return lines.join(' \\\n');
  };

  const copyUpstreamCurl = async () => {
    const curl = generateUpstreamCurl();
    if (curl) await onCopyText(curl, 'curl command');
  };
</script>

<div class="proxy-log-details">
  <!-- Summary metadata (always visible, covers what the row hides on mobile) -->
  <div class="detail-meta">
    <span class="meta-item">
      <span class="meta-label">Time</span>
      {new Date(log.created_at).toLocaleString()}
    </span>
    <span class="meta-item">
      <span class="meta-label">Status</span>
      {log.response_status}
    </span>
    <span class="meta-item">
      <span class="meta-label">Duration</span>
      {log.total_duration_ms}ms
    </span>
    <span class="meta-item">
      <span class="meta-label">Size</span>
      {log.response_body_size < 1024
        ? `${log.response_body_size} B`
        : log.response_body_size < 1024 * 1024
          ? `${(log.response_body_size / 1024).toFixed(1)} KB`
          : `${(log.response_body_size / (1024 * 1024)).toFixed(1)} MB`}
    </span>
    <span class="meta-item">
      <span class="meta-label">Decision</span>
      {log.decision}
    </span>
  </div>

  <!-- URLs Section -->
  <div class="detail-section urls-section">
    <div class="url-row">
      <span class="url-label">Proxy URL:</span>
      <code class="url-value">{getFullProxyUrl(log)}</code>
    </div>
    {#if log.upstream_url}
      <div class="url-row">
        <span class="url-label">Upstream URL:</span>
        <code class="url-value">{log.upstream_url}</code>
      </div>
    {/if}
    {#if log.request_query}
      <div class="url-row query-params">
        <span class="url-label">Query Params:</span>
        <div class="params-list">
          {#each formatQueryParams(log.request_query) as param, i (param.key + ':' + i)}
            <div class="param-row">
              <span class="param-key">{param.key}</span>
              <span class="param-eq">=</span>
              <span class="param-value">{param.value}</span>
            </div>
          {/each}
        </div>
      </div>
    {/if}
  </div>

  {#if log.entity_refs.length > 0 || log.notes || log.has_alert}
    <div class="detail-section notes-section">
      <h4>Notes</h4>
      <ProxyLogNotes
        notes={log.notes}
        entityRefs={log.entity_refs}
        alertMessage={log.alert_message}
      />
    </div>
  {/if}

  <div class="detail-actions">
    <button class="btn btn-small" type="button" onclick={copyLogToClipboard}> Copy JSON </button>
    {#if log.upstream_url}
      <button
        class="btn btn-small"
        type="button"
        onclick={copyUpstreamCurl}
        title="Copy curl command to replay request directly to upstream (bypassing proxy)"
      >
        Copy as curl
      </button>
    {/if}
    {#if log.has_request_blob}
      <a class="btn btn-small" href={getProxyLogBlobUrl(log.id, 'request')}>
        Download Request Blob
      </a>
    {/if}
    {#if log.has_response_blob}
      <a class="btn btn-small" href={getProxyLogBlobUrl(log.id, 'response')}>
        Download Response Blob
      </a>
    {/if}
  </div>

  <div class="detail-section">
    <h4>Request</h4>
    {#if log.request_headers}
      <!-- eslint-disable-next-line svelte/no-at-html-tags -- highlight.js output is safe -->
      <pre class="headers hljs">{@html highlightJson(
          JSON.stringify(log.request_headers, null, 2)
        )}</pre>
    {/if}
    {#if log.request_body}
      <!-- eslint-disable-next-line svelte/no-at-html-tags -- highlight.js output is safe -->
      <pre class="body hljs">{@html highlightJson(formatJson(log.request_body))}</pre>
    {:else}
      <p class="muted">No request body ({log.request_body_size} bytes)</p>
    {/if}
  </div>

  {#if log.decision === 'modify' && log.upstream_status !== null}
    <div class="detail-section upstream">
      <div class="section-header">
        <h4>Response Comparison</h4>
        <div class="section-row">
          <button
            class="btn btn-small"
            type="button"
            onclick={() => (diffViewEnabled = !diffViewEnabled)}
          >
            {diffViewEnabled ? 'Show Side-by-Side' : 'Show Diff'}
          </button>
          <button class="btn btn-small" type="button" onclick={() => onOpenFullscreenDiff(log)}>
            View Fullscreen
          </button>
        </div>
      </div>
      <p class="muted">
        Upstream: {log.upstream_status} from {log.upstream_url || 'unknown'} ({log.upstream_duration_ms ??
          0}ms) | Final: {log.response_status} ({log.total_duration_ms}ms total)
      </p>

      {#if diffViewEnabled}
        <!-- Diff View -->
        <div class="diff-view">
          <h5>Headers Diff</h5>
          <pre
            class="diff-content">{#each computeDiff(log.upstream_headers ? JSON.stringify(sortObjectKeys(log.upstream_headers), null, 2) : '', log.response_headers ? JSON.stringify(sortObjectKeys(log.response_headers), null, 2) : '') as part, i (i)}{#each part.value
                .split('\n')
                .filter((line, i, arr) => i < arr.length - 1 || line) as line, j (`${i}-${j}`)}<span
                  class={part.added ? 'diff-add' : part.removed ? 'diff-remove' : 'diff-same'}
                  >{part.added ? '+' : part.removed ? '-' : ' '} {line}</span
                >{/each}{/each}</pre>

          <h5>Body Diff</h5>
          <pre
            class="diff-content">{#each computeDiff(formatJson(log.upstream_body, true), formatJson(log.response_body, true)) as part, i (i)}{#each part.value
                .split('\n')
                .filter((line, i, arr) => i < arr.length - 1 || line) as line, j (`${i}-${j}`)}<span
                  class={part.added ? 'diff-add' : part.removed ? 'diff-remove' : 'diff-same'}
                  >{part.added ? '+' : part.removed ? '-' : ' '} {line}</span
                >{/each}{/each}</pre>
        </div>
      {:else}
        <!-- Side-by-Side View -->
        <div class="side-by-side">
          <div class="side upstream-side">
            <h5>Upstream (from Kobo)</h5>
            {#if log.upstream_headers}
              <!-- eslint-disable-next-line svelte/no-at-html-tags -- highlight.js output is safe -->
              <pre class="headers hljs">{@html highlightJson(
                  JSON.stringify(log.upstream_headers, null, 2)
                )}</pre>
            {/if}
            {#if log.upstream_body}
              <!-- eslint-disable-next-line svelte/no-at-html-tags -- highlight.js output is safe -->
              <pre class="body hljs">{@html highlightJson(formatJson(log.upstream_body))}</pre>
            {:else}
              <p class="muted">No body ({log.upstream_body_size} bytes)</p>
            {/if}
          </div>
          <div class="side final-side">
            <h5>Final (to device)</h5>
            {#if log.response_headers}
              <!-- eslint-disable-next-line svelte/no-at-html-tags -- highlight.js output is safe -->
              <pre class="headers hljs">{@html highlightJson(
                  JSON.stringify(log.response_headers, null, 2)
                )}</pre>
            {/if}
            {#if log.response_body}
              <!-- eslint-disable-next-line svelte/no-at-html-tags -- highlight.js output is safe -->
              <pre class="body hljs">{@html highlightJson(formatJson(log.response_body))}</pre>
            {:else}
              <p class="muted">No body ({log.response_body_size} bytes)</p>
            {/if}
          </div>
        </div>
      {/if}
    </div>
  {:else}
    <!-- Non-modify: just show response -->
    <div class="detail-section">
      <h4>Response</h4>
      <p class="muted">
        Status: {log.response_status}
        {#if log.upstream_url}| Upstream: {log.upstream_url} ({log.upstream_duration_ms}ms){/if}
      </p>
      {#if log.response_headers}
        <!-- eslint-disable-next-line svelte/no-at-html-tags -- highlight.js output is safe -->
        <pre class="headers hljs">{@html highlightJson(
            JSON.stringify(log.response_headers, null, 2)
          )}</pre>
      {/if}
      {#if log.response_body}
        <!-- eslint-disable-next-line svelte/no-at-html-tags -- highlight.js output is safe -->
        <pre class="body hljs">{@html highlightJson(formatJson(log.response_body))}</pre>
      {:else}
        <p class="muted">No response body ({log.response_body_size} bytes)</p>
      {/if}
    </div>
  {/if}
  {#if log.upstream_url && log.decision !== 'modify'}
    <div class="muted">Upstream: {log.upstream_url} ({log.upstream_duration_ms}ms)</div>
  {/if}
</div>

<style>
  .proxy-log-details {
    padding: 14px;
    border-top: 1px solid var(--ring);
    background: var(--bg);
  }

  .detail-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 12px 20px;
    padding: 10px 12px;
    margin-bottom: 12px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 10px;
    font-size: 12px;
  }

  .meta-item {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .meta-label {
    color: var(--muted);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }

  .urls-section {
    background: var(--card);
    padding: 12px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    margin-bottom: 12px;
  }

  .url-row {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    margin-bottom: 8px;
  }

  .url-row:last-child {
    margin-bottom: 0;
  }

  .url-label {
    font-size: 12px;
    color: var(--muted);
    min-width: 100px;
    flex-shrink: 0;
    padding-top: 2px;
  }

  .url-value {
    font-family: monospace;
    font-size: 12px;
    background: var(--bg);
    padding: 4px 8px;
    border-radius: 6px;
    word-break: break-all;
    flex: 1;
  }

  .query-params {
    align-items: flex-start;
  }

  .params-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    flex: 1;
  }

  .param-row {
    display: flex;
    align-items: center;
    gap: 4px;
    font-family: monospace;
    font-size: 12px;
    background: var(--bg);
    padding: 4px 8px;
    border-radius: 6px;
  }

  .param-key {
    color: var(--accent);
    font-weight: 600;
  }

  .param-eq {
    color: var(--muted);
  }

  .param-value {
    word-break: break-all;
  }

  .notes-section {
    background: var(--card);
    padding: 12px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    margin-bottom: 12px;
  }

  .detail-actions {
    display: flex;
    gap: 8px;
    margin-bottom: 12px;
  }

  .detail-section {
    margin-bottom: 16px;
  }

  .detail-section h4 {
    margin: 0 0 8px;
    font-size: 13px;
    color: var(--muted);
  }

  .detail-section h5 {
    margin: 12px 0 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .detail-section.upstream {
    background: var(--card);
    padding: 12px;
    border-radius: 10px;
    border: 1px solid var(--ring);
  }

  .section-header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    margin-bottom: 16px;
  }

  .section-header h4 {
    margin: 0;
    font-size: 14px;
  }

  .section-row {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  pre.headers,
  pre.body {
    font-family: monospace;
    font-size: 11px;
    background: var(--card);
    padding: 10px;
    border-radius: 8px;
    overflow-x: auto;
    overflow-y: auto;
    max-height: 400px;
    white-space: pre-wrap;
    word-break: break-all;
    margin: 0;
    border: 1px solid var(--ring);
  }

  /* highlight.js theme overrides */
  :global(pre.hljs) {
    color: var(--ink);
  }
  :global(pre.hljs .hljs-attr) {
    color: #d38c5a;
  }
  :global(pre.hljs .hljs-string) {
    color: #10b981;
  }
  :global(pre.hljs .hljs-number) {
    color: #60a5fa;
  }
  :global(pre.hljs .hljs-literal) {
    color: #f59e0b;
  }
  :global([data-theme='light'] pre.hljs .hljs-attr) {
    color: #9a5b30;
  }
  :global([data-theme='light'] pre.hljs .hljs-string) {
    color: #047857;
  }
  :global([data-theme='light'] pre.hljs .hljs-number) {
    color: #2563eb;
  }
  :global([data-theme='light'] pre.hljs .hljs-literal) {
    color: #d97706;
  }

  .side-by-side {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }

  .side h5 {
    margin: 0 0 8px;
    font-size: 12px;
  }

  .diff-view {
    margin-top: 12px;
  }

  pre.diff-content {
    font-family: monospace;
    font-size: 11px;
    background: var(--card);
    padding: 10px;
    border-radius: 8px;
    overflow-x: auto;
    white-space: pre-wrap;
    word-break: break-all;
    margin: 0;
    border: 1px solid var(--ring);
  }

  :global(.diff-add) {
    background: rgba(16, 185, 129, 0.2);
    display: block;
  }
  :global(.diff-remove) {
    background: rgba(239, 68, 68, 0.2);
    display: block;
  }
  :global(.diff-same) {
    display: block;
  }

  .muted {
    color: var(--muted);
    font-size: 12px;
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
    text-decoration: none;
  }

  .btn-small {
    padding: 6px 10px;
    font-size: 12px;
  }
</style>
