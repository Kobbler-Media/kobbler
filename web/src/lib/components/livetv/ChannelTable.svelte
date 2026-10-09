<script lang="ts">
  import type { SvelteSet } from 'svelte/reactivity';
  import type { LivetvChannel } from '$lib/types';
  import { networkTint, typeTint, signalTint } from './tint';

  interface Props {
    channels: LivetvChannel[];
    selectedIds: SvelteSet<string>;
    busyIds: SvelteSet<string>;
    onToggleSelect: (id: string, additive: boolean) => void;
    onToggleEnabled: (channel: LivetvChannel) => void;
    onWatch?: (channel: LivetvChannel) => void;
    onWatchIntent?: (channel: LivetvChannel) => void;
  }

  let {
    channels,
    selectedIds,
    busyIds,
    onToggleSelect,
    onToggleEnabled,
    onWatch,
    onWatchIntent
  }: Props = $props();

  function noteFor(channel: LivetvChannel): string {
    if (channel.type_label === 'Dead') return 'unnamed service · auto-hidden';
    if (!channel.has_guide) return 'no guide data';
    if (!channel.enabled) return 'hidden';
    return '';
  }

  function renamedFrom(channel: LivetvChannel): string | null {
    if (!channel.display_name) return null;
    const scan = channel.scan_name ?? channel.callsign;
    if (!scan || scan === channel.display_name) return null;
    return scan;
  }
</script>

