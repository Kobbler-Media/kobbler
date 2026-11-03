<script lang="ts">
  /**
   * ReadingSection - Reading status icons + progress bar with popover.
   * Design: [Want] [Reading] [Done]  ━━━━━━░░░  67%
   */
  import ReadingProgressPopover from './ReadingProgressPopover.svelte';
  import type { ReadingStatus } from '#lib/types.js';

  interface Props {
    /** Current reading status */
    status: ReadingStatus;
    /** Progress percent (0-100) */
    progressPercent: number | null;
    /** Total pages */
    pageCount: number | null;
    /** Total spent reading minutes */
    spentReadingMinutes: number | null;
    /** Whether controls are editable */
    editable?: boolean;
    /** Callback when status changes */
    onStatusChange?: (status: ReadingStatus) => void;
    /** Callback when progress changes */
    onProgressChange?: (percent: number) => void;
    /** Callback when reading time is logged */
    onLogTime?: (minutes: number) => void;
    /** Callback to clear all reading progress and status */
    onClear?: () => void;
  }

  let {
    status,
    progressPercent,
    pageCount,
    spentReadingMinutes,
    editable = false,
    onStatusChange,
    onProgressChange,
    onLogTime,
    onClear
  }: Props = $props();

  let showPopover = $state(false);

  const statusIcons = [
    { key: 'want_to_read' as const, icon: 'radio_button_unchecked', label: 'Want to Read' },
    { key: 'reading' as const, icon: 'auto_stories', label: 'Currently Reading' },
    { key: 'finished' as const, icon: 'check_circle', label: 'Finished' }
  ];

  const handleStatusClick = (newStatus: ReadingStatus) => {
    if (!editable || !onStatusChange) return;
    // Toggle off if clicking the same status
    if (status === newStatus) {
      onStatusChange(null);
    } else {
      onStatusChange(newStatus);
    }
  };

  const handleProgressClick = () => {
    if (editable) {
      showPopover = true;
    }
  };

  const formatTime = (minutes: number | null): string => {
    if (!minutes) return '0m';
    if (minutes < 60) return `${minutes}m`;
    const hours = Math.floor(minutes / 60);
    const mins = minutes % 60;
    return mins > 0 ? `${hours}h ${mins}m` : `${hours}h`;
  };
</script>

<div class="reading-section">
  <div class="label">Reading</div>
  <div class="controls">
    <div class="status-icons">
      {#each statusIcons as { key, icon, label } (key)}
        <button
          type="button"
          class="status-btn"
          class:active={status === key}
          class:editable
          onclick={() => handleStatusClick(key)}
          title={label}
          disabled={!editable}
          aria-label={label}
        >
          <span class="material-icons-outlined">{icon}</span>
        </button>
      {/each}
    </div>

    <button
      type="button"
      class="progress-bar-wrapper"
      class:clickable={editable}
      onclick={handleProgressClick}
      title={editable ? 'Click to adjust progress' : undefined}
      disabled={!editable}
      aria-label="Adjust reading progress"
    >
      <div class="progress-bar">
        <div
          class="progress-fill"
          style="width: {progressPercent !== null ? Math.min(100, progressPercent) : 0}%"
        ></div>
      </div>
      <span class="progress-text"
        >{progressPercent !== null ? Math.round(progressPercent) : 0}%</span
      >
    </button>
  </div>

  {#if spentReadingMinutes && spentReadingMinutes > 0}
    <div class="reading-time">
      <span class="material-icons-outlined" style="font-size: 14px;">schedule</span>
      <span>{formatTime(spentReadingMinutes)}</span>
    </div>
  {/if}
</div>

{#if showPopover}
  <ReadingProgressPopover
    {progressPercent}
    {pageCount}
    {onProgressChange}
    {onLogTime}
    {onClear}
    onClose={() => (showPopover = false)}
  />
{/if}

<style>
  .reading-section {
    margin-bottom: 16px;
  }

  .label {
    font-size: 10px;
    text-transform: uppercase;
    color: var(--muted);
    font-weight: 700;
    letter-spacing: 0.5px;
    margin-bottom: 8px;
  }

  .controls {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .status-icons {
    display: flex;
    gap: 4px;
  }

  .status-btn {
    width: 32px;
    height: 32px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: default;
    transition: all 0.15s;
  }

  .status-btn.editable {
    cursor: pointer;
  }

  .status-btn.editable:hover {
    background: var(--ring);
    color: var(--ink);
  }

  .status-btn.active {
    background: var(--accent);
    border-color: var(--accent);
    color: #1d130c;
  }

  .status-btn .material-icons-outlined {
    font-size: 18px;
  }

  .progress-bar-wrapper {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 8px;
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    color: inherit;
    text-align: inherit;
  }

  .progress-bar-wrapper.clickable {
    cursor: pointer;
  }

  .progress-bar {
    flex: 1;
    height: 6px;
    background: var(--ring);
    border-radius: 3px;
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 3px;
    transition: width 0.2s ease;
  }

  .progress-text {
    font-size: 12px;
    color: var(--ink);
    min-width: 32px;
    text-align: right;
  }

  .reading-time {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-top: 8px;
    font-size: 11px;
    color: var(--muted);
  }
</style>
