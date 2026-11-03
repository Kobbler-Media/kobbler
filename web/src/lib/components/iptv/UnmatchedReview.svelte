<script lang="ts">
  import type { IptvUnmatchedSource } from '#lib/types.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import * as api from '#lib/api/client.js';

  interface Props {
    sources: IptvUnmatchedSource[];
    onrefresh: () => void;
  }

  let { sources, onrefresh }: Props = $props();

  let busyIds = $state(new Set<string>());

  async function handleAssign(source: IptvUnmatchedSource) {
    if (!source.suggested_channel_id) return;
    busyIds = new Set([...busyIds, source.id]);
    try {
      await api.assignIptvUnmatched(source.id, source.suggested_channel_id);
      showMessage('Source assigned to channel');
      onrefresh();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to assign');
    } finally {
      const next = new Set(busyIds);
      next.delete(source.id);
      busyIds = next;
    }
  }

  async function handleCreate(source: IptvUnmatchedSource) {
    busyIds = new Set([...busyIds, source.id]);
    try {
      await api.createChannelFromUnmatched(source.id);
      showMessage('Channel created');
      onrefresh();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to create channel');
    } finally {
      const next = new Set(busyIds);
      next.delete(source.id);
      busyIds = next;
    }
  }

  async function handleDismiss(source: IptvUnmatchedSource) {
    busyIds = new Set([...busyIds, source.id]);
    try {
      await api.dismissIptvUnmatched(source.id);
      showMessage('Source dismissed');
      onrefresh();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to dismiss');
    } finally {
      const next = new Set(busyIds);
      next.delete(source.id);
      busyIds = next;
    }
  }

  function confidenceLabel(score: number | null): string {
    if (score === null) return '';
    if (score >= 0.9) return 'High';
    if (score >= 0.6) return 'Medium';
    return 'Low';
  }
</script>

{#if sources.length === 0}
  <div class="empty">No unmatched sources</div>
{:else}
  <div class="unmatched-list">
    {#each sources as source (source.id)}
      <div class="unmatched-item" class:busy={busyIds.has(source.id)}>
        <div class="source-info">
          <span class="source-name">{source.name ?? 'Unnamed source'}</span>
          {#if source.language}
            <span class="source-lang">{source.language}</span>
          {/if}
          {#if source.suggested_channel_id}
            <span
              class="suggested-badge"
              title="Suggested match confidence: {confidenceLabel(source.suggested_confidence)}"
            >
              Suggested match ({confidenceLabel(source.suggested_confidence)})
            </span>
          {/if}
        </div>
        <div class="source-actions">
          {#if source.suggested_channel_id}
            <button
              class="action-btn assign"
              type="button"
              disabled={busyIds.has(source.id)}
              onclick={() => handleAssign(source)}
            >
              Assign
            </button>
          {/if}
          <button
            class="action-btn create"
            type="button"
            disabled={busyIds.has(source.id)}
            onclick={() => handleCreate(source)}
          >
            Create
          </button>
          <button
            class="action-btn dismiss"
            type="button"
            disabled={busyIds.has(source.id)}
            onclick={() => handleDismiss(source)}
          >
            Dismiss
          </button>
        </div>
      </div>
    {/each}
  </div>
{/if}

<style>
  .unmatched-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .unmatched-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 14px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 8px;
    transition: opacity 0.15s;
  }

  .unmatched-item.busy {
    opacity: 0.5;
    pointer-events: none;
  }

  .source-info {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    flex: 1;
  }

  .source-name {
    font-size: 13px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .source-lang {
    font-size: 11px;
    padding: 1px 6px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.08);
    color: var(--muted);
    flex-shrink: 0;
  }

  :global([data-theme='light']) .source-lang {
    background: rgba(0, 0, 0, 0.06);
  }

  .suggested-badge {
    font-size: 11px;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--accent-dim);
    color: var(--accent);
    flex-shrink: 0;
  }

  .source-actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
  }

  .action-btn {
    padding: 4px 10px;
    border-radius: 6px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    transition: all 0.15s;
  }

  .action-btn:hover:not(:disabled) {
    border-color: var(--accent);
  }

  .action-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .action-btn.assign {
    background: var(--accent-dim);
    color: var(--accent);
    border-color: transparent;
  }

  .action-btn.dismiss {
    color: var(--muted);
  }

  .action-btn.dismiss:hover:not(:disabled) {
    color: var(--danger);
    border-color: var(--danger);
  }

  .empty {
    padding: 24px;
    text-align: center;
    color: var(--muted);
    font-size: 13px;
  }
</style>
