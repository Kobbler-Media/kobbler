<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import { onReconnect, offReconnect } from '$lib/services/eventstream.svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import * as api from '$lib/api/client';
  import type { IptvChannel, IptvWatchMatch, IptvEpgEntry } from '$lib/types';

  type WatchHistoryEntry = { channel_id: string; watch_count: number; last_watched_at: string };
  let watchMatches = $state<IptvWatchMatch[]>([]);
  let favourites = $state<IptvChannel[]>([]);
  let historyChannels = $state<IptvChannel[]>([]);
  type EpgGridChannel = IptvChannel & { has_catchup?: boolean; catchup_days?: number };
  let catchupPrograms = $state<Array<{ channel: EpgGridChannel; programs: IptvEpgEntry[] }>>([]);
  let loading = $state(true);
  let recordingInProgress = $state<Set<string>>(new Set());

  let reconnectHandler: (() => void) | null = null;
  let refreshTimer: ReturnType<typeof setInterval> | undefined;

  const liveMatches = $derived(watchMatches.filter((m) => m.is_live));
  const upcomingMatches = $derived(watchMatches.filter((m) => !m.is_live));

  const catchupEntries = $derived.by(() => {
    const now = new Date();
    const entries: Array<{
      channel_name: string;
      channel_id: string;
      program: IptvEpgEntry;
      day: string;
    }> = [];
    for (const { channel, programs } of catchupPrograms) {
      if (!channel.has_catchup) continue;
      for (const p of programs) {
        const stop = new Date(p.stop_time);
        if (stop < now) {
          entries.push({
            channel_name: channel.name,
            channel_id: channel.id,
            program: p,
            day: stop.toLocaleDateString(undefined, {
              weekday: 'long',
              month: 'short',
              day: 'numeric'
            })
          });
        }
      }
    }
    entries.sort(
      (a, b) => new Date(b.program.stop_time).getTime() - new Date(a.program.stop_time).getTime()
    );
    return entries.slice(0, 20);
  });

  const catchupByDay = $derived.by(() => {
    const groups = new Map<string, typeof catchupEntries>();
    for (const entry of catchupEntries) {
      const existing = groups.get(entry.day);
      if (existing) {
        existing.push(entry);
      } else {
        groups.set(entry.day, [entry]);
      }
    }
    return groups;
  });

  function formatTimeRemaining(stopTime: string): string {
    const now = Date.now();
    const end = new Date(stopTime).getTime();
    const diff = end - now;
    if (diff <= 0) return 'Ending';
    const mins = Math.floor(diff / 60000);
    if (mins < 60) return `${mins}m remaining`;
    const hrs = Math.floor(mins / 60);
    const rm = mins % 60;
    return rm > 0 ? `${hrs}h ${rm}m remaining` : `${hrs}h remaining`;
  }

  function formatStartsIn(startTime: string): string {
    const now = Date.now();
    const start = new Date(startTime).getTime();
    const diff = start - now;
    if (diff <= 0) return 'Starting now';
    const mins = Math.floor(diff / 60000);
    if (mins < 60) return `Starts in ${mins}m`;
    const hrs = Math.floor(mins / 60);
    const rm = mins % 60;
    return rm > 0 ? `Starts in ${hrs}h ${rm}m` : `Starts in ${hrs}h`;
  }

  function formatTime(iso: string): string {
    return new Date(iso).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  }

  function timeProgress(startTime: string, stopTime: string): number {
    const now = Date.now();
    const start = new Date(startTime).getTime();
    const end = new Date(stopTime).getTime();
    const total = end - start;
    if (total <= 0) return 100;
    const elapsed = now - start;
    return Math.max(0, Math.min(100, (elapsed / total) * 100));
  }

  async function loadWatchMatches() {
    try {
      watchMatches = await api.getIptvWatchMatches();
    } catch {
      watchMatches = [];
    }
  }

  async function loadFavourites() {
    try {
      favourites = await api.getIptvFavourites();
    } catch {
      favourites = [];
    }
  }

  async function loadHistory() {
    try {
      const entries = (await api.getIptvHistory()) as unknown as WatchHistoryEntry[];
      // Fetch actual channel details for history entries
      if (entries.length > 0) {
        const channelIds = entries.map((e) => e.channel_id);
        const result = await api.getIptvChannels({ limit: channelIds.length });
        // Filter to only channels in history, maintain history order
        const channelMap = new Map(result.items.map((c) => [c.id, c]));
        historyChannels = channelIds
          .map((id) => channelMap.get(id))
          .filter((c): c is IptvChannel => c != null);
      } else {
        historyChannels = [];
      }
    } catch {
      historyChannels = [];
    }
  }

  async function loadCatchup() {
    try {
      const now = new Date();
      const from = new Date(now.getTime() - 7 * 24 * 60 * 60 * 1000);
      const result = await api.getIptvEpgFavourites({
        from: from.toISOString(),
        to: now.toISOString()
      });
      catchupPrograms = result.grid;
    } catch {
      catchupPrograms = [];
    }
  }

  async function loadAll() {
    loading = true;
    await Promise.all([loadWatchMatches(), loadFavourites(), loadHistory(), loadCatchup()]);
    loading = false;
  }

  async function handleRecord(match: IptvWatchMatch) {
    const key = match.channel_id + match.start_time;
    if (recordingInProgress.has(key)) return;
    recordingInProgress = new Set([...recordingInProgress, key]);
    try {
      await api.createIptvRecording({
        channel_id: match.channel_id,
        title: match.epg_title,
        start_time: match.start_time,
        end_time: match.stop_time
      });
      showMessage('Recording scheduled');
    } catch {
      showMessage('Failed to schedule recording');
    } finally {
      const next = new Set(recordingInProgress);
      next.delete(key);
      recordingInProgress = next;
    }
  }

  async function handleRecordCatchup(entry: (typeof catchupEntries)[0]) {
    const key = entry.channel_id + entry.program.start_time;
    if (recordingInProgress.has(key)) return;
    recordingInProgress = new Set([...recordingInProgress, key]);
    try {
      await api.createIptvRecording({
        channel_id: entry.channel_id,
        title: entry.program.title,
        start_time: entry.program.start_time,
        end_time: entry.program.stop_time
      });
      showMessage('Recording scheduled');
    } catch {
      showMessage('Failed to schedule recording');
    } finally {
      const next = new Set(recordingInProgress);
      next.delete(key);
      recordingInProgress = next;
    }
  }

  onMount(() => {
    loadAll();

    refreshTimer = setInterval(() => {
      loadWatchMatches();
    }, 60_000);

    reconnectHandler = onReconnect(() => {
      loadAll();
    });
  });

  onDestroy(() => {
    if (reconnectHandler) offReconnect(reconnectHandler);
    clearInterval(refreshTimer);
  });
