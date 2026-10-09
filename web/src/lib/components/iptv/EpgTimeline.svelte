<script lang="ts">
  import type { IptvEpgEntry } from '$lib/types';

  interface Props {
    programs: IptvEpgEntry[];
    onselect?: (program: IptvEpgEntry) => void;
  }

  let { programs, onselect }: Props = $props();

  const now = $state(new Date());
  let intervalId: ReturnType<typeof setInterval> | undefined;

  $effect(() => {
    intervalId = setInterval(() => {
      // Force reactivity by reassigning
      now.setTime(Date.now());
    }, 60000);
    return () => {
      if (intervalId) clearInterval(intervalId);
    };
  });

  function isCurrentProgram(program: IptvEpgEntry): boolean {
    const start = new Date(program.start_time);
    const stop = new Date(program.stop_time);
    const n = new Date();
    return n >= start && n < stop;
  }

  function isPastProgram(program: IptvEpgEntry): boolean {
    const stop = new Date(program.stop_time);
    return new Date() >= stop;
  }

  function formatTime(dateStr: string): string {
    return new Date(dateStr).toLocaleTimeString(undefined, {
      hour: '2-digit',
      minute: '2-digit'
    });
  }

  function progressPercent(program: IptvEpgEntry): number {
    const start = new Date(program.start_time).getTime();
    const stop = new Date(program.stop_time).getTime();
    const n = Date.now();
    if (n <= start) return 0;
    if (n >= stop) return 100;
    return ((n - start) / (stop - start)) * 100;
  }
</script>

<div class="epg-timeline">
  {#each programs as program (program.id)}
    <button
      class="program-block"
      class:current={isCurrentProgram(program)}
      class:past={isPastProgram(program)}
      type="button"
      onclick={() => onselect?.(program)}
    >
      <div class="program-time">{formatTime(program.start_time)}</div>
      <div class="program-title">{program.title}</div>
      {#if isCurrentProgram(program)}
        <div class="progress-bar">
          <div class="progress-fill" style="width: {progressPercent(program)}%"></div>
        </div>
      {/if}
    </button>
  {/each}
  {#if programs.length === 0}
    <div class="no-epg">No program guide available</div>
  {/if}
</div>

<style>
  .epg-timeline {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .program-block {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 10px 14px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 8px;
    text-align: left;
    cursor: pointer;
    color: var(--ink);
    transition: all 0.15s;
    position: relative;
    overflow: hidden;
    width: 100%;
  }

  .program-block:hover {
    border-color: var(--accent);
  }

  .program-block.current {
    border-color: var(--accent);
    background: var(--accent-dim);
  }

  .program-block.past {
    opacity: 0.5;
  }

  .program-time {
    font-size: 11px;
    color: var(--muted);
    font-weight: 500;
  }

  .program-title {
    font-size: 13px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .progress-bar {
    height: 2px;
    background: var(--ring);
    border-radius: 1px;
    margin-top: 4px;
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 1px;
    transition: width 1s linear;
  }

  .no-epg {
    padding: 16px;
    text-align: center;
    color: var(--muted);
    font-size: 13px;
  }
</style>
