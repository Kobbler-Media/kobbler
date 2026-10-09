<script lang="ts">
  import type { LivetvChannel, LivetvGuideChannel, LivetvGuideEntry } from '$lib/types';
  import EpgProgramme from './EpgProgramme.svelte';
  import { networkTint } from './tint';

  interface Props {
    channels: LivetvGuideChannel[];
    windowStart: Date;
    windowEnd: Date;
    now: Date;
    pxPerMinute?: number;
    selectedEntryId: string | null;
    onSelect: (entry: LivetvGuideEntry, channel: LivetvChannel) => void;
  }

  let {
    channels,
    windowStart,
    windowEnd,
    now,
    pxPerMinute = 4,
    selectedEntryId,
    onSelect
  }: Props = $props();

  const SLOT_MINUTES = 30;

  const startMs = $derived(windowStart.getTime());
  const endMs = $derived(windowEnd.getTime());
  const totalMinutes = $derived(Math.max((endMs - startMs) / 60000, SLOT_MINUTES));
  const totalWidth = $derived(totalMinutes * pxPerMinute);
  const slotWidth = $derived(SLOT_MINUTES * pxPerMinute);

  const timeFmt = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' });

  const slots = $derived(
    Array.from({ length: Math.round(totalMinutes / SLOT_MINUTES) }, (_, i) => {
      const at = new Date(startMs + i * SLOT_MINUTES * 60000);
      return { key: at.getTime(), label: timeFmt.format(at) };
    })
  );

  const nowOffset = $derived(((now.getTime() - startMs) / 60000) * pxPerMinute);
  const nowVisible = $derived(now.getTime() >= startMs && now.getTime() <= endMs);

  function placement(entry: LivetvGuideEntry) {
    const s = Math.max(new Date(entry.start_time).getTime(), startMs);
    const e = Math.min(new Date(entry.end_time).getTime(), endMs);
    return {
      left: ((s - startMs) / 60000) * pxPerMinute,
      width: Math.max(((e - s) / 60000) * pxPerMinute, 3)
    };
  }

  function inWindow(entry: LivetvGuideEntry): boolean {
    return (
      new Date(entry.end_time).getTime() > startMs && new Date(entry.start_time).getTime() < endMs
    );
  }

  let lanesEl = $state<HTMLDivElement | null>(null);
  let scrolledWindow = 0;

  // Re-centre on the now line once per window, not on every clock tick.
  $effect(() => {
    if (!lanesEl || startMs === scrolledWindow) return;
    scrolledWindow = startMs;
    if (!nowVisible) {
      lanesEl.scrollLeft = 0;
      return;
    }
    lanesEl.scrollLeft = Math.max(nowOffset - lanesEl.clientWidth / 3, 0);
  });
</script>

<div class="grid">
  <div class="labels">
    <div class="labels-head">Channel</div>
    {#each channels as row (row.channel.id)}
      <div class="label-row">
        <span class="lcn">{row.channel.lcn ?? ''}</span>
        {#if row.channel.logo_url}
          <img class="logo" src={row.channel.logo_url} alt="" loading="lazy" />
        {:else}
          <span class="tint" style:background={networkTint(row.channel.network)}></span>
        {/if}
        <span class="name">{row.channel.name}</span>
        {#if row.channel.hd}
          <span class="hd" style:background={networkTint(row.channel.network)}>HD</span>
        {/if}
      </div>
    {/each}
  </div>

  <div class="lanes" bind:this={lanesEl}>
    <div class="lanes-inner" style:width="{totalWidth}px">
      <div class="slots">
        {#each slots as slot (slot.key)}
          <div class="slot" style:width="{slotWidth}px">{slot.label}</div>
        {/each}
      </div>
      <div class="rows">
        {#if nowVisible}
          <div class="now-line" style:left="{nowOffset}px"></div>
          <div class="now-dot" style:left="{nowOffset}px"></div>
        {/if}
        {#each channels as row (row.channel.id)}
          <div class="row">
            {#each row.entries.filter(inWindow) as entry (entry.id)}
              {@const place = placement(entry)}
              <EpgProgramme
                {entry}
                tint={networkTint(row.channel.network)}
                left={place.left}
                width={place.width}
                live={entry.is_now}
                selected={entry.id === selectedEntryId}
                onSelect={(e) => onSelect(e, row.channel)}
              />
            {/each}
          </div>
        {/each}
      </div>
    </div>
  </div>
</div>

<style>
  .grid {
    display: flex;
    border: 1px solid var(--ring);
    border-radius: 12px;
    overflow: hidden;
    background: var(--card);
  }

  .labels {
    flex-shrink: 0;
    width: 190px;
    border-right: 1px solid var(--ring);
    background: var(--card);
  }

  .labels-head {
    height: 36px;
    border-bottom: 1px solid var(--ring);
    display: flex;
    align-items: center;
    padding: 0 12px;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: var(--muted);
  }

  .label-row {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 45px;
    padding: 0 12px;
    border-bottom: 1px solid var(--ring);
    overflow: hidden;
    flex-shrink: 0;
  }

  .label-row:last-child,
  .row:last-child {
    border-bottom: none;
  }

  .lcn {
    width: 26px;
    flex-shrink: 0;
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
    text-align: right;
  }

  .tint {
    width: 3px;
    height: 22px;
    border-radius: 2px;
    flex-shrink: 0;
  }

  .logo {
    width: 26px;
    height: 26px;
    object-fit: contain;
    border-radius: 4px;
    flex-shrink: 0;
  }

  .name {
    font-size: 12px;
    font-weight: 500;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex-grow: 1;
  }

  .hd {
    font-size: 9px;
    font-weight: 700;
    letter-spacing: 0.04em;
    color: var(--bg);
    border-radius: 3px;
    padding: 1px 4px;
    line-height: 1.4;
    flex-shrink: 0;
  }

  .lanes {
    flex-grow: 1;
    min-width: 0;
    overflow-x: auto;
    overflow-y: hidden;
  }

  .lanes-inner {
    position: relative;
  }

  .slots {
    display: flex;
    height: 36px;
    border-bottom: 1px solid var(--ring);
  }

  .slot {
    box-sizing: border-box;
    display: flex;
    align-items: center;
    padding: 0 8px;
    font-size: 11px;
    font-weight: 500;
    color: var(--muted);
    border-right: 1px solid var(--ring);
    flex-shrink: 0;
    font-variant-numeric: tabular-nums;
  }

  .rows {
    position: relative;
  }

  .row {
    position: relative;
    height: 45px;
    border-bottom: 1px solid var(--ring);
  }

  .now-line {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 2px;
    background: var(--accent);
    z-index: 3;
    box-shadow: 0 0 10px rgba(211, 140, 90, 0.6);
    pointer-events: none;
  }

  .now-dot {
    position: absolute;
    top: -4px;
    transform: translateX(-50%);
    z-index: 4;
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--accent);
    box-shadow: 0 0 8px rgba(211, 140, 90, 0.7);
    pointer-events: none;
  }
</style>
