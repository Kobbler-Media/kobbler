<script lang="ts">
  import type { ProxyLogEntry } from '#lib/api/client.js';
  import UuidTooltip from './UuidTooltip.svelte';

  interface Props {
    log: ProxyLogEntry;
    expanded: boolean;
    onToggle: () => void;
  }

  let { log, expanded, onToggle }: Props = $props();

  const UUID_REGEX = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gi;

  type PathPart = { type: 'text' | 'uuid'; value: string };

  function getPathParts(path: string): PathPart[] {
    const parts: PathPart[] = [];
    let lastIndex = 0;
    const regex = new RegExp(UUID_REGEX);
    let match;

    while ((match = regex.exec(path)) !== null) {
      if (match.index > lastIndex) {
        parts.push({ type: 'text', value: path.substring(lastIndex, match.index) });
      }
      parts.push({ type: 'uuid', value: match[0] });
      lastIndex = regex.lastIndex;
    }

    if (lastIndex < path.length) {
      parts.push({ type: 'text', value: path.substring(lastIndex) });
    }

    return parts;
  }

  function formatBytes(bytes: number): string {
    if (bytes === 0) return '0 B';
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }

  const decisionIcon: Record<string, { icon: string; title: string }> = {
    intercept: { icon: 'shield', title: 'Intercepted' },
    passthrough: { icon: 'arrow_forward', title: 'Passed through' },
    modify: { icon: 'tune', title: 'Modified' }
  };

  const decision = $derived(
    decisionIcon[log.decision] ?? { icon: 'help_outline', title: log.decision }
  );
</script>

<div
  class="proxy-log-row"
  class:expanded
  class:severity-error={log.severity === 'error'}
  class:severity-warning={log.severity === 'warning'}
>
  <button class="proxy-log-summary" type="button" onclick={onToggle}>
    <span
      class="method"
      class:get={log.request_method === 'GET'}
      class:post={log.request_method === 'POST'}
      class:put={log.request_method === 'PUT'}
      class:delete={log.request_method === 'DELETE'}
    >
      {log.request_method}
    </span>
    <span class="path"
      >{#each getPathParts(log.request_path) as part, i (i)}{#if part.type === 'uuid'}<UuidTooltip
            uuid={part.value}
            entityRefs={log.entity_refs}
          />{:else}{part.value}{/if}{/each}</span
    >
    <span
      class="status"
      class:ok={(log.response_status ?? 0) >= 200 && (log.response_status ?? 0) < 300}
      class:err={(log.response_status ?? 0) >= 400}
    >
      {log.response_status ?? '—'}
    </span>
    <span
      class="decision"
      class:decision-intercept={log.decision === 'intercept'}
      class:decision-modify={log.decision === 'modify'}
      title={decision.title}
    >
      <span class="material-icons-outlined">{decision.icon}</span>
      {#if log.has_alert}
        <span class="alert-dot" title={log.alert_message ?? 'Alert'}>!</span>
      {/if}
    </span>
    <span class="duration">{log.total_duration_ms}ms</span>
    <span class="response-size muted">{formatBytes(log.response_body_size)}</span>
    <span class="time muted">{new Date(log.created_at).toLocaleString()}</span>
  </button>
</div>

<style>
  .proxy-log-row {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    overflow: hidden;
  }

  .proxy-log-row.expanded {
    border-color: var(--accent);
  }

  .proxy-log-row.severity-error {
    border-color: var(--danger);
  }

  .proxy-log-row.severity-warning {
    border-color: #f59e0b;
  }

  .proxy-log-summary {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 14px;
    width: 100%;
    background: transparent;
    border: none;
    color: var(--ink);
    cursor: pointer;
    text-align: left;
    font-size: 13px;
  }

  .proxy-log-summary:hover {
    background: var(--ring);
  }

  .method {
    font-weight: 700;
    font-size: 11px;
    padding: 2px 6px;
    border-radius: 4px;
    background: var(--muted);
    color: var(--card);
    min-width: 50px;
    text-align: center;
  }

  .method.get {
    background: #10b981;
  }
  .method.post {
    background: #3b82f6;
  }
  .method.put {
    background: #f59e0b;
  }
  .method.delete {
    background: #ef4444;
  }

  .path {
    flex: 1;
    font-family: monospace;
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .status {
    font-weight: 600;
    font-size: 12px;
    padding: 2px 6px;
    border-radius: 4px;
  }

  .status.ok {
    background: rgba(16, 185, 129, 0.2);
    color: #10b981;
  }
  .status.err {
    background: rgba(239, 68, 68, 0.2);
    color: #ef4444;
  }

  .decision {
    display: flex;
    align-items: center;
    gap: 2px;
    color: var(--muted);
  }

  .decision .material-icons-outlined {
    font-size: 16px;
  }

  .decision-intercept {
    color: var(--accent);
  }

  .decision-modify {
    color: #f59e0b;
  }

  .alert-dot {
    color: #ef4444;
    font-weight: 700;
    font-size: 12px;
  }

  .duration {
    font-size: 11px;
    color: var(--muted);
    min-width: 50px;
    text-align: right;
  }

  .response-size {
    font-size: 11px;
    min-width: 60px;
    text-align: right;
  }

  .time {
    font-size: 11px;
    min-width: 140px;
    text-align: right;
  }

  .muted {
    color: var(--muted);
  }

  @media (max-width: 768px) {
    .duration,
    .response-size,
    .time {
      display: none;
    }

    .proxy-log-summary {
      gap: 8px;
      padding: 8px 10px;
    }

    .method {
      min-width: 40px;
      font-size: 10px;
      padding: 2px 4px;
    }
  }
</style>
