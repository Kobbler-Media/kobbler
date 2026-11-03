<script lang="ts">
  import type { ChatTurnLogEntry } from '#lib/api/client.js';

  interface Props {
    log: ChatTurnLogEntry;
    onCopyText: (text: string, label: string) => Promise<void>;
  }

  let { log, onCopyText }: Props = $props();

  const pretty = (value: unknown): string => {
    if (value === null || value === undefined) return '';
    try {
      return JSON.stringify(value, null, 2);
    } catch {
      return String(value);
    }
  };

  type StageView = {
    key: string;
    label: string;
    ms: number | null;
    error: string | null;
    detail: string | null;
  };

  const stages = $derived<StageView[]>([
    {
      key: 'extract',
      label: 'Extract',
      ms: log.extract_ms,
      error: log.extract_error,
      detail: log.content?.extract_output ? pretty(log.content.extract_output) : null
    },
    {
      key: 'resolve',
      label: 'Resolve',
      ms: log.resolve_ms,
      error: log.resolve_error,
      detail: log.content?.resolve_candidates ? pretty(log.content.resolve_candidates) : null
    },
    {
      key: 'action',
      label: 'Action',
      ms: log.action_ms,
      error: log.action_error,
      detail: log.content?.action_result ? pretty(log.content.action_result) : null
    }
  ]);

  const copyLog = () => onCopyText(pretty(log), 'Turn log');

  const slots = $derived(Object.entries(log.extract_shape ?? {}));
</script>

