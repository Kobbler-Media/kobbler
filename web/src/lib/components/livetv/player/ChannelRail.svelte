<script lang="ts">
  import type { LivetvChannel } from '$lib/types';
  import FacetPills from '../FacetPills.svelte';
  import { networkTint } from '../tint';

  interface Props {
    layout: 'rail' | 'strip';
    channels: LivetvChannel[];
    totalCount: number;
    activeChannelId: string | null;
    focusedChannelId: string | null;
    filter: string;
    facet: string | null;
    facetLabel: string;
    onFilter: (value: string) => void;
    onFacet: (value: string | null) => void;
    onPick: (channel: LivetvChannel) => void;
    onClose: () => void;
  }

  let {
    layout,
    channels,
    totalCount,
    activeChannelId,
    focusedChannelId,
    filter,
    facet,
    facetLabel,
    onFilter,
    onFacet,
    onPick,
    onClose
  }: Props = $props();

  const timeFmt = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' });

  const facetOptions = [
    { value: 'favourites', label: 'Favourites' },
    { value: 'hd', label: 'HD' },
    { value: 'radio', label: 'Radio' }
  ];

  type Row = {
    id: string;
    lcn: string;
    name: string;
    tint: string;
    nowTitle: string;
    times: string;
    pct: number;
    leftLine: string;
    logo: string | null;
    channel: LivetvChannel;
  };

  const rows = $derived.by<Row[]>(() =>
    channels.map((channel) => {
      const slot = channel.now;
      const entry = slot?.now ?? null;
      const minutesLeft = slot?.minutes_left ?? null;
      return {
        id: channel.id,
        lcn: channel.lcn?.toString() ?? '—',
        name: channel.name,
        tint: networkTint(channel.network),
        nowTitle: entry?.title ?? 'No guide data',
        times: entry
          ? `${timeFmt.format(new Date(entry.start_time))} – ${timeFmt.format(new Date(entry.end_time))}`
          : '',
        pct: Math.min(100, Math.max(0, entry?.progress_pct ?? 0)),
        leftLine: minutesLeft != null && minutesLeft >= 0 ? `${minutesLeft} min` : '',
        logo: channel.logo_url,
        channel
      };
    })
  );
</script>

