<script lang="ts">
  import type { LivetvClashNotice } from '$lib/types';

  interface Props {
    notice: LivetvClashNotice;
    busy?: boolean;
    onIgnore: () => void;
    onRecordAlternative: (guideId: string) => void;
  }

  let { notice, busy = false, onIgnore, onRecordAlternative }: Props = $props();

  function clock(iso: string): string {
    const d = new Date(iso);
    return isNaN(d.getTime())
      ? ''
      : d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }
</script>

<div class="notice">
  <span class="material-icons-outlined">schedule</span>
  <div class="body">
    <span class="headline">
      {notice.guide.title} starts at {clock(notice.guide.start_time)} and every tuner is busy
    </span>
    <span class="reason">{notice.reason}</span>
  </div>
  <div class="actions">
    <button type="button" class="ghost" onclick={onIgnore}>Ignore</button>
    {#if notice.alternative}
      <button
        type="button"
        class="primary"
        disabled={busy}
        onclick={() => notice.alternative && onRecordAlternative(notice.alternative.id)}
      >
        Record the {clock(notice.alternative.start_time)}
      </button>
    {/if}
  </div>
</div>

<style>
  .notice {
    display: flex;
    align-items: center;
    gap: 14px;
    border: 1px solid var(--accent);
    background: var(--accent-dim);
    border-radius: 12px;
    padding: 13px 15px;
    margin-bottom: 20px;
  }

  .notice .material-icons-outlined {
    font-size: 19px;
    color: var(--accent);
    flex-shrink: 0;
  }

  .body {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex-grow: 1;
  }

  .headline {
    font-size: 13.5px;
    font-weight: 600;
  }

  .reason {
    font-size: 12px;
    color: var(--muted);
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .ghost,
  .primary {
    height: 32px;
    padding: 0 15px;
    border-radius: 8px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
  }

  .ghost {
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
  }

  .ghost:hover {
    color: var(--ink);
  }

  .primary {
    border: 1px solid var(--accent);
    background: var(--accent);
    color: var(--bg);
    font-weight: 700;
  }

  .primary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