<div class="table" role="grid" aria-label="Scanned services">
  <div class="head" role="row">
    <span class="col on" role="columnheader">On</span>
    <span class="col lcn" role="columnheader">LCN</span>
    <span class="col logo" role="columnheader"></span>
    <span class="col name" role="columnheader">Name</span>
    <span class="col network" role="columnheader">Network</span>
    <span class="col type" role="columnheader">Type</span>
    <span class="col region" role="columnheader">Region</span>
    <span class="col mux" role="columnheader">Mux</span>
    <span class="col signal" role="columnheader">Signal</span>
    <span class="col epg" role="columnheader">EPG</span>
    <span class="col notes" role="columnheader">Notes</span>
    {#if onWatch}
      <span class="col watch" role="columnheader">Watch</span>
    {/if}
  </div>

  <div class="body">
    {#each channels as channel (channel.id)}
      {@const selected = selectedIds.has(channel.id)}
      {@const note = noteFor(channel)}
      {@const previous = renamedFrom(channel)}
      <div
        class="row"
        class:selected
        class:off={!channel.enabled}
        role="row"
        tabindex="0"
        aria-selected={selected}
        onclick={(e) => onToggleSelect(channel.id, e.shiftKey || e.metaKey || e.ctrlKey)}
        onkeydown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            onToggleSelect(channel.id, e.shiftKey);
          }
        }}
      >
        <span class="col on" role="gridcell">
          <button
            type="button"
            class="switch"
            class:checked={channel.enabled}
            disabled={busyIds.has(channel.id)}
            role="switch"
            aria-checked={channel.enabled}
            aria-label={channel.enabled ? `Hide ${channel.name}` : `Enable ${channel.name}`}
            onclick={(e) => {
              e.stopPropagation();
              onToggleEnabled(channel);
            }}
          >
            <span class="knob"></span>
          </button>
        </span>

        <span class="col lcn" role="gridcell">{channel.lcn ?? '—'}</span>

        <span class="col logo" role="gridcell">
          {#if channel.logo_url}
            <img src={channel.logo_url} alt="" loading="lazy" />
          {/if}
        </span>

        <span class="col name" role="gridcell">
          <span class="name-main">
            {channel.name}
            {#if channel.favourite}
              <span class="material-icons-outlined fav" title="Favourite">star</span>
            {/if}
          </span>
          {#if previous}
            <span class="name-was">{previous}</span>
          {/if}
        </span>

        <span class="col network" role="gridcell">
          <span class="dot" style:background={networkTint(channel.network)}></span>
          <span class="dim">{channel.network ?? '—'}</span>
        </span>

        <span class="col type" role="gridcell">
          <span
            class="type-badge"
            style:color={typeTint(channel.type_label)}
            style:border-color={typeTint(channel.type_label)}
          >
            {channel.type_label}
          </span>
        </span>

        <span class="col region dim" role="gridcell">{channel.region ?? '—'}</span>
        <span class="col mux dim num" role="gridcell">{channel.mux_label ?? '—'}</span>

        <span class="col signal" role="gridcell">
          {#if channel.signal_quality != null}
            <span class="bar">
              <span
                class="fill"
                style:width="{channel.signal_quality}%"
                style:background={signalTint(channel.signal_quality)}
              ></span>
            </span>
            <span class="num dim">{channel.signal_quality}</span>
          {:else}
            <span class="dim">—</span>
          {/if}
        </span>

        <span class="col epg" role="gridcell">
          <span
            class="material-icons-outlined"
            class:yes={channel.has_guide}
            class:no={!channel.has_guide}
          >
            {channel.has_guide ? 'check' : 'close'}
          </span>
        </span>

        <span class="col notes" role="gridcell">
          {#if note}<span class="note">{note}</span>{/if}
        </span>

        {#if onWatch}
          <span class="col watch" role="gridcell">
            <button
              type="button"
              class="watch-btn"
              disabled={!channel.enabled}
              title={channel.enabled ? `Watch ${channel.name}` : 'Enable this service first'}
              aria-label={`Watch ${channel.name}`}
              onpointerdown={() => onWatchIntent?.(channel)}
              onclick={(e) => {
                e.stopPropagation();
                onWatch?.(channel);
              }}
            >
              <span class="material-icons-outlined">play_arrow</span>
            </button>
          </span>
        {/if}
      </div>
    {/each}
  </div>
</div>

<style>
  .table {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .head,
  .row {
    display: flex;
    align-items: center;
    padding: 0 14px;
    box-sizing: border-box;
    min-width: 940px;
  }

  .head {
    height: 32px;
    border-bottom: 1px solid var(--ring);
    background: var(--bg-2);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: var(--muted);
    position: sticky;
    top: 0;
    z-index: 1;
  }

  .row {
    height: 46px;
    border-bottom: 1px solid var(--ring);
    cursor: pointer;
    border-left: 2px solid transparent;
    padding-left: 12px;
  }

  .row:hover {
    background: rgba(255, 255, 255, 0.03);
  }

  :global([data-theme='light']) .row:hover {
    background: rgba(0, 0, 0, 0.03);
  }

  .row.selected {
    border-left-color: var(--accent);
    background: var(--accent-dim);
  }

  .row.off .name-main {
    color: var(--muted);
  }

  .col {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    overflow: hidden;
  }

  .on {
    width: 44px;
  }
  .lcn {
    width: 44px;
    font-size: 12px;
    font-weight: 600;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }
  .logo {
    width: 34px;
    justify-content: center;
  }
  .logo img {
    width: 26px;
    height: 26px;
    object-fit: contain;
    border-radius: 4px;
  }
  .name {
    width: 250px;
    flex-direction: column;
    align-items: flex-start;
    gap: 1px;
    padding-right: 12px;
  }
  .network {
    width: 88px;
  }
  .type {
    width: 74px;
  }
  .region {
    width: 92px;
    font-size: 11.5px;
  }
  .mux {
    width: 92px;
    font-size: 11px;
  }
  .signal {
    width: 78px;
  }
  .epg {
    width: 44px;
    justify-content: center;
  }
  .watch {
    width: 52px;
    justify-content: flex-end;
  }

  .watch-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    cursor: pointer;
  }

  .watch-btn:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }

  .watch-btn:disabled {
    opacity: 0.35;
    cursor: not-allowed;
  }

  .watch-btn .material-icons-outlined {
    font-size: 17px;
  }

  .notes {
    flex-grow: 1;
    flex-shrink: 1;
    min-width: 0;
  }

  .name-main {
    font-size: 12.5px;
    font-weight: 600;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
    display: flex;
    align-items: center;
    gap: 5px;
  }

  .name-was {
    font-size: 10.5px;
    color: var(--muted);
    text-decoration: line-through;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
  }

  .fav {
    font-size: 13px;
    color: var(--accent);
  }

  .dim {
    font-size: 11.5px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .num {
    font-variant-numeric: tabular-nums;
  }

  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .type-badge {
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.04em;
    border: 1px solid;
    border-radius: 4px;
    padding: 2px 6px;
    white-space: nowrap;
  }

  .bar {
    width: 26px;
    height: 4px;
    border-radius: 2px;
    background: var(--ring);
    overflow: hidden;
    display: block;
    flex-shrink: 0;
  }

  .fill {
    display: block;
    height: 4px;
    border-radius: 2px;
  }

  .epg .material-icons-outlined {
    font-size: 15px;
  }

  .yes {
    color: var(--success);
  }

  .no {
    color: var(--ring);
  }

  .note {
    font-size: 10.5px;
    color: var(--muted);
    font-style: italic;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .switch {
    width: 30px;
    height: 17px;
    border-radius: 9px;
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
    width: 13px;
    height: 13px;
    border-radius: 50%;
    background: var(--muted);
  }

  .switch.checked .knob {
    background: #1d130c;
  }
</style>