</script>

<svelte:head>
  <title>Live TV — Kobbler</title>
</svelte:head>

<div class="live-home page-container">
  <div class="page-header">
    <h1>Live TV</h1>
    <div class="header-actions">
      <a href="/iptv/channels" class="header-link">
        <span class="material-icons-outlined" style="font-size: 16px;">grid_view</span>
        Channels
      </a>
      <a href="/iptv/guide" class="header-link">
        <span class="material-icons-outlined" style="font-size: 16px;">calendar_month</span>
        Guide
      </a>
      <a href="/iptv/recordings" class="header-link">
        <span class="material-icons-outlined" style="font-size: 16px;">fiber_manual_record</span>
        Recordings
      </a>
    </div>
  </div>

  {#if loading}
    <div class="skeleton-sections">
      {#each [1, 2, 3] as s (s)}
        <section class="section">
          <div class="skeleton-title"></div>
          <div class="skeleton-row">
            {#each [1, 2, 3, 4] as c (c)}
              <div class="skeleton-card"></div>
            {/each}
          </div>
        </section>
      {/each}
    </div>
  {:else}
    <!-- On Now -->
    {#if liveMatches.length > 0}
      <section class="section">
        <h2 class="section-title">
          <span class="live-indicator"></span>
          On Now
          <span class="section-count">{liveMatches.length}</span>
        </h2>
        <div class="card-grid">
          {#each liveMatches as match, i (match.channel_id + match.start_time + i)}
            <div class="now-card">
              <div class="now-card-header">
                {#if match.channel_icon_url}
                  <img src={match.channel_icon_url} alt="" class="channel-icon" />
                {:else}
                  <div class="channel-icon-placeholder">
                    <span class="material-icons-outlined" style="font-size: 18px;">tv</span>
                  </div>
                {/if}
                <span class="channel-name">{match.channel_name}</span>
              </div>
              <div class="now-card-body">
                <span class="programme-title">{match.epg_title}</span>
                <span class="time-remaining">{formatTimeRemaining(match.stop_time)}</span>
                <div class="progress-track">
                  <div
                    class="progress-fill"
                    style="width: {timeProgress(match.start_time, match.stop_time)}%"
                  ></div>
                </div>
              </div>
              <div class="now-card-actions">
                <button
                  class="btn-watch"
                  type="button"
                  onclick={() => goto(`/iptv/${match.channel_id}`)}>Watch</button
                >
                <button
                  class="btn-record"
                  type="button"
                  disabled={recordingInProgress.has(match.channel_id + match.start_time)}
                  onclick={() => handleRecord(match)}>Record</button
                >
              </div>
            </div>
          {/each}
        </div>
      </section>
    {/if}

    <!-- Coming Up -->
    {#if upcomingMatches.length > 0}
      <section class="section">
        <h2 class="section-title">
          Coming Up
          <span class="section-count">{upcomingMatches.length}</span>
        </h2>
        <div class="card-grid">
          {#each upcomingMatches as match, i (match.channel_id + match.start_time + i)}
            <div class="upcoming-card">
              <div class="now-card-header">
                {#if match.channel_icon_url}
                  <img src={match.channel_icon_url} alt="" class="channel-icon" />
                {:else}
                  <div class="channel-icon-placeholder">
                    <span class="material-icons-outlined" style="font-size: 18px;">tv</span>
                  </div>
                {/if}
                <span class="channel-name">{match.channel_name}</span>
              </div>
              <div class="now-card-body">
                <span class="programme-title">{match.epg_title}</span>
                <span class="start-time"
                  >{formatStartsIn(match.start_time)} — {formatTime(match.start_time)}</span
                >
              </div>
              <div class="now-card-actions">
                <button
                  class="btn-record primary"
                  type="button"
                  disabled={recordingInProgress.has(match.channel_id + match.start_time)}
                  onclick={() => handleRecord(match)}>Record</button
                >
                <button
                  class="btn-watch secondary"
                  type="button"
                  onclick={() => goto(`/iptv/${match.channel_id}`)}>Channel</button
                >
              </div>
            </div>
          {/each}
        </div>
      </section>
    {/if}

    <!-- No watches configured hint -->
    {#if liveMatches.length === 0 && upcomingMatches.length === 0}
      <section class="section">
        <div class="empty-hint">
          <span class="material-icons-outlined" style="font-size: 28px; color: var(--muted);"
            >notifications_none</span
          >
          <p>
            No watch alerts configured. Set up EPG watches in <a href="/settings/iptv"
              >IPTV Settings</a
            > to see live and upcoming matches here.
          </p>
        </div>
      </section>
    {/if}

    <!-- My Channels -->
    {#if favourites.length > 0}
      <section class="section">
        <h2 class="section-title">
          My Channels
          <span class="section-count">{favourites.length}</span>
        </h2>
        <div class="horizontal-scroll">
          {#each favourites as channel (channel.id)}
            <button
              class="favourite-card"
              type="button"
              onclick={() => goto(`/iptv/${channel.id}`)}
            >
              {#if channel.icon_url}
                <img src={channel.icon_url} alt="" class="fav-icon" />
              {:else}
                <div class="fav-icon-placeholder">
                  <span class="material-icons-outlined" style="font-size: 24px;">tv</span>
                </div>
              {/if}
              <span class="fav-name">{channel.name}</span>
              <span class="fav-program">{channel.current_program || 'No programme info'}</span>
            </button>
          {/each}
        </div>
      </section>
    {:else}
      <section class="section">
        <h2 class="section-title">My Channels</h2>
        <div class="empty-hint">
          <span class="material-icons-outlined" style="font-size: 28px; color: var(--muted);"
            >favorite_border</span
          >
          <p>
            No favourite channels yet. Browse <a href="/iptv/channels">Channels</a> and favourite the
            ones you watch regularly.
          </p>
        </div>
      </section>
    {/if}

    <!-- Catch Up -->
    {#if catchupEntries.length > 0}
      <section class="section">
        <h2 class="section-title">
          Catch Up
          <span class="section-count">{catchupEntries.length}</span>
        </h2>
        {#each catchupByDay as [day, entries] (day)}
          <h3 class="day-header">{day}</h3>
          <div class="catchup-list">
            {#each entries as entry, i (entry.channel_id + entry.program.id + i)}
              <div class="catchup-entry">
                <div class="catchup-info">
                  <span class="catchup-channel">{entry.channel_name}</span>
                  <span class="catchup-title">{entry.program.title}</span>
                  <span class="catchup-time"
                    >{formatTime(entry.program.start_time)} — {formatTime(
                      entry.program.stop_time
                    )}</span
                  >
                </div>
                <div class="catchup-actions">
                  <button
                    class="btn-watch small"
                    type="button"
                    onclick={() => goto(`/iptv/${entry.channel_id}?catchup=${entry.program.id}`)}
                    >Watch</button
                  >
                  <button
                    class="btn-record small"
                    type="button"
                    disabled={recordingInProgress.has(entry.channel_id + entry.program.start_time)}
                    onclick={() => handleRecordCatchup(entry)}>Record</button
                  >
                </div>
              </div>
            {/each}
          </div>
        {/each}
      </section>
    {/if}

    <!-- Continue Watching -->
    {#if historyChannels.length > 0}
      <section class="section">
        <h2 class="section-title">Continue Watching</h2>
        <div class="horizontal-scroll">
          {#each historyChannels.slice(0, 12) as channel (channel.id)}
            <button
              class="favourite-card"
              type="button"
              onclick={() => goto(`/iptv/${channel.id}`)}
            >
              {#if channel.icon_url}
                <img src={channel.icon_url} alt="" class="fav-icon" />
              {:else}
                <div class="fav-icon-placeholder">
                  <span class="material-icons-outlined" style="font-size: 24px;">tv</span>
                </div>
              {/if}
              <span class="fav-name">{channel.name}</span>
              <span class="fav-program">{channel.current_program || 'No programme info'}</span>
            </button>
          {/each}
        </div>
      </section>
    {/if}
  {/if}
</div>

<style>
  .live-home {
    padding-bottom: 80px;
  }

  .page-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 24px;
    gap: 12px;
    flex-wrap: wrap;
  }

  .page-header h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 700;
    color: var(--ink);
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .header-link {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 6px 12px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--muted);
    font-size: 12px;
    font-weight: 500;
    text-decoration: none;
    transition: all 0.15s;
  }

  .header-link:hover {
    background: var(--bg-2);
    color: var(--ink);
    border-color: var(--accent);
  }

  /* Sections */
  .section {
    margin-bottom: 28px;
  }

  .section-title {
    font-size: 15px;
    font-weight: 600;
    margin: 0 0 12px;
    color: var(--ink);
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .section-count {
    font-size: 11px;
    font-weight: 500;
    color: var(--muted);
    background: var(--bg-2);
    padding: 1px 7px;
    border-radius: 10px;
  }

  .live-indicator {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #ef4444;
    flex-shrink: 0;
    animation: pulse 2s ease-in-out infinite;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.4;
    }
  }

  /* Card grid for On Now / Coming Up */
  .card-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 10px;
  }

  .now-card,
  .upcoming-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 10px;
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    transition:
      border-color 0.15s,
      box-shadow 0.15s;
  }

  .now-card:hover,
  .upcoming-card:hover {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--accent) 15%, transparent);
  }

  .now-card {
    border-left: 3px solid #ef4444;
  }

  .now-card-header {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .channel-icon {
    width: 28px;
    height: 28px;
    border-radius: 6px;
    object-fit: contain;
    background: var(--bg);
    flex-shrink: 0;
  }

  .channel-icon-placeholder {
    width: 28px;
    height: 28px;
    border-radius: 6px;
    background: var(--bg-2);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--muted);
    flex-shrink: 0;
  }

  .channel-name {
    font-size: 12px;
    font-weight: 500;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .now-card-body {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .programme-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .time-remaining {
    font-size: 11px;
    color: var(--muted);
  }

  .start-time {
    font-size: 11px;
    color: var(--accent);
    font-weight: 500;
  }

  .progress-track {
    height: 3px;
    background: var(--bg-2);
    border-radius: 2px;
    overflow: hidden;
    margin-top: 2px;
  }

  .progress-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 2px;
    transition: width 1s linear;
  }

  .now-card-actions {
    display: flex;
    gap: 6px;
    margin-top: 2px;
  }

  .btn-watch {
    flex: 1;
    padding: 7px 0;
    border: none;
    border-radius: 6px;
    background: var(--accent);
    color: var(--bg);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: opacity 0.15s;
  }

  .btn-watch:hover {
    opacity: 0.85;
  }

  .btn-watch.secondary {
    background: var(--bg-2);
    color: var(--ink);
    border: 1px solid var(--ring);
    flex: 0;
    padding: 7px 14px;
  }

  .btn-watch.secondary:hover {
    border-color: var(--accent);
    opacity: 1;
  }

  .btn-watch.small,
  .btn-record.small {
    flex: 0;
    padding: 5px 12px;
    font-size: 11px;
  }

  .btn-record {
    flex: 0;
    padding: 7px 14px;
    border: 1px solid rgba(239, 68, 68, 0.3);
    border-radius: 6px;
    background: rgba(239, 68, 68, 0.08);
    color: #ef4444;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
  }

  .btn-record:hover {
    background: rgba(239, 68, 68, 0.15);
    border-color: rgba(239, 68, 68, 0.5);
  }

  .btn-record:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .btn-record.primary {
    flex: 1;
    padding: 7px 0;
  }

  /* Horizontal scroll rows */
  .horizontal-scroll {
    display: flex;
    gap: 10px;
    overflow-x: auto;
    padding-bottom: 6px;
    scroll-snap-type: x mandatory;
    scrollbar-width: thin;
  }

  .favourite-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 14px 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 10px;
    cursor: pointer;
    transition: all 0.15s;
    flex-shrink: 0;
    min-width: 140px;
    max-width: 160px;
    text-align: center;
    scroll-snap-align: start;
    color: var(--ink);
    font: inherit;
  }

  .favourite-card:hover {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--accent) 15%, transparent);
  }

  .fav-icon {
    width: 40px;
    height: 40px;
    border-radius: 8px;
    object-fit: contain;
    background: var(--bg);
  }

  .fav-icon-placeholder {
    width: 40px;
    height: 40px;
    border-radius: 8px;
    background: var(--bg-2);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--muted);
  }

  .fav-name {
    font-size: 12px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
  }

  .fav-program {
    font-size: 11px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
  }

  /* Catch Up */
  .day-header {
    font-size: 12px;
    font-weight: 600;
    color: var(--muted);
    margin: 14px 0 6px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .day-header:first-of-type {
    margin-top: 0;
  }

  .catchup-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .catchup-entry {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 12px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 8px;
    transition: border-color 0.15s;
  }

  .catchup-entry:hover {
    border-color: var(--accent);
  }

  .catchup-info {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    flex: 1;
  }

  .catchup-channel {
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    flex-shrink: 0;
    min-width: 80px;
  }

  .catchup-title {
    font-size: 13px;
    font-weight: 500;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 1;
    min-width: 0;
  }

  .catchup-time {
    font-size: 11px;
    color: var(--muted);
    flex-shrink: 0;
  }

  .catchup-actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
  }

  /* Empty hints */
  .empty-hint {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 16px 20px;
    background: var(--card);
    border: 1px dashed var(--ring);
    border-radius: 10px;
  }

  .empty-hint p {
    margin: 0;
    font-size: 13px;
    color: var(--muted);
    line-height: 1.4;
  }

  .empty-hint a {
    color: var(--accent);
    text-decoration: none;
    font-weight: 500;
  }

  .empty-hint a:hover {
    text-decoration: underline;
  }

  /* Skeletons */
  .skeleton-sections {
    display: flex;
    flex-direction: column;
    gap: 28px;
  }

  .skeleton-title {
    width: 120px;
    height: 16px;
    background: var(--bg-2);
    border-radius: 4px;
    margin-bottom: 12px;
    animation: shimmer 1.5s ease-in-out infinite;
  }

  .skeleton-row {
    display: flex;
    gap: 10px;
    overflow: hidden;
  }

  .skeleton-card {
    width: 280px;
    height: 120px;
    background: var(--bg-2);
    border-radius: 10px;
    flex-shrink: 0;
    animation: shimmer 1.5s ease-in-out infinite;
  }

  @keyframes shimmer {
    0%,
    100% {
      opacity: 0.5;
    }
    50% {
      opacity: 0.3;
    }
  }

  @media (max-width: 768px) {
    .page-header {
      flex-direction: column;
      align-items: flex-start;
    }

    .card-grid {
      grid-template-columns: 1fr;
    }

    .catchup-info {
      flex-direction: column;
      align-items: flex-start;
      gap: 2px;
    }

    .catchup-channel {
      min-width: 0;
    }

    .catchup-entry {
      flex-direction: column;
      align-items: flex-start;
      gap: 8px;
    }

    .catchup-actions {
      align-self: flex-end;
    }
  }
</style>
