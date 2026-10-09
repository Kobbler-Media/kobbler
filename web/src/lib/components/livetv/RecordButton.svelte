<script lang="ts">
  interface Props {
    recording: boolean;
    busy?: boolean;
    disabled?: boolean;
    title?: string;
    variant?: 'default' | 'overlay';
    onclick: () => void;
  }

  let {
    recording,
    busy = false,
    disabled = false,
    title,
    variant = 'default',
    onclick
  }: Props = $props();
</script>

<button
  type="button"
  class="record-btn"
  class:recording
  class:overlay={variant === 'overlay'}
  disabled={disabled || busy}
  {title}
  {onclick}
>
  <span class="ring"><span class="core"></span></span>
  {busy ? 'Working…' : recording ? 'Recording' : 'Record'}
</button>

<style>
  .record-btn {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 34px;
    padding: 0 14px;
    border-radius: 9px;
    border: 1px solid var(--ring);
    background: transparent;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--ink);
    cursor: pointer;
    white-space: nowrap;
  }

  .record-btn:hover:not(:disabled) {
    border-color: var(--danger);
  }

  .record-btn.overlay {
    height: 40px;
    border-radius: 11px;
    background: rgba(0, 0, 0, 0.45);
    border-color: rgba(255, 255, 255, 0.1);
  }

  .record-btn.overlay:hover:not(:disabled) {
    background: rgba(0, 0, 0, 0.62);
    border-color: var(--danger);
  }

  .record-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .record-btn.recording {
    border-color: var(--danger);
    color: var(--danger);
  }

  .ring {
    width: 13px;
    height: 13px;
    border-radius: 50%;
    border: 2px solid var(--danger);
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .core {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--danger);
  }
</style>
