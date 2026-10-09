<script lang="ts">
  interface Props {
    value: Date;
    minDate?: Date;
    maxDate?: Date;
    onChange: (next: Date) => void;
  }

  let { value, minDate, maxDate, onChange }: Props = $props();

  const fmt = new Intl.DateTimeFormat(undefined, {
    weekday: 'short',
    day: 'numeric',
    month: 'short'
  });

  function startOfDay(d: Date): Date {
    const c = new Date(d);
    c.setHours(0, 0, 0, 0);
    return c;
  }

  function shift(days: number) {
    const next = startOfDay(value);
    next.setDate(next.getDate() + days);
    onChange(next);
  }

  const canPrev = $derived(!minDate || startOfDay(value) > startOfDay(minDate));
  const canNext = $derived(!maxDate || startOfDay(value) < startOfDay(maxDate));
</script>

<div class="stepper">
  <button
    type="button"
    class="step"
    disabled={!canPrev}
    aria-label="Previous day"
    onclick={() => shift(-1)}
  >
    <span class="material-icons-outlined">chevron_left</span>
  </button>
  <span class="label">{fmt.format(value)}</span>
  <button
    type="button"
    class="step"
    disabled={!canNext}
    aria-label="Next day"
    onclick={() => shift(1)}
  >
    <span class="material-icons-outlined">chevron_right</span>
  </button>
</div>

<style>
  .stepper {
    display: flex;
    align-items: center;
    border: 1px solid var(--ring);
    border-radius: 9px;
    overflow: hidden;
    height: 34px;
    background: var(--card);
  }

  .step {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0 8px;
    height: 100%;
    background: none;
    border: none;
    color: var(--muted);
    cursor: pointer;
  }

  .step:first-child {
    border-right: 1px solid var(--ring);
  }

  .step:last-child {
    border-left: 1px solid var(--ring);
  }

  .step:hover:not(:disabled) {
    color: var(--ink);
    background: var(--bg-2);
  }

  .step:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .step .material-icons-outlined {
    font-size: 18px;
  }

  .label {
    display: flex;
    align-items: center;
    padding: 0 14px;
    height: 100%;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--ink);
    white-space: nowrap;
  }
</style>
