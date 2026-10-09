<script lang="ts">
  import type { ProxyLogEntry } from '$lib/api/client';
  import { diffLines } from 'diff';

  interface DiffPart {
    value: string;
    added?: boolean;
    removed?: boolean;
  }

  interface Props {
    log: ProxyLogEntry;
    onClose: () => void;
  }

  let { log, onClose }: Props = $props();

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
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="modal-overlay" onkeydown={(e) => e.key === 'Escape' && onClose()} onclick={onClose}>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="modal-content" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
    <div class="modal-header">
      <h3>Response Diff: {log.request_path}</h3>
      <button class="modal-close" type="button" onclick={onClose}>&times;</button>
    </div>
    <div class="modal-body">
      <p class="muted modal-meta">
        {log.request_method}
        {log.request_path} | Upstream: {log.upstream_status} ({log.upstream_duration_ms}ms) | Final: {log.response_status}
        ({log.total_duration_ms}ms total)
      </p>

      <h4>Headers Diff</h4>
      <pre
        class="modal-diff-content">{#each computeDiff(log.upstream_headers ? JSON.stringify(sortObjectKeys(log.upstream_headers), null, 2) : '', log.response_headers ? JSON.stringify(sortObjectKeys(log.response_headers), null, 2) : '') as part, i (i)}{#each part.value
            .split('\n')
            .filter((line, i, arr) => i < arr.length - 1 || line) as line, j (`${i}-${j}`)}<span
              class={part.added ? 'diff-add' : part.removed ? 'diff-remove' : 'diff-same'}
              >{part.added ? '+' : part.removed ? '-' : ' '} {line}</span
            >{/each}{/each}</pre>

      <h4>Body Diff</h4>
      <pre
        class="modal-diff-content">{#each computeDiff(formatJson(log.upstream_body, true), formatJson(log.response_body, true)) as part, i (i)}{#each part.value
            .split('\n')
            .filter((line, i, arr) => i < arr.length - 1 || line) as line, j (`${i}-${j}`)}<span
              class={part.added ? 'diff-add' : part.removed ? 'diff-remove' : 'diff-same'}
              >{part.added ? '+' : part.removed ? '-' : ' '} {line}</span
            >{/each}{/each}</pre>
    </div>
  </div>
</div>

<style>
  .modal-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.8);
    z-index: 1000;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 20px;
  }

  .modal-content {
    background: var(--bg);
    border-radius: 16px;
    width: 100%;
    max-width: 95vw;
    max-height: 95vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    border: 1px solid var(--ring);
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--ring);
    background: var(--card);
    flex-shrink: 0;
  }

  .modal-header h3 {
    margin: 0;
    font-size: 16px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .modal-close {
    background: transparent;
    border: none;
    font-size: 28px;
    line-height: 1;
    color: var(--muted);
    cursor: pointer;
    padding: 0 8px;
  }

  .modal-close:hover {
    color: var(--ink);
  }

  .modal-body {
    padding: 20px;
    overflow: auto;
    flex: 1;
  }

  .modal-body h4 {
    margin: 20px 0 10px;
    font-size: 14px;
    color: var(--muted);
  }

  .modal-body h4:first-child {
    margin-top: 0;
  }

  .modal-meta {
    margin: 0 0 16px;
  }

  pre.modal-diff-content {
    font-family: monospace;
    font-size: 12px;
    background: var(--card);
    padding: 16px;
    border-radius: 8px;
    overflow-x: auto;
    white-space: pre;
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
</style>
