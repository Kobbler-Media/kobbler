<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { uid } from '@koblibs/ui';
  import { registerOverlay, unregisterOverlay } from '$lib/stores/ui.svelte';
  import { on as onLiveEvent, off as offLiveEvent } from '$lib/services/eventstream.svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import * as api from '$lib/api/client';
  import type {
    LivetvBufferState,
    LivetvChannel,
    LivetvRecording,
    LivetvSession
  } from '$lib/types';
  import {
    claimLivetvSession,
    closeLivetvPlayer,
    setLivetvPlayerChannel
  } from '$lib/stores/livetvPlayer.svelte';
  import { getLivetvState, fetchLivetvLeases } from '$lib/stores/livetv.svelte';
  import { networkTint } from '../tint';
  import LivePlayerTopBar from './LivePlayerTopBar.svelte';
  import LiveProgrammeBar from './LiveProgrammeBar.svelte';
  import LiveSeekBar from './LiveSeekBar.svelte';
  import LivePlayerControls from './LivePlayerControls.svelte';
  import ChannelRail from './ChannelRail.svelte';
  import { preloadHls, createLiveHls, type LiveHlsHandle } from './liveHls';
  import { portal } from '$lib/utils/portal';

  interface Props {
    channel: LivetvChannel;
  }

  let { channel }: Props = $props();

  const livetv = getLivetvState();
  const overlayId = `livetv-player-${uid()}`;
  const timeFmt = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' });

  // Two segments of slack: the playhead is "live" while it sits inside the
  // holdback the player deliberately keeps.
  const LIVE_TOLERANCE_SECONDS = 6;
  const CONTROLS_HIDE_MS = 3500;

  let videoEl = $state<HTMLVideoElement>();
  let containerEl = $state<HTMLDivElement>();
  let hls: LiveHlsHandle | null = null;
  let destroyed = false;

  let session = $state<LivetvSession | null>(null);
  let fatalError = $state<string | null>(null);
  let awaitingPicture = $state(true);
  let slowTune = $state(false);
  let switching = $state(false);

  let playing = $state(false);
  let muted = $state(false);
  let isFullscreen = $state(false);
  let controlsVisible = $state(true);
  let panelOpen = $state(true);
  let viewportWidth = $state(typeof window !== 'undefined' ? window.innerWidth : 1440);

  let playhead = $state(0);
  let availableStart = $state(0);
  let liveEdge = $state(0);
  let buffer = $state<LivetvBufferState | null>(null);

  let channels = $state<LivetvChannel[]>([]);
  let totalChannels = $state(0);
  let railFilter = $state('');
  let railFacet = $state<string | null>('favourites');
  let focusedId = $state<string | null>(null);

  let recordBusy = $state(false);
  let activeRecording = $state<LivetvRecording | null>(null);

  let watchStartedAt = '';
  let watchedSecs = 0;

  let hideTimer: ReturnType<typeof setTimeout> | null = null;
  let slowTuneTimer: ReturnType<typeof setTimeout> | null = null;
  let tickTimer: ReturnType<typeof setInterval> | null = null;
  let heartbeatTimer: ReturnType<typeof setInterval> | null = null;
  let bufferTimer: ReturnType<typeof setInterval> | null = null;
  let guideTimer: ReturnType<typeof setInterval> | null = null;
  let eventHandlers: Array<[Parameters<typeof onLiveEvent>[0], (data: unknown) => void]> = [];

  const layout = $derived<'rail' | 'strip'>(viewportWidth < 1100 ? 'strip' : 'rail');

  const tint = $derived(networkTint(channel.network));
  const tunerCount = $derived(livetv.tunerCount || (session?.tuner_count ?? 0));
  const tunersInUse = $derived(
    livetv.leases.length > 0 ? livetv.leases.length : (session?.tuners_in_use ?? 0)
  );

  // Decision 3: a scheduled recording can take the tuner out from under a
  // viewer who is behind live. The buffer keeps serving, but there is nothing
  // to jump forward to until a lease frees up.
  const preempted = $derived.by(() => {
    if (!session || livetv.leases.length === 0) return false;
    return !livetv.leases.some((l) => l.session_id === session?.session_id);
  });

  const bufferStartMs = $derived(session ? new Date(session.buffer_start_time).getTime() : 0);
  const nowEntry = $derived(session?.now ?? null);
  const nextEntry = $derived(session?.next ?? null);

  function toBufferSeconds(iso: string): number {
    return (new Date(iso).getTime() - bufferStartMs) / 1000;
  }

  function toWallClock(bufferSeconds: number): Date {
    return new Date(bufferStartMs + bufferSeconds * 1000);
  }

  const programmeStart = $derived(nowEntry ? toBufferSeconds(nowEntry.start_time) : null);
  const programmeEnd = $derived(nowEntry ? toBufferSeconds(nowEntry.end_time) : null);

  const trackStart = $derived(programmeStart ?? 0);
  const trackEnd = $derived(programmeEnd ?? Math.max(liveEdge, 60));

  const behindSeconds = $derived(Math.max(0, liveEdge - playhead));
  const atLive = $derived(behindSeconds < LIVE_TOLERANCE_SECONDS);
  const behindLabel = $derived.by(() => {
    if (behindSeconds < LIVE_TOLERANCE_SECONDS) return '';
    if (behindSeconds < 90) return `${Math.round(behindSeconds)} sec behind`;
    return `${Math.round(behindSeconds / 60)} min behind`;
  });

  const canRestart = $derived(programmeStart !== null && programmeStart >= availableStart - 2);

  const programmeTimes = $derived(
    nowEntry
      ? `${timeFmt.format(new Date(nowEntry.start_time))} – ${timeFmt.format(new Date(nowEntry.end_time))}`
      : ''
  );

  const programmeSub = $derived([nowEntry?.episode_number, nowEntry?.genres?.[0]]);

  const nextLine = $derived(
    nextEntry ? `Next ${timeFmt.format(new Date(nextEntry.start_time))}   ${nextEntry.title}` : ''
  );

  const leftLine = $derived.by(() => {
    if (!programmeEnd) return '';
    const mins = Math.round((programmeEnd - liveEdge) / 60);
    if (mins <= 0) return 'ending now';
    return `${mins} min left`;
  });

  const metaLine = $derived.by(() => {
    if (!session) return '';
    const parts: string[] = [];
    if (session.height) parts.push(`${session.height}i25`);
    const codecs = [session.video_codec, session.audio_codec].filter(Boolean).join(' / ');
    if (codecs) parts.push(codecs.toUpperCase());
    return parts.join(' · ');
  });

  const bufferedTitle = $derived.by(() => {
    if (!session) return 'This channel is being written to disk as you watch.';
    const from = timeFmt.format(new Date(session.buffer_start_time));
    const cap = buffer?.cap_seconds ? ` · kept for ${Math.round(buffer.cap_seconds / 3600)} h` : '';
    return `On disk since ${from}${cap}. Record captures all of it, retroactively.`;
  });

  const recordTitle = $derived.by(() => {
    if (activeRecording) return `Recording — ${activeRecording.status}`;
    if (!session) return 'Record this channel';
    const from = timeFmt.format(new Date(session.buffer_start_time));
    return nowEntry
      ? `Record ${nowEntry.title} — everything buffered since ${from} is already in it`
      : `Record from ${from} — the buffer is already in it`;
  });

  const facetLabel = $derived(
    railFacet === 'favourites'
      ? 'Favourites'
      : railFacet === 'hd'
        ? 'HD'
        : railFacet === 'radio'
          ? 'Radio'
          : 'All'
  );

  const railChannels = $derived.by(() => {
    const query = railFilter.trim().toLowerCase();
    return channels.filter((c) => {
      if (railFacet === 'favourites' && !c.favourite) return false;
      if (railFacet === 'hd' && !c.hd) return false;
      if (railFacet === 'radio' && c.type_label !== 'Radio') return false;
      if (!query) return true;
      return `${c.lcn ?? ''} ${c.name} ${c.network ?? ''}`.toLowerCase().includes(query);
    });
  });

  async function attachPlaylist(next: LivetvSession) {
    if (!videoEl || destroyed) return;
    awaitingPicture = true;
    armSlowTune();
    if (next.playlist_url) {
      const Ctor = await preloadHls();
      if (destroyed || !videoEl) return;
      if (!Ctor.isSupported()) {
        if (videoEl.canPlayType('application/vnd.apple.mpegurl')) {
          videoEl.src = next.playlist_url;
          videoEl.play().catch(() => {});
          return;
        }
        fatalError = 'This browser cannot play the tuner stream.';
        return;
      }
      if (!hls) {
        hls = createLiveHls(Ctor, videoEl, {
          onFatal: (message) => {
            fatalError = message;
          }
        });
      }
      hls.load(next.playlist_url);
    } else if (next.direct_url) {
      videoEl.src = next.direct_url;
      videoEl.play().catch(() => {});
    } else {
      fatalError = 'The tuner session came back without a stream.';
    }
  }

  function armSlowTune() {
    slowTune = false;
    if (slowTuneTimer) clearTimeout(slowTuneTimer);
    slowTuneTimer = setTimeout(() => {
      slowTune = true;
    }, 2500);
  }

  function resetBufferState() {
    playhead = 0;
    availableStart = 0;
    liveEdge = 0;
    buffer = null;
    activeRecording = null;
  }

  function startWatchWindow() {
    watchStartedAt = new Date().toISOString();
    watchedSecs = 0;
  }

  function flushWatchEvent(
    channelId: string,
    guideId: string | null,
    programmeSecs: number | null
  ) {
    if (!watchStartedAt || watchedSecs < 5) return;
    const body = {
      channel_id: channelId,
      guide_id: guideId,
      started_at: watchStartedAt,
      ended_at: new Date().toISOString(),
      watched_secs: Math.round(watchedSecs),
      programme_secs: programmeSecs,
      source: 'web'
    };
    watchStartedAt = '';
    watchedSecs = 0;
    api.postLivetvWatchEvent(body).catch(() => {});
  }

  function flushCurrentWatchEvent() {
    if (!session) return;
    flushWatchEvent(session.channel_id, nowEntry?.id ?? null, nowEntry?.duration_seconds ?? null);
  }

  async function startSession() {
    try {
      const next = await claimLivetvSession(channel.id);
      if (destroyed) {
        api.destroyLivetvSession(next.session_id).catch(() => {});
        return;
      }
      session = next;
      focusedId = next.channel_id;
      startWatchWindow();
      await attachPlaylist(next);
      fetchLivetvLeases();
      pollBuffer();
    } catch (e) {
      fatalError =
        e instanceof Error ? e.message : 'Could not get a tuner for this channel right now.';
    }
  }

  async function switchTo(target: LivetvChannel) {
    if (!session || switching) return;
    if (target.id === session.channel_id) {
      panelOpen = false;
      return;
    }
    switching = true;
    flushCurrentWatchEvent();
    try {
      const next = await api.switchLivetvSessionChannel(session.session_id, target.id);
      if (destroyed) return;
      session = next;
      focusedId = target.id;
      setLivetvPlayerChannel(target);
      resetBufferState();
      startWatchWindow();
      await attachPlaylist(next);
      fetchLivetvLeases();
      pollBuffer();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Could not switch channel');
    } finally {
      switching = false;
    }
  }

  function seekTo(bufferSeconds: number) {
    if (!videoEl) return;
    const target = Math.min(liveEdge, Math.max(availableStart, bufferSeconds));
    videoEl.currentTime = target;
    playhead = target;
  }

  function jumpToLive() {
    if (preempted) return;
    sampleBuffer();
    seekTo(liveEdge);
  }

  function restart() {
    if (programmeStart === null) return;
    seekTo(Math.max(programmeStart, availableStart));
  }

  function togglePlayPause() {
    if (!videoEl) return;
    if (videoEl.paused) videoEl.play().catch(() => {});
    else videoEl.pause();
  }

  function toggleMute() {
    if (!videoEl) return;
    videoEl.muted = !videoEl.muted;
    muted = videoEl.muted;
  }

  function toggleFullscreen() {
    if (!containerEl) return;
    if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
    else containerEl.requestFullscreen().catch(() => {});
  }

  async function handleRecord() {
    if (!session || recordBusy) return;
    recordBusy = true;
    try {
      const recording = await api.recordLivetvSession(session.session_id, {
        mode: nowEntry ? 'programme' : 'open_ended',
        ...(nowEntry ? { guide_id: nowEntry.id } : {})
      });
      activeRecording = recording;
      const from = timeFmt.format(new Date(session.buffer_start_time));
      showMessage(`Recording ${recording.title} — the buffer back to ${from} is already in it`);
      fetchLivetvLeases();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Could not start the recording');
    } finally {
      recordBusy = false;
    }
  }

  function sampleBuffer() {
    const el = videoEl;
    if (!el) return;
    const ranges = el.seekable;
    if (ranges && ranges.length > 0) {
      availableStart = ranges.start(0);
      liveEdge = Math.max(ranges.end(ranges.length - 1), availableStart);
    } else if (buffer) {
      liveEdge = buffer.live_edge_seconds;
    }
    playhead = el.currentTime;
  }

  async function pollBuffer() {
    if (!session) return;
    try {
      const next = await api.getLivetvSessionBuffer(session.session_id);
      if (destroyed) return;
      buffer = next;
      if (liveEdge <= 0) liveEdge = next.live_edge_seconds;
    } catch {
      /* the buffer endpoint is advisory — seekable ranges drive the UI */
    }
  }

  async function loadChannels() {
    try {
      const result = await api.getLivetvChannels({
        enabled: true,
        with_now: true,
        limit: 500,
        sort: 'lcn'
      });
      if (destroyed) return;
      channels = result.items ?? [];
      totalChannels = result.total ?? channels.length;
      syncNowFromLineup();
    } catch {
      /* the rail degrades to whatever it already has */
    }
  }

  // The guide slot on the lineup is the freshest now/next we have; roll the
  // player's own programme metadata over when the programme changes under us.
  function syncNowFromLineup() {
    if (!session) return;
    const mine = channels.find((c) => c.id === session?.channel_id);
    const slot = mine?.now;
    if (!slot) return;
    if (slot.now?.id !== session.now?.id || slot.next?.id !== session.next?.id) {
      session = { ...session, now: slot.now, next: slot.next };
    }
  }

  function showControls() {
    controlsVisible = true;
    if (hideTimer) clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      if (!panelOpen) controlsVisible = false;
    }, CONTROLS_HIDE_MS);
  }

  function moveFocus(delta: number) {
    const list = railChannels;
    if (list.length === 0) return;
    const current = list.findIndex((c) => c.id === focusedId);
    const nextIndex = current < 0 ? 0 : (current + delta + list.length) % list.length;
    focusedId = list[nextIndex].id;
    document
      .querySelector(`[data-channel-row="${focusedId}"]`)
      ?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }

  function handleKeydown(e: KeyboardEvent) {
    const target = e.target as HTMLElement | null;
    if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA')) {
      if (e.key === 'Escape') target.blur();
      return;
    }
    showControls();
    const browseAxis = layout === 'strip' ? ['ArrowLeft', 'ArrowRight'] : ['ArrowUp', 'ArrowDown'];

    switch (e.key) {
      case 'Escape':
        e.preventDefault();
        if (panelOpen) panelOpen = false;
        else handleClose();
        return;
      case ' ':
      case 'k':
        e.preventDefault();
        togglePlayPause();
        return;
      case 'Enter':
        if (panelOpen && focusedId) {
          e.preventDefault();
          const target2 = railChannels.find((c) => c.id === focusedId);
          if (target2) switchTo(target2);
        }
        return;
      case 'l':
        e.preventDefault();
        jumpToLive();
        return;
      case 'r':
        e.preventDefault();
        restart();
        return;
      case 'c':
      case 'g':
        e.preventDefault();
        panelOpen = !panelOpen;
        return;
      case 'f':
        e.preventDefault();
        toggleFullscreen();
        return;
      case 'm':
        e.preventDefault();
        toggleMute();
        return;
    }

    if (panelOpen && browseAxis.includes(e.key)) {
      e.preventDefault();
      moveFocus(e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 1);
      return;
    }
    if (e.key === 'ArrowLeft') {
      e.preventDefault();
      seekTo(playhead - 10);
    } else if (e.key === 'ArrowRight') {
      e.preventDefault();
      seekTo(playhead + 10);
    }
  }

  function handleClose() {
    flushCurrentWatchEvent();
    closeLivetvPlayer();
  }

  onMount(() => {
    registerOverlay(overlayId, handleClose);
    startSession();
    loadChannels();
    armSlowTune();

    tickTimer = setInterval(() => {
      sampleBuffer();
      if (playing) watchedSecs += 0.5;
    }, 500);

    heartbeatTimer = setInterval(() => {
      if (!session) return;
      api
        .heartbeatLivetvSession(session.session_id, {
          position_seconds: Math.round(playhead),
          source: 'web'
        })
        .catch(() => {});
    }, 15_000);

    bufferTimer = setInterval(pollBuffer, 10_000);
    guideTimer = setInterval(loadChannels, 60_000);

    const leaseHandler = () => {
      fetchLivetvLeases();
    };
    const recordingHandler = (data: unknown) => {
      fetchLivetvLeases();
      const payload = data as { id?: string; status?: LivetvRecording['status'] } | null;
      if (payload?.id && payload.id === activeRecording?.id && payload.status) {
        activeRecording = { ...activeRecording, status: payload.status };
      }
    };
    const guideHandler = () => {
      loadChannels();
    };
    eventHandlers = [
      ['livetv_lease_changed', leaseHandler],
      ['livetv_recording_updated', recordingHandler],
      ['livetv_guide_synced', guideHandler]
    ];
    for (const [type, handler] of eventHandlers) onLiveEvent(type, handler);

    showControls();
  });

  onDestroy(() => {
    destroyed = true;
    unregisterOverlay(overlayId);
    for (const [type, handler] of eventHandlers) offLiveEvent(type, handler);
    if (hideTimer) clearTimeout(hideTimer);
    if (slowTuneTimer) clearTimeout(slowTuneTimer);
    if (tickTimer) clearInterval(tickTimer);
    if (heartbeatTimer) clearInterval(heartbeatTimer);
    if (bufferTimer) clearInterval(bufferTimer);
    if (guideTimer) clearInterval(guideTimer);
    flushCurrentWatchEvent();
    hls?.destroy();
    hls = null;
    if (session) api.destroyLivetvSession(session.session_id).catch(() => {});
    fetchLivetvLeases();
  });
