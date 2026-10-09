<script lang="ts">
  interface Props {
    label: string;
    meta?: string;
    checked: boolean;
    disabled?: boolean;
    onChange: (next: boolean) => void;
  }

  let { label, meta, checked, disabled = false, onChange }: Props = $props();
</script>

<div class="toggle-row">
  <span class="label">{label}</span>
  <span class="spacer"></span>
  {#if meta}
    <span class="meta">{meta}</span>
  {/if}
  <button
    type="button"
    class="switch"
    class:checked
    role="switch"
    aria-checked={checked}
    aria-label={label}
    {disabled}
    onclick={() => onChange(!checked)}
  >
    <span class="knob"></span>
  </button>
</div>

<style>
  .toggle-row {
    display: flex;
    align-items: center;
    gap: 11px;
    min-height: 34px;
  }

  .label {
    font-size: 12.5px;
    color: var(--ink);
  }

  .spacer {
    flex-grow: 1;
  }

  .meta {
    font-size: 11px;
    color: var(--muted);
    opacity: 0.8;
    text-align: right;
  }

  .switch {
    width: 34px;
    height: 19px;
    border-radius: 10px;
    background: var(--ring);
    border: none;
    display: flex;
    align-items: center;
    padding: 2px;
    box-sizing: border-box;
    justify-content: flex-start;
    flex-shrink: 0;
    cursor: pointer;
  }

  .switch.checked {
    background: var(--accent);
    justify-content: flex-end;
  }

  .switch:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .knob {
    width: 15px;
    height: 15px;
    border-radius: 50%;
    background: var(--muted);
  }

  .switch.checked .knob {
    background: #1d130c;
  }
</style>
