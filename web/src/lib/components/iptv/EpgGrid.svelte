<script lang="ts">
  import { onMount } from 'svelte';
  import type { IptvChannel, IptvEpgEntry } from '$lib/types';

  interface Props {
    channels: IptvChannel[];
    programs: IptvEpgEntry[];
    startTime: Date;
    endTime: Date;
    onprogramclick?: (program: IptvEpgEntry) => void;
  }

  let { channels, programs, startTime, endTime, onprogramclick }: Props = $props();

  let gridEl: HTMLDivElement | undefined = $state();
  let nowLinePercent = $state(0);

  const totalMinutes = $derived((endTime.getTime() - startTime.getTime()) / 60000);

  const slotCount = $derived(Math.ceil(totalMinutes / 30));

  const timeSlots = $derived(
    Array.from({ length: slotCount }, (_, i) => {
      const t = new Date(startTime.getTime() + i * 30 * 60000);
      return t;
    })
  );

  const programsByChannel = $derived(() => {
    const map = new Map<string, IptvEpgEntry[]>();
    for (const p of programs) {
      const list = map.get(p.channel_id) ?? [];
      list.push(p);
      map.set(p.channel_id, list);
    }
    return map;
  });

  function programStyle(program: IptvEpgEntry): string {
    const pStart = new Date(program.start_time).getTime();
    const pStop = new Date(program.stop_time).getTime();
    const clampStart = Math.max(pStart, startTime.getTime());
    const clampStop = Math.min(pStop, endTime.getTime());
    const leftPct = ((clampStart - startTime.getTime()) / (totalMinutes * 60000)) * 100;
    const widthPct = ((clampStop - clampStart) / (totalMinutes * 60000)) * 100;
    return `left: ${leftPct}%; width: ${widthPct}%;`;
  }

  function isCurrent(program: IptvEpgEntry): boolean {
    const now = Date.now();
    return (
      now >= new Date(program.start_time).getTime() && now < new Date(program.stop_time).getTime()
    );
  }

  function formatSlotTime(date: Date): string {
    return date.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  }

  function updateNowLine() {
    const now = Date.now();
    if (now < startTime.getTime() || now > endTime.getTime()) {
      nowLinePercent = -1;
    } else {
      nowLinePercent = ((now - startTime.getTime()) / (totalMinutes * 60000)) * 100;
    }
  }

  onMount(() => {
    updateNowLine();
    const interval = setInterval(updateNowLine, 30000);

    // Auto-scroll to current time
    if (gridEl && nowLinePercent > 0) {
      const scrollTarget = (nowLinePercent / 100) * gridEl.scrollWidth - gridEl.clientWidth / 3;
      gridEl.scrollLeft = Math.max(0, scrollTarget);
    }

    return () => clearInterval(interval);
  });
</script>

<div class="epg-grid-wrapper">
  <div class="channel-labels">
    <div class="time-header-spacer"></div>
    {#each channels as channel (channel.id)}
      <div class="channel-label">
        {#if channel.icon_url}
          <img src={channel.icon_url} alt="" class="channel-icon" />
        {/if}
        <span class="channel-label-name">{channel.name}</span>
      </div>
    {/each}
  </div>

  <div class="epg-scroll" bind:this={gridEl}>
    <div class="time-header">
      {#each timeSlots as slot (slot.getTime())}
        <div class="time-slot" style="width: {100 / slotCount}%;">
          {formatSlotTime(slot)}
        </div>
      {/each}
    </div>

    <div class="program-rows" style="position: relative;">
      {#if nowLinePercent >= 0}
        <div class="now-line" style="left: {nowLinePercent}%;"></div>
      {/if}

      {#each channels as channel (channel.id)}
        <div class="program-row">
          {#each programsByChannel().get(channel.id) ?? [] as program (program.id)}
            <button
              class="program-cell"
              class:current={isCurrent(program)}
              type="button"
              style={programStyle(program)}
              title="{program.title} ({formatSlotTime(
                new Date(program.start_time)
              )} - {formatSlotTime(new Date(program.stop_time))})"
              onclick={() => onprogramclick?.(program)}
            >
              <span class="program-cell-title">{program.title}</span>
            </button>
          {/each}
        </div>
      {/each}
    </div>
  </div>
</div>

<style>
  .epg-grid-wrapper {
    display: flex;
    border: 1px solid var(--ring);
    border-radius: 12px;
    overflow: hidden;
    background: var(--card);
  }

  .channel-labels {
    flex-shrink: 0;
    width: 160px;
    border-right: 1px solid var(--ring);
    background: var(--card);
    z-index: 2;
  }

  .time-header-spacer {
    height: 36px;
    border-bottom: 1px solid var(--ring);
  }

  .channel-label {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 48px;
    padding: 0 12px;
    border-bottom: 1px solid var(--ring);
    overflow: hidden;
  }

  .channel-icon {
    width: 24px;
    height: 24px;
    border-radius: 4px;
    object-fit: contain;
    flex-shrink: 0;
  }

  .channel-label-name {
    font-size: 12px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .epg-scroll {
    flex: 1;
    overflow-x: auto;
    overflow-y: hidden;
  }

  .time-header {
    display: flex;
    height: 36px;
    border-bottom: 1px solid var(--ring);
    min-width: max(100%, 1200px);
  }

  .time-slot {
    display: flex;
    align-items: center;
    padding: 0 8px;
    font-size: 11px;
    color: var(--muted);
    font-weight: 500;
    border-right: 1px solid var(--ring);
    flex-shrink: 0;
    box-sizing: border-box;
  }

  .program-rows {
    min-width: max(100%, 1200px);
  }

  .program-row {
    position: relative;
    height: 48px;
    border-bottom: 1px solid var(--ring);
  }

  .program-cell {
    position: absolute;
    top: 2px;
    height: calc(100% - 4px);
    border-radius: 6px;
    padding: 0 8px;
    display: flex;
    align-items: center;
    background: rgba(255, 255, 255, 0.06);
    border: 1px solid var(--ring);
    color: var(--ink);
    cursor: pointer;
    overflow: hidden;
    transition: all 0.15s;
    box-sizing: border-box;
  }

  :global([data-theme='light']) .program-cell {
    background: rgba(0, 0, 0, 0.04);
  }

  .program-cell:hover {
    border-color: var(--accent);
    z-index: 1;
  }

  .program-cell.current {
    background: var(--accent-dim);
    border-color: var(--accent);
  }

  .program-cell-title {
    font-size: 11px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .now-line {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 2px;
    background: var(--accent);
    z-index: 3;
    pointer-events: none;
  }

  .now-line::before {
    content: '';
    position: absolute;
    top: -4px;
    left: -4px;
    width: 10px;
    height: 10px;
    background: var(--accent);
    border-radius: 50%;
  }
</style>