</script>

<svelte:window
  onkeydown={handleKeydown}
  onresize={() => (viewportWidth = window.innerWidth)}
  onfullscreenchange={() => (isFullscreen = !!document.fullscreenElement)}
/>

<div
  class="live-overlay"
  class:panel-rail={panelOpen && layout === 'rail'}
  class:panel-strip={panelOpen && layout === 'strip'}
  use:portal
  bind:this={containerEl}
  onmousemove={showControls}
  role="dialog"
  aria-label="Live TV player"
  tabindex="-1"
>
  <div class="stage">
    <video
      bind:this={videoEl}
      class="video"
      playsinline
      autoplay
      onplay={() => (playing = true)}
      onpause={() => (playing = false)}
      ontimeupdate={sampleBuffer}
      onloadeddata={() => {
        awaitingPicture = false;
        slowTune = false;
      }}
      onvolumechange={() => (muted = videoEl?.muted ?? false)}
    ></video>

    {#if awaitingPicture && !fatalError}
      <div class="tuning">
        <span class="tuning-badge" style:border-color={tint} style:color={tint}>
          {channel.lcn ?? '—'}
        </span>
        <span class="tuning-name">{channel.name}</span>
        {#if slowTune}
          <span class="tuning-note">Waiting for a keyframe…</span>
        {/if}
      </div>
    {/if}

    {#if fatalError}
      <div class="fatal">
        <span class="material-icons-outlined">error_outline</span>
        <p>{fatalError}</p>
        <button type="button" class="btn-sec" onclick={handleClose}>Close</button>
      </div>
    {/if}

    {#if preempted}
      <div class="preempt">
        <span class="material-icons-outlined">fiber_manual_record</span>
        A recording took this tuner. You can keep watching the buffer, but there is no live edge to jump
        to until a tuner frees up.
      </div>
    {/if}

    <div class="chrome" class:hidden={!controlsVisible}>
      <LivePlayerTopBar
        lcn={channel.lcn?.toString() ?? '—'}
        name={channel.name}
        hdLabel={channel.hd ? 'HD' : 'SD'}
        {tint}
        meta={metaLine}
        {tunersInUse}
        {tunerCount}
        signalQuality={channel.signal_quality}
        logoUrl={channel.logo_url}
        onBack={handleClose}
      />

      <div class="bottom">
        <LiveProgrammeBar
          title={nowEntry?.title ?? channel.name}
          times={programmeTimes}
          sub={programmeSub}
          {nextLine}
          {leftLine}
          {bufferedTitle}
        />

        <LiveSeekBar
          {trackStart}
          {trackEnd}
          {availableStart}
          {liveEdge}
          {playhead}
          startLabel={timeFmt.format(toWallClock(trackStart))}
          endLabel={timeFmt.format(toWallClock(trackEnd))}
          {atLive}
          onSeek={seekTo}
          onInteraction={showControls}
        />

        <LivePlayerControls
          {playing}
          {atLive}
          {behindLabel}
          jumpDisabled={preempted}
          jumpDisabledReason="A recording holds the tuner — the buffer has stopped growing"
          {canRestart}
          recording={activeRecording !== null}
          {recordBusy}
          {recordTitle}
          {muted}
          {panelOpen}
          {isFullscreen}
          onPlayPause={togglePlayPause}
          onJumpToLive={jumpToLive}
          onRestart={restart}
          onRecord={handleRecord}
          onToggleMute={toggleMute}
          onTogglePanel={() => (panelOpen = !panelOpen)}
          onToggleFullscreen={toggleFullscreen}
        />
      </div>
    </div>
  </div>

  {#if panelOpen}
    <ChannelRail
      {layout}
      channels={railChannels}
      totalCount={totalChannels}
      activeChannelId={session?.channel_id ?? channel.id}
      focusedChannelId={focusedId}
      filter={railFilter}
      facet={railFacet}
      {facetLabel}
      onFilter={(v) => (railFilter = v)}
      onFacet={(v) => (railFacet = v)}
      onPick={switchTo}
      onClose={() => (panelOpen = false)}
    />
  {/if}

  {#if switching}
    <div class="switching" aria-live="polite">Switching…</div>
  {/if}
</div>

<style>
  .live-overlay {
    position: fixed;
    inset: -1px;
    background: #07080a;
    color: #f3f1ee;
    z-index: 200;
    overflow: hidden;
  }

  .stage {
    position: absolute;
    inset: 0;
    transition:
      right 0.18s ease,
      bottom 0.18s ease;
  }

  .panel-rail .stage {
    right: 372px;
  }

  .panel-strip .stage {
    bottom: 186px;
  }

  .video {
    width: 100%;
    height: 100%;
    object-fit: contain;
    background: #07080a;
    display: block;
  }

  .chrome {
    position: absolute;
    inset: 0;
    pointer-events: none;
    opacity: 1;
    transition: opacity 0.2s ease;
    background: linear-gradient(
      to bottom,
      rgba(0, 0, 0, 0.55) 0%,
      rgba(0, 0, 0, 0) 22%,
      rgba(0, 0, 0, 0) 52%,
      rgba(0, 0, 0, 0.82) 100%
    );
  }

  .chrome.hidden {
    opacity: 0;
  }

  .chrome :global(button),
  .chrome :global(input),
  .chrome :global(a) {
    pointer-events: auto;
  }

  .bottom {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    padding: 0 24px 22px;
    padding-bottom: calc(22px + env(safe-area-inset-bottom));
    display: flex;
    flex-direction: column;
    gap: 13px;
  }

  .tuning {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 14px;
    pointer-events: none;
  }

  .tuning-badge {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 96px;
    height: 96px;
    border-radius: 22px;
    border: 1px solid;
    background: rgba(0, 0, 0, 0.35);
    font-size: 34px;
    font-weight: 700;
    letter-spacing: -0.03em;
    font-variant-numeric: tabular-nums;
  }

  .tuning-name {
    font-size: 14px;
    font-weight: 600;
  }

  .tuning-note {
    font-size: 13px;
    font-weight: 500;
    color: #5f6874;
    letter-spacing: 0.02em;
  }

  .fatal {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    background: rgba(7, 8, 10, 0.9);
    z-index: 5;
  }

  .fatal .material-icons-outlined {
    font-size: 40px;
    color: var(--danger);
  }

  .fatal p {
    margin: 0;
    font-size: 14px;
    max-width: 420px;
    text-align: center;
  }

  .preempt {
    position: absolute;
    top: 74px;
    left: 24px;
    right: 24px;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 14px;
    border-radius: 11px;
    background: rgba(239, 68, 68, 0.16);
    border: 1px solid var(--danger);
    font-size: 12.5px;
    z-index: 4;
  }

  .preempt .material-icons-outlined {
    font-size: 14px;
    color: var(--danger);
  }

  .switching {
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    padding: 8px 16px;
    border-radius: 10px;
    background: rgba(0, 0, 0, 0.7);
    border: 1px solid rgba(255, 255, 255, 0.12);
    font-size: 12.5px;
    font-weight: 600;
    z-index: 6;
  }

  @media (max-width: 720px) {
    .bottom {
      padding: 0 14px 14px;
    }

    .preempt {
      left: 14px;
      right: 14px;
      top: 62px;
    }
  }
</style>
