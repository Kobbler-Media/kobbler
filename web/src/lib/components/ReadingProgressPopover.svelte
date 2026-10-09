<script lang="ts">
  /**
   * ReadingProgressPopover - Popover for adjusting reading progress and logging time.
   * Shows percent/page inputs with steppers, and a reading time logger.
   */

  interface Props {
    /** Current progress percent (0-100) */
    progressPercent: number | null;
    /** Total page count (for page input) */
    pageCount: number | null;
    /** Callback when progress changes */
    onProgressChange?: (percent: number) => void;
    /** Callback when reading time is logged */
    onLogTime?: (minutes: number) => void;
    /** Callback to clear all reading progress and status */
    onClear?: () => void;
    /** Callback to close the popover */
    onClose: () => void;
  }

  let { progressPercent, pageCount, onProgressChange, onLogTime, onClear, onClose }: Props =
    $props();

  // Local state for inputs
  let percentInput = $state(0);
  let pageInput = $state(0);
  let timeInput = $state(15);

  // Sync state with props
  $effect(() => {
    if (progressPercent !== null) {
      percentInput = Math.round(progressPercent);
      if (pageCount) {
        pageInput = Math.round((progressPercent / 100) * pageCount);
      }
    }
  });

  // Sync page from percent
  $effect(() => {
    if (pageCount && percentInput !== null) {
      pageInput = Math.round((percentInput / 100) * pageCount);
    }
  });

  const handlePercentChange = (delta: number) => {
    percentInput = Math.max(0, Math.min(100, (percentInput || 0) + delta));
  };

  const handlePageChange = (delta: number) => {
    if (!pageCount) return;
    const newPage = Math.max(0, Math.min(pageCount, (pageInput || 0) + delta));
    pageInput = newPage;
    percentInput = Math.round((newPage / pageCount) * 100);
  };

  const handleTimeChange = (delta: number) => {
    timeInput = Math.max(1, (timeInput || 0) + delta);
  };

  const saveProgress = () => {
    if (onProgressChange && percentInput !== null) {
      onProgressChange(percentInput);
    }
    onClose();
  };

  const logTime = () => {
    if (onLogTime && timeInput > 0) {
      onLogTime(timeInput);
    }
  };

  const handleBackdropClick = (e: MouseEvent) => {
    if (e.target === e.currentTarget) {
      saveProgress();
    }
  };

  const handleKeydown = (e: KeyboardEvent) => {
    if (e.key === 'Escape') {
      saveProgress();
    }
  };
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="popover-backdrop" role="presentation" onclick={handleBackdropClick}>
  <div class="popover" role="dialog" aria-label="Reading progress">
    <div class="popover-row">
      <label class="row-label" for="percent-input">Percent</label>
      <div class="stepper-group">
        <button type="button" class="stepper-btn" onclick={() => handlePercentChange(-5)}>-</button>
        <input
          id="percent-input"
          type="number"
          class="stepper-input"
          bind:value={percentInput}
          min="0"
          max="100"
          aria-label="Progress percent"
        />
        <button type="button" class="stepper-btn" onclick={() => handlePercentChange(5)}>+</button>
        <span class="unit">%</span>
      </div>
    </div>

    {#if pageCount}
      <div class="popover-row">
        <label class="row-label" for="page-input">Page</label>
        <div class="stepper-group">
          <button type="button" class="stepper-btn" onclick={() => handlePageChange(-10)}>-</button>
          <input
            id="page-input"
            type="number"
            class="stepper-input"
            bind:value={pageInput}
            min="0"
            max={pageCount}
            aria-label="Current page"
          />
          <button type="button" class="stepper-btn" onclick={() => handlePageChange(10)}>+</button>
          <span class="unit">/ {pageCount}</span>
        </div>
      </div>
    {/if}

    <div class="divider"></div>

    <div class="popover-row">
      <label class="row-label" for="time-input">Log time</label>
      <div class="stepper-group">
        <button type="button" class="stepper-btn" onclick={() => handleTimeChange(-5)}>-</button>
        <input
          id="time-input"
          type="number"
          class="stepper-input"
          bind:value={timeInput}
          min="1"
          aria-label="Reading time"
        />
        <button type="button" class="stepper-btn" onclick={() => handleTimeChange(5)}>+</button>
        <span class="unit">min</span>
        <button type="button" class="log-btn" onclick={logTime}>Log</button>
      </div>
    </div>

    {#if onClear && (progressPercent ?? 0) > 0}
      <div class="divider"></div>
      <button
        type="button"
        class="clear-btn"
        onclick={() => {
          onClear();
          onClose();
        }}
      >
        <span class="material-icons-outlined" style="font-size: 14px;">delete_outline</span>
        Clear progress
      </button>
    {/if}

    <div class="hint">Saves when you click away</div>
  </div>
</div>

<style>
  .popover-backdrop {
    position: fixed;
    inset: 0;
    z-index: 100;
    background: rgba(0, 0, 0, 0.3);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .popover {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    padding: 16px;
    min-width: 260px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.3);
  }

  .popover-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 12px;
  }

  .row-label {
    font-size: 12px;
    color: var(--muted);
    min-width: 60px;
  }

  .stepper-group {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .stepper-btn {
    width: 28px;
    height: 28px;
    border-radius: 6px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 16px;
    font-weight: 600;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: all 0.15s;
  }

  .stepper-btn:hover {
    background: var(--ring);
  }

  .stepper-input {
    width: 50px;
    background: transparent;
    border: none;
    color: var(--ink);
    font-size: 14px;
    font-weight: 600;
    text-align: center;
    -moz-appearance: textfield;
    appearance: textfield;
  }

  .stepper-input::-webkit-outer-spin-button,
  .stepper-input::-webkit-inner-spin-button {
    -webkit-appearance: none;
    margin: 0;
  }

  .unit {
    font-size: 12px;
    color: var(--muted);
    min-width: 40px;
  }

  .divider {
    height: 1px;
    background: var(--ring);
    margin: 16px 0;
  }

  .log-btn {
    padding: 4px 10px;
    border-radius: 6px;
    border: none;
    background: var(--accent);
    color: #1d130c;
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
    margin-left: 8px;
    transition: all 0.15s;
  }

  .log-btn:hover {
    filter: brightness(1.1);
  }

  .clear-btn {
    width: 100%;
    padding: 6px 0;
    border: none;
    background: transparent;
    color: var(--muted);
    font-size: 12px;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 4px;
    border-radius: 6px;
    transition: all 0.15s;
  }

  .clear-btn:hover {
    background: var(--ring);
    color: var(--ink);
  }

  .hint {
    font-size: 10px;
    color: var(--muted);
    text-align: center;
    margin-top: 8px;
  }
</style>
