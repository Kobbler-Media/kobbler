<script lang="ts">
  import type { ChatTurnLogEntry } from '#lib/api/client.js';

  interface Props {
    log: ChatTurnLogEntry;
    expanded: boolean;
    onToggle: () => void;
  }

  let { log, expanded, onToggle }: Props = $props();

  const STAGE_LABEL: Record<string, string> = {
    extract: 'Extract',
    resolve: 'Resolve',
    action: 'Action'
  };

  const summary = $derived(
    log.content?.user_message?.trim() ||
      (log.content ? '(empty message)' : 'Content is private to this user')
  );

  const transportLabel = $derived(log.transport === 'google_chat' ? 'Chat' : log.transport);

  const nullSlots = $derived(
    Object.entries(log.extract_shape ?? {})
      .filter(([, type]) => type === null)
      .map(([slot]) => slot)
  );
</script>

<div class="turn-row" class:expanded class:failed={!log.succeeded}>
  <button class="turn-summary" type="button" onclick={onToggle}>
    <span class="transport" class:google={log.transport === 'google_chat'}>{transportLabel}</span>
    <span class="message" class:private={!log.content}>{summary}</span>
    {#if log.failed_stage}
      <span class="stage stage-{log.failed_stage}" title={STAGE_LABEL[log.failed_stage]}>
        {STAGE_LABEL[log.failed_stage]} failed
      </span>
    {/if}
    {#if !log.succeeded && !log.failed_stage}
      <span class="stage stage-turn">Failed</span>
    {/if}
    {#if nullSlots.length > 0}
      <span class="null-slots" title="Extraction returned nothing for: {nullSlots.join(', ')}">
        {nullSlots.join(' ')}
      </span>
    {/if}
    {#if log.tool_names.length > 0}
      <span class="tools" title={log.tool_names.join(', ')}>
        <span class="material-icons-outlined">build</span>{log.tool_call_count}
      </span>
    {/if}
    <span class="rounds muted" title="LLM rounds">{log.rounds}r</span>
    <span class="duration muted">{log.total_ms ?? 0}ms</span>
    <span class="time muted">{new Date(log.created_at).toLocaleString()}</span>
  </button>
</div>

<style>
  .turn-row {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    overflow: hidden;
  }

  .turn-row.expanded {
    border-color: var(--accent);
  }

  .turn-row.failed {
    border-color: var(--danger);
  }

  .turn-summary {
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

  .turn-summary:hover {
    background: var(--ring);
  }

  .transport {
    font-weight: 700;
    font-size: 11px;
    padding: 2px 6px;
    border-radius: 4px;
    background: #3b82f6;
    color: #fff;
    min-width: 54px;
    text-align: center;
    text-transform: capitalize;
  }

  .transport.google {
    background: #10b981;
  }

  .message {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .message.private {
    color: var(--muted);
    font-style: italic;
  }

  .stage {
    font-weight: 600;
    font-size: 11px;
    padding: 2px 6px;
    border-radius: 4px;
    white-space: nowrap;
  }

  .stage-extract {
    background: rgba(245, 158, 11, 0.2);
    color: #f59e0b;
  }

  .stage-resolve {
    background: rgba(249, 115, 22, 0.2);
    color: #f97316;
  }

  .stage-action,
  .stage-turn {
    background: rgba(239, 68, 68, 0.2);
    color: #ef4444;
  }

  .null-slots {
    font-family: monospace;
    font-size: 11px;
    padding: 2px 6px;
    border-radius: 4px;
    background: rgba(245, 158, 11, 0.15);
    color: #f59e0b;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 200px;
  }

  .tools {
    display: flex;
    align-items: center;
    gap: 2px;
    font-size: 11px;
    color: var(--muted);
  }

  .tools .material-icons-outlined {
    font-size: 14px;
  }

  .rounds {
    font-size: 11px;
    min-width: 28px;
    text-align: right;
  }

  .duration {
    font-size: 11px;
    min-width: 56px;
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
    .rounds,
    .duration,
    .time,
    .null-slots {
      display: none;
    }

    .turn-summary {
      gap: 8px;
      padding: 8px 10px;
    }
  }
</style>