<aside class="panel" class:strip={layout === 'strip'} aria-label="Channels">
  <div class="head">
    <div class="head-line">
      <span class="head-title">Channels</span>
      <span class="head-count">
        {#if layout === 'strip'}{facetLabel} ·
        {/if}{rows.length} of {totalCount}
        {#if layout === 'rail'}shown{/if}
      </span>
      <span class="grow"></span>
      {#if layout === 'strip'}
        <span class="hint">&larr; &rarr; to browse &middot; Enter to switch</span>
      {/if}
      <button type="button" class="close" onclick={onClose} aria-label="Hide the channel list">
        <span class="material-icons-outlined">close</span>
      </button>
    </div>

    {#if layout === 'rail'}
      <label class="filter">
        <span class="material-icons-outlined">search</span>
        <input
          type="search"
          placeholder="Filter"
          value={filter}
          oninput={(e) => onFilter(e.currentTarget.value)}
        />
      </label>
      <FacetPills options={facetOptions} value={facet} onChange={onFacet} />
      <span class="hint">&uarr; &darr; to browse &middot; Enter to switch</span>
    {/if}
  </div>

  <div class="list">
    {#each rows as row (row.id)}
      {@const active = row.id === activeChannelId}
      {@const focused = row.id === focusedChannelId}
      <button
        type="button"
        class="row"
        class:active
        class:focused
        data-channel-row={row.id}
        onclick={() => onPick(row.channel)}
      >
        {#if layout === 'rail'}
          <span class="lcn">{row.lcn}</span>
          {#if row.logo}
            <img class="logo" src={row.logo} alt="" loading="lazy" />
          {:else}
            <span class="tint" style:background={row.tint}></span>
          {/if}
          <span class="body">
            <span class="name">{row.name}</span>
            <span class="now">{row.nowTitle}</span>
            <span class="bar"
              ><span class="bar-fill" style:width="{row.pct}%" style:background={row.tint}
              ></span></span
            >
          </span>
          <span class="left">{row.leftLine}</span>
        {:else}
          <span class="card-head">
            <span class="lcn">{row.lcn}</span>
            {#if row.logo}
              <img class="logo short" src={row.logo} alt="" loading="lazy" />
            {:else}
              <span class="tint short" style:background={row.tint}></span>
            {/if}
            <span class="name">{row.name}</span>
          </span>
          <span class="now strong">{row.nowTitle}</span>
          <span class="times">{row.times}</span>
          <span class="bar"
            ><span class="bar-fill" style:width="{row.pct}%" style:background={row.tint}
            ></span></span
          >
        {/if}
      </button>
    {/each}

    {#if rows.length === 0}
      <p class="empty">No channels match this filter.</p>
    {/if}
  </div>
</aside>

<style>
  .panel {
    position: absolute;
    top: 0;
    right: 0;
    bottom: 0;
    width: 372px;
    box-sizing: border-box;
    background: rgba(15, 17, 21, 0.94);
    border-left: 1px solid var(--ring);
    display: flex;
    flex-direction: column;
    z-index: 4;
    backdrop-filter: blur(6px);
  }

  :global([data-theme='light']) .panel {
    background: rgba(248, 247, 245, 0.94);
  }

  .panel.strip {
    top: auto;
    left: 0;
    right: 0;
    width: auto;
    height: 186px;
    border-left: none;
    border-top: 1px solid var(--ring);
    padding: 14px 24px;
  }

  .head {
    padding: 18px 18px 12px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    border-bottom: 1px solid var(--ring);
    flex-shrink: 0;
  }

  .panel.strip .head {
    padding: 0 0 11px;
    border-bottom: none;
  }

  .head-line {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .head-title {
    font-size: 15px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .panel.strip .head-title {
    font-size: 13px;
  }

  .head-count,
  .hint {
    font-size: 11px;
    color: var(--muted);
  }

  .grow {
    flex-grow: 1;
  }

  .close {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border: none;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    border-radius: 6px;
  }

  .close:hover {
    color: var(--ink);
    background: rgba(255, 255, 255, 0.08);
  }

  .close .material-icons-outlined {
    font-size: 17px;
  }

  .filter {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 32px;
    padding: 0 11px;
    border: 1px solid var(--ring);
    border-radius: 9px;
    background: var(--card);
    color: var(--muted);
  }

  .filter input {
    background: none;
    border: none;
    outline: none;
    color: var(--ink);
    font-size: 12px;
    width: 100%;
  }

  .filter .material-icons-outlined {
    font-size: 15px;
  }

  .list {
    flex-grow: 1;
    overflow-y: auto;
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }

  .panel.strip .list {
    flex-direction: row;
    overflow-x: auto;
    overflow-y: hidden;
    padding: 0;
    gap: 9px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    border-radius: 10px;
    box-sizing: border-box;
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid var(--ring);
    color: var(--ink);
    text-align: left;
    cursor: pointer;
    flex-shrink: 0;
    font: inherit;
  }

  :global([data-theme='light']) .row {
    background: rgba(0, 0, 0, 0.03);
  }

  .panel.strip .row {
    width: 186px;
    flex-direction: column;
    align-items: stretch;
    gap: 7px;
    padding: 10px 11px;
    border-radius: 11px;
  }

  .row:hover {
    border-color: var(--muted);
  }

  .row.focused {
    border-color: var(--accent);
  }

  .row.active {
    background: var(--accent-dim);
    border-color: var(--accent);
  }

  .lcn {
    width: 20px;
    flex-shrink: 0;
    font-size: 11px;
    font-weight: 700;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
    text-align: right;
  }

  .panel.strip .lcn {
    width: auto;
    text-align: left;
  }

  .tint {
    width: 3px;
    height: 30px;
    border-radius: 2px;
    flex-shrink: 0;
  }

  .tint.short {
    height: 13px;
  }

  .logo {
    width: 30px;
    height: 30px;
    object-fit: contain;
    border-radius: 5px;
    flex-shrink: 0;
  }

  .logo.short {
    width: 18px;
    height: 18px;
  }

  .card-head {
    display: flex;
    align-items: center;
    gap: 7px;
    min-width: 0;
  }

  .body {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex-grow: 1;
  }

  .name {
    font-size: 12.5px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .now {
    font-size: 11px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .now.strong {
    font-size: 11.5px;
    color: var(--ink);
  }

  .times {
    font-size: 10.5px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .bar {
    display: block;
    height: 2px;
    border-radius: 1px;
    background: rgba(255, 255, 255, 0.1);
    margin-top: 2px;
  }

  :global([data-theme='light']) .bar {
    background: rgba(0, 0, 0, 0.12);
  }

  .bar-fill {
    display: block;
    height: 2px;
    border-radius: 1px;
  }

  .left {
    font-size: 10.5px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
    flex-shrink: 0;
  }

  .empty {
    margin: 12px 4px;
    font-size: 12px;
    color: var(--muted);
  }
</style>