<div class="turn-detail">
  <div class="detail-actions">
    <button class="btn btn-small" type="button" onclick={copyLog}>Copy JSON</button>
    <span class="muted mono">{log.id}</span>
  </div>

  {#if !log.content}
    <div class="privacy-note">
      This conversation belongs to another user. Timings, errors, tool names and model details are
      shown; what was said is not.
    </div>
  {/if}

  <div class="detail-section">
    <h4>Turn</h4>
    <div class="kv-grid">
      <span class="k">Transport</span><span class="v">{log.transport}</span>
      <span class="k">Status</span>
      <span class="v" class:err={!log.succeeded}>{log.succeeded ? 'Succeeded' : 'Failed'}</span>
      <span class="k">User</span><span class="v mono">{log.user_id ?? '—'}</span>
      <span class="k">Session</span><span class="v mono">{log.session_id ?? '—'}</span>
      <span class="k">Total</span><span class="v">{log.total_ms ?? 0}ms</span>
    </div>
    {#if log.turn_error}
      <pre class="error-body">{log.turn_error}</pre>
    {/if}
  </div>

  {#if log.content}
    <div class="detail-section">
      <h4>Conversation</h4>
      <div class="bubble user">{log.content.user_message ?? '(no message)'}</div>
      {#if log.content.assistant_text}
        <div class="bubble assistant">{log.content.assistant_text}</div>
      {:else}
        <p class="muted">No assistant reply was produced.</p>
      {/if}
    </div>
  {/if}

  <div class="detail-section">
    <h4>Stages</h4>
    <div class="stages">
      {#each stages as stage (stage.key)}
        <div class="stage-card" class:failed={!!stage.error} class:idle={stage.ms === null}>
          <div class="stage-head">
            <span class="stage-name">{stage.label}</span>
            <span class="muted">{stage.ms === null ? 'not run' : `${stage.ms}ms`}</span>
          </div>
          {#if stage.key === 'resolve' && log.resolve_candidate_count !== null}
            <p class="muted">{log.resolve_candidate_count} candidates</p>
          {/if}
          {#if stage.key === 'resolve' && log.content?.resolve_query}
            <p class="muted mono">query: {log.content.resolve_query}</p>
          {/if}
          {#if stage.key === 'resolve' && log.content?.resolved_ref}
            <p class="muted mono">ref: {log.content.resolved_ref}</p>
          {/if}
          {#if stage.key === 'action' && log.action_name}
            <p class="muted mono">{log.action_name}</p>
          {/if}
          {#if stage.key === 'extract' && slots.length > 0}
            <ul class="slots">
              {#each slots as [slot, type] (slot)}
                <li class:empty={type === null}>
                  <span class="slot-name mono">{slot}</span>
                  <span class="slot-type mono">{type ?? 'null'}</span>
                </li>
              {/each}
            </ul>
          {/if}
          {#if stage.error}
            <pre class="error-body">{stage.error}</pre>
          {/if}
          {#if stage.detail}
            <pre class="json">{stage.detail}</pre>
          {/if}
        </div>
      {/each}
    </div>
  </div>

  <div class="detail-section">
    <h4>Model</h4>
    <div class="kv-grid">
      <span class="k">Provider</span><span class="v">{log.llm_provider ?? '—'}</span>
      <span class="k">Model</span><span class="v">{log.llm_model ?? '—'}</span>
      <span class="k">Rounds</span><span class="v">{log.rounds}</span>
      <span class="k">LLM time</span><span class="v">{log.llm_ms ?? 0}ms</span>
      <span class="k">Tokens</span>
      <span class="v">
        {log.prompt_tokens ?? 0} in / {log.completion_tokens ?? 0} out
      </span>
    </div>
  </div>

  <div class="detail-section">
    <h4>Tool calls ({log.tool_call_count})</h4>
    {#if log.tool_call_count === 0}
      <p class="muted">No tools were called.</p>
    {:else if log.content}
      {#each log.content.tool_calls as call, i (i)}
        <div class="tool-call" class:failed={!!call.error}>
          <div class="stage-head">
            <span class="stage-name mono">{call.name}</span>
            <span class="muted">{call.duration_ms}ms</span>
          </div>
          {#if call.arguments}
            <pre class="json">{pretty(call.arguments)}</pre>
          {/if}
          {#if call.error}
            <pre class="error-body">{call.error}</pre>
          {:else if call.result}
            <pre class="json">{pretty(call.result)}</pre>
          {/if}
        </div>
      {/each}
    {:else}
      <ul class="tool-names">
        {#each log.tool_names as name, i (i)}
          <li class="mono">{name}</li>
        {/each}
      </ul>
    {/if}
  </div>
</div>

<style>
  .turn-detail {
    padding: 12px 14px 16px;
    border-top: 1px solid var(--ring);
    background: var(--card);
    display: grid;
    gap: 14px;
  }

  .detail-actions {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .privacy-note {
    padding: 10px 12px;
    border: 1px dashed var(--ring);
    border-radius: 10px;
    color: var(--muted);
    font-size: 12px;
  }

  .detail-section h4 {
    margin: 0 0 8px;
    font-size: 13px;
  }

  .kv-grid {
    display: grid;
    grid-template-columns: 110px 1fr;
    gap: 4px 12px;
    font-size: 12px;
  }

  .k {
    color: var(--muted);
  }

  .v.err {
    color: var(--danger);
    font-weight: 600;
  }

  .stages {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
    gap: 8px;
  }

  .stage-card,
  .tool-call {
    border: 1px solid var(--ring);
    border-radius: 10px;
    padding: 8px 10px;
    display: grid;
    gap: 6px;
    align-content: start;
  }

  .tool-call {
    margin-bottom: 8px;
  }

  .stage-card.idle {
    opacity: 0.55;
  }

  .stage-card.failed,
  .tool-call.failed {
    border-color: var(--danger);
  }

  .stage-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
    font-size: 12px;
  }

  .stage-name {
    font-weight: 600;
  }

  .bubble {
    padding: 8px 12px;
    border-radius: 12px;
    font-size: 13px;
    white-space: pre-wrap;
    margin-bottom: 8px;
  }

  .bubble.user {
    background: var(--ring);
  }

  .bubble.assistant {
    border: 1px solid var(--ring);
  }

  pre.json,
  pre.error-body {
    margin: 0;
    padding: 8px 10px;
    border-radius: 8px;
    font-size: 11px;
    max-height: 260px;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-word;
  }

  pre.json {
    background: var(--ring);
  }

  pre.error-body {
    background: rgba(239, 68, 68, 0.12);
    color: var(--danger);
  }

  .slots {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 2px;
    font-size: 11px;
  }

  .slots li {
    display: flex;
    justify-content: space-between;
    gap: 8px;
  }

  .slots .slot-type {
    color: var(--muted);
  }

  .slots li.empty .slot-name,
  .slots li.empty .slot-type {
    color: #f59e0b;
  }

  .tool-names {
    margin: 0;
    padding-left: 18px;
    font-size: 12px;
  }

  .mono {
    font-family: monospace;
  }

  .muted {
    color: var(--muted);
    font-size: 12px;
    margin: 0;
  }

  p.muted {
    margin: 0;
  }
</style>
