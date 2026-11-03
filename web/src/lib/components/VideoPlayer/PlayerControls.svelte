<script lang="ts">
  import TrackPicker from './TrackPicker.svelte';
  import ChapterMenu from './ChapterMenu.svelte';
  import SubtitlePanel from './SubtitlePanel.svelte';
  import EpisodeListPanel from './EpisodeListPanel.svelte';
  import CastButton from '#lib/components/CastButton.svelte';
  import type {
    PlaybackSubtitleTrack,
    PlaybackAudioTrack,
    VideoChapter,
    SkipSegment,
    TvEpisode
  } from '#lib/types.js';
  import {
    parseVttThumbnails,
    findThumbnailAtTime,
    type ThumbnailEntry
  } from '#lib/utils/vttThumbnails.js';
  import { SEGMENT_COLORS, isVisibleSkipSegment, segmentTitle } from './segments';
  import { pollThumbnailsVtt } from '#lib/utils/vttPoll.js';

  interface Props {
    playing: boolean;
    currentTime: number;
    duration: number;
    buffered: number;
    volume: number;
    muted: boolean;
    isFullscreen: boolean;
    visible: boolean;
    subtitles: PlaybackSubtitleTrack[];
    activeSubtitleId: string | null;
    audioTracks: PlaybackAudioTrack[];
    activeAudioIndex: number | null;
    onPlayPause: () => void;
    onSeek: (time: number) => void;
    onVolumeChange: (volume: number) => void;
    onMuteToggle: () => void;
    onFullscreenToggle: () => void;
    onSubtitleSelect: (trackId: string | null) => void;
    onAudioTrackSelect: (streamIndex: number | null) => void;
    playbackSpeed: number;
    onSpeedChange: (speed: number) => void;
    isPip: boolean;
    onPipToggle: () => void;
    pipSupported: boolean;
    aspectMode: 'auto' | 'fit' | 'fill';
    onAspectModeCycle: () => void;
    onSearchSubtitles?: () => void;
    onClose: () => void;
    mediaTitle?: string;
    mediaSubtitle?: string;
    seasonEpisodes?: TvEpisode[];
    currentEpisodeId?: string;
    onSelectEpisode?: (episode: TvEpisode) => void;
    thumbnailsVttUrl: string | null;
    chapters: VideoChapter[];
    skipSegments: SkipSegment[];
    onEditSkipSegment?: (seg: SkipSegment) => void;
    isLive?: boolean;
    onSwitchStream?: () => void;
    keep?: { label: string; busy: boolean; saved: boolean; onKeep: () => void };
    isTouchDevice?: boolean;
    onInteraction?: () => void;
  }

  let {
    playing,
    currentTime,
    duration,
    buffered,
    volume,
    muted,
    isFullscreen,
    isLive = false,
    visible,
    subtitles,
    activeSubtitleId,
    audioTracks,
    activeAudioIndex,
    onPlayPause,
    onSeek,
    onVolumeChange,
    onMuteToggle,
    onFullscreenToggle,
    onSubtitleSelect,
    onAudioTrackSelect,
    playbackSpeed,
    onSpeedChange,
    isPip,
    onPipToggle,
    pipSupported,
    aspectMode,
    onAspectModeCycle,
    onSearchSubtitles,
    onClose,
    mediaTitle,
    mediaSubtitle,
    seasonEpisodes,
    currentEpisodeId,
    onSelectEpisode,
    thumbnailsVttUrl,
    chapters,
    skipSegments,
    onEditSkipSegment,
    onSwitchStream,
    keep,
    isTouchDevice = false,
    onInteraction
  }: Props = $props();

  const SPEED_OPTIONS = [0.5, 0.75, 1, 1.25, 1.5, 2];

  // Panel mutual exclusivity — only one dropdown open at a time
  type PanelId = 'chapters' | 'episodes' | 'audio' | 'subtitles' | 'speed';
  let activePanel = $state<PanelId | null>(null);

  function openPanel(id: PanelId) {
    activePanel = activePanel === id ? null : id;
  }

  let thumbnailEntries = $state<ThumbnailEntry[]>([]);

  $effect(() => {
    const url = thumbnailsVttUrl;
    if (!url) {
      thumbnailEntries = [];
      return;
    }
    return pollThumbnailsVtt(url, (vtt) => (thumbnailEntries = parseVttThumbnails(vtt, url)));
  });

  let seekHover = $state(false);
  let seekHoverTime = $state(0);
  let seekBarEl: HTMLDivElement | undefined = $state();
  let volumeExpanded = $state(false);

  // Scrub state: during drag, show the scrubbed position without firing server seeks.
  // The actual seek fires only on mouseup/touchend (the 'change' event).
  let scrubbing = $state(false);
  let scrubTime = $state(0);
  let seekValue = $derived(scrubbing ? scrubTime : currentTime);

  function formatTime(seconds: number): string {
    if (!isFinite(seconds) || seconds < 0) seconds = 0;
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    const s = Math.floor(seconds % 60);
    const pad = (n: number) => n.toString().padStart(2, '0');
    if (h > 0) {
      return `${pad(h)}:${pad(m)}:${pad(s)}`;
    }
    return `${pad(m)}:${pad(s)}`;
  }

  function handleSeekInput(e: Event) {
    const target = e.target as HTMLInputElement;
    scrubbing = true;
    scrubTime = parseFloat(target.value);
  }

  function handleSeekCommit() {
    scrubbing = false;
    onSeek(scrubTime);
  }

  function handleSeekBarHover(e: MouseEvent) {
    if (!seekBarEl) return;
    // Use the range input's bounds and account for thumb width so the
    // hover tooltip time matches what the range input would produce on click.
    const input = seekBarEl.querySelector('input[type=range]') as HTMLInputElement | null;
    const el = input ?? seekBarEl;
    const rect = el.getBoundingClientRect();
    // Range input tracks are inset by half the thumb width on each side.
    // CSS sets thumb to 14px wide, so inset is 7px.
    const thumbInset = input ? 7 : 0;
    const trackLeft = rect.left + thumbInset;
    const trackWidth = rect.width - thumbInset * 2;
    const fraction = Math.max(0, Math.min(1, (e.clientX - trackLeft) / trackWidth));
    seekHoverTime = fraction * duration;
    seekHover = true;
  }

  function handleSeekBarLeave() {
    seekHover = false;
  }

  function handleVolumeInput(e: Event) {
    const target = e.target as HTMLInputElement;
    onVolumeChange(parseFloat(target.value));
  }

  let audioTrackItems = $derived(
    audioTracks.map((a, i) => ({
      id: `${i}-${a.stream_index}`,
      label:
        [a.title, a.language, a.channels > 0 ? `${a.channels}ch` : null, a.codec]
          .filter(Boolean)
          .join(' \u2013 ') || `Track ${a.stream_index}`,
      active: a.stream_index === activeAudioIndex
    }))
  );

  let hoverThumbnail = $derived(
    seekHover ? findThumbnailAtTime(thumbnailEntries, seekHoverTime) : null
  );

  let scrubThumbnail = $derived(
    scrubbing ? findThumbnailAtTime(thumbnailEntries, scrubTime) : null
  );
  let showTooltip = $derived(seekHover || scrubbing);
  let tooltipTime = $derived(scrubbing ? scrubTime : seekHoverTime);
  let tooltipThumbnail = $derived(scrubbing ? scrubThumbnail : hoverThumbnail);
  let tooltipPercent = $derived(duration > 0 ? (tooltipTime / duration) * 100 : 0);
  // Shift translateX from 0% at left edge to -100% at right edge to prevent overflow
  let tooltipTranslateX = $derived(-tooltipPercent);

  let progressPercent = $derived(duration > 0 ? (seekValue / duration) * 100 : 0);
  let bufferedPercent = $derived(duration > 0 ? (buffered / duration) * 100 : 0);

  let volumeIcon = $derived(
    muted || volume === 0 ? 'volume_off' : volume < 0.5 ? 'volume_down' : 'volume_up'
  );
</script>

<!--
  This is a surface "wake up" overlay for the video player — tapping/clicking
  anywhere reveals the controls. The focusable child buttons handle keyboard
  nav, so this outer layer has no keyboard affordance of its own.
-->
<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="controls-overlay" class:visible ontouchstart={onInteraction} onclick={onInteraction}>
  <!-- Media info (top left) -->
  {#if mediaTitle}
    <div class="media-info">
      <span class="media-title">{mediaTitle}</span>
      {#if mediaSubtitle}
        <span class="media-subtitle">{mediaSubtitle}</span>
      {/if}
    </div>
  {/if}

  <!-- Close button (top right) -->
  <button class="close-btn" type="button" onclick={onClose} aria-label="Close player">
    <span class="material-icons-outlined">close</span>
  </button>

  <!-- Center play/pause (mobile) -->
  {#if isTouchDevice}
    <button
      class="center-play-btn"
      type="button"
      onclick={onPlayPause}
      aria-label={playing ? 'Pause' : 'Play'}
    >
      <span class="material-icons-outlined">{playing ? 'pause' : 'play_arrow'}</span>
    </button>
  {/if}

  <!-- Bottom controls bar -->
  <div class="controls-bar">
    <!-- Seek bar (hidden for live streams) -->
    {#if !isLive}
      <div
        class="seek-container"
        class:scrubbing
        bind:this={seekBarEl}
        onmousemove={handleSeekBarHover}
        onmouseleave={handleSeekBarLeave}
        role="group"
        aria-label="Seek bar"
      >
        <div class="seek-track">
          <div class="seek-buffered" style="width: {bufferedPercent}%"></div>
          {#each skipSegments as seg, i (i)}
            {#if duration > 0 && isVisibleSkipSegment(seg) && seg.end_seconds > seg.start_seconds}
              {@const editable = onEditSkipSegment !== undefined}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="seek-segment"
                class:editable
                style="left: {(seg.start_seconds / duration) * 100}%; width: {((seg.end_seconds -
                  seg.start_seconds) /
                  duration) *
                  100}%; background: {SEGMENT_COLORS[seg.segment_type]}"
                title={editable ? `${segmentTitle(seg)} — click to edit` : segmentTitle(seg)}
                onpointerdown={editable
                  ? (e) => {
                      e.stopPropagation();
                      e.preventDefault();
                    }
                  : undefined}
                onclick={editable
                  ? (e) => {
                      e.stopPropagation();
                      onEditSkipSegment?.(seg);
                    }
                  : undefined}
              ></div>
            {/if}
          {/each}
          <div class="seek-progress" style="width: {progressPercent}%"></div>
          {#each chapters as chapter (chapter.index)}
            {#if duration > 0}
              <div
                class="chapter-marker"
                style="left: {(chapter.start_seconds / duration) * 100}%"
                title={chapter.title || ''}
              ></div>
            {/if}
          {/each}
        </div>
        <input
          type="range"
          class="seek-input"
          min="0"
          max={duration || 0}
          step="0.1"
          value={seekValue}
          oninput={handleSeekInput}
          onchange={handleSeekCommit}
          aria-label="Seek"
        />
        {#if showTooltip}
          <div
            class="seek-tooltip"
            style="left: {tooltipPercent}%; transform: translateX({tooltipTranslateX}%)"
          >
            {#if tooltipThumbnail}
              <div
                class="thumb-preview"
                style="
                  width: {tooltipThumbnail.w}px;
                  height: {tooltipThumbnail.h}px;
                  background-image: url({tooltipThumbnail.url});
                  background-position: -{tooltipThumbnail.x}px -{tooltipThumbnail.y}px;
                "
              ></div>
            {/if}
            <span class="thumb-time">{formatTime(tooltipTime)}</span>
          </div>
        {/if}
      </div>
    {/if}

    <!-- Main controls row -->
    <div class="controls-row">
      <div class="controls-left">
        <!-- Play/Pause -->
        <button
          class="ctrl-btn"
          type="button"
          onclick={onPlayPause}
          aria-label={playing ? 'Pause' : 'Play'}
        >
          <span class="material-icons-outlined">{playing ? 'pause' : 'play_arrow'}</span>
        </button>

        <!-- Volume (hidden on touch — use hardware buttons) -->
        {#if !isTouchDevice}
          <div
            class="volume-group"
            onmouseenter={() => (volumeExpanded = true)}
            onmouseleave={() => (volumeExpanded = false)}
            role="group"
            aria-label="Volume controls"
          >
            <button
              class="ctrl-btn"
              type="button"
              onclick={onMuteToggle}
              aria-label={muted ? 'Unmute' : 'Mute'}
            >
              <span class="material-icons-outlined">{volumeIcon}</span>
            </button>
            {#if volumeExpanded}
              <div class="volume-slider-wrap">
                <input
                  type="range"
                  class="volume-slider"
                  min="0"
                  max="1"
                  step="0.01"
                  value={muted ? 0 : volume}
                  oninput={handleVolumeInput}
                  aria-label="Volume"
                />
              </div>
            {/if}
          </div>
        {/if}

        <!-- Time display / LIVE badge -->
        {#if isLive}
          <span class="live-badge">LIVE</span>
        {:else}
          <span class="time-display">
            {formatTime(seekValue)} / {formatTime(duration)}
          </span>
        {/if}

        {#if chapters.length > 0 || skipSegments.length > 0}
          <ChapterMenu
            {chapters}
            {skipSegments}
            {currentTime}
            {onSeek}
            isOpen={activePanel === 'chapters'}
            onToggle={() => openPanel('chapters')}
          />
        {/if}
      </div>

      <div class="controls-right">
        <!-- Switch stream (debrid sessions only) -->
        {#if onSwitchStream}
          <button
            class="ctrl-btn"
            type="button"
            onclick={onSwitchStream}
            aria-label="Switch stream"
            title="Switch stream"
          >
            <span class="material-icons-outlined">swap_horiz</span>
          </button>
        {/if}

        {#if keep}
          <button
            class="ctrl-btn keep-btn"
            type="button"
            onclick={keep.onKeep}
            disabled={keep.busy || keep.saved}
            aria-label={keep.label}
            title={keep.label}
          >
            <span class="material-icons-outlined"
              >{keep.saved ? 'library_add_check' : 'library_add'}</span
            >
            {#if keep.busy || keep.saved}
              <span class="keep-status" aria-live="polite">{keep.label}</span>
            {/if}
          </button>
        {/if}

        <!-- Episode list (TV shows only) -->
        {#if seasonEpisodes && seasonEpisodes.length > 1 && onSelectEpisode && currentEpisodeId}
          <EpisodeListPanel
            episodes={seasonEpisodes}
            {currentEpisodeId}
            {onSelectEpisode}
            isOpen={activePanel === 'episodes'}
            onToggle={() => openPanel('episodes')}
          />
        {/if}

        <!-- Audio track picker (only when multiple tracks) -->
        {#if audioTracks.length > 1}
          <TrackPicker
            tracks={audioTrackItems}
            icon="audiotrack"
            showOff={false}
            onSelect={(id) => onAudioTrackSelect(id !== null ? parseInt(id.split('-')[1]) : null)}
            isOpen={activePanel === 'audio'}
            onToggle={() => openPanel('audio')}
          />
        {/if}

        <!-- Subtitles -->
        {#if subtitles.length > 0 || onSearchSubtitles}
          <SubtitlePanel
            {subtitles}
            {activeSubtitleId}
            {onSubtitleSelect}
            {onSearchSubtitles}
            isOpen={activePanel === 'subtitles'}
            onToggle={() => openPanel('subtitles')}
          />
        {/if}

        <!-- Playback speed (hidden for live) -->
        {#if !isLive}
          <div class="speed-group" role="group">
            <button
              class="ctrl-btn speed-btn"
              type="button"
              onclick={() => openPanel('speed')}
              aria-label="Playback speed"
              title="Playback speed"
            >
              {playbackSpeed === 1 ? '1x' : `${playbackSpeed}x`}
            </button>
            {#if activePanel === 'speed'}
              <div class="speed-menu">
                {#each SPEED_OPTIONS as speed (speed)}
                  <button
                    class="speed-option"
                    class:active={playbackSpeed === speed}
                    type="button"
                    onclick={() => {
                      onSpeedChange(speed);
                      activePanel = null;
                    }}
                  >
                    {speed}x
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        {/if}

        <!-- Cast -->
        <CastButton class="ctrl-btn" />

        <!-- PiP -->
        {#if pipSupported}
          <button
            class="ctrl-btn"
            type="button"
            onclick={onPipToggle}
            aria-label={isPip ? 'Exit picture-in-picture' : 'Picture-in-picture'}
            title={isPip ? 'Exit picture-in-picture' : 'Picture-in-picture'}
          >
            <span class="material-icons-outlined"
              >{isPip ? 'pip_exit' : 'picture_in_picture_alt'}</span
            >
          </button>
        {/if}

        <!-- Aspect ratio mode -->
        <button
          class="ctrl-btn"
          type="button"
          onclick={onAspectModeCycle}
          aria-label="Aspect ratio: {aspectMode}"
          title="Aspect: {aspectMode} (tap to cycle)"
        >
          <span class="material-icons-outlined"
            >{aspectMode === 'fill'
              ? 'crop_landscape'
              : aspectMode === 'fit'
                ? 'fit_screen'
                : 'aspect_ratio'}</span
          >
        </button>

        <!-- Fullscreen -->
        <button
          class="ctrl-btn"
          type="button"
          onclick={onFullscreenToggle}
          aria-label={isFullscreen ? 'Exit fullscreen' : 'Fullscreen'}
        >
          <span class="material-icons-outlined"
            >{isFullscreen ? 'fullscreen_exit' : 'fullscreen'}</span
          >
        </button>
      </div>
    </div>
  </div>
</div>

<style>
  .controls-overlay {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    opacity: 0;
    transition: opacity 0.3s ease;
    pointer-events: none;
    z-index: 2;
  }

  .controls-overlay.visible {
    opacity: 1;
  }

  .media-info {
    position: absolute;
    top: calc(16px + env(safe-area-inset-top));
    left: calc(24px + env(safe-area-inset-left));
    display: flex;
    flex-direction: column;
    gap: 2px;
    pointer-events: none;
    max-width: 50%;
  }

  .media-title {
    font-size: 16px;
    font-weight: 600;
    color: white;
    text-shadow: 0 1px 4px rgba(0, 0, 0, 0.5);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .media-subtitle {
    font-size: 13px;
    color: rgba(255, 255, 255, 0.7);
    text-shadow: 0 1px 4px rgba(0, 0, 0, 0.5);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .close-btn {
    position: absolute;
    top: calc(16px + env(safe-area-inset-top));
    right: calc(16px + env(safe-area-inset-right));
    display: flex;
    align-items: center;
    justify-content: center;
    width: 44px;
    height: 44px;
    border-radius: 50%;
    border: none;
    background: rgba(0, 0, 0, 0.6);
    color: #fff;
    cursor: pointer;
    transition: background 0.15s;
    pointer-events: auto;
  }

  .close-btn:focus {
    outline: none;
  }

  .close-btn:focus-visible {
    outline: 2px solid rgba(255, 255, 255, 0.6);
    outline-offset: 2px;
  }

  .close-btn:hover {
    background: rgba(0, 0, 0, 0.8);
  }

  .close-btn .material-icons-outlined {
    font-size: 24px;
  }

  .center-play-btn {
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    display: flex;
    align-items: center;
    justify-content: center;
    width: 64px;
    height: 64px;
    border-radius: 50%;
    border: none;
    background: rgba(0, 0, 0, 0.5);
    color: #fff;
    cursor: pointer;
    pointer-events: auto;
    transition: background 0.15s;
    z-index: 1;
  }

  .center-play-btn:focus {
    outline: none;
  }

  .center-play-btn:focus-visible {
    outline: 2px solid rgba(255, 255, 255, 0.6);
    outline-offset: 2px;
  }

  .center-play-btn:active {
    background: rgba(0, 0, 0, 0.7);
  }

  .center-play-btn .material-icons-outlined {
    font-size: 40px;
  }

  .controls-bar {
    background: linear-gradient(transparent, rgba(0, 0, 0, 0.85));
    padding: 40px calc(16px + env(safe-area-inset-right)) calc(12px + env(safe-area-inset-bottom))
      calc(16px + env(safe-area-inset-left));
    pointer-events: auto;
  }

  /* Seek bar */
  .seek-container {
    position: relative;
    height: 20px;
    display: flex;
    align-items: center;
    cursor: pointer;
    margin-bottom: 4px;
  }

  .seek-track {
    position: absolute;
    left: 0;
    right: 0;
    height: 4px;
    background: rgba(255, 255, 255, 0.2);
    border-radius: 2px;
    overflow: hidden;
    pointer-events: none;
  }

  .seek-container:hover .seek-track {
    height: 6px;
  }

  .seek-buffered {
    position: absolute;
    top: 0;
    left: 0;
    height: 100%;
    background: rgba(255, 255, 255, 0.3);
    border-radius: 2px;
  }

  .seek-progress {
    position: absolute;
    top: 0;
    left: 0;
    height: 100%;
    background: var(--accent, #e8a87c);
    border-radius: 2px;
  }

  .seek-input {
    position: relative;
    width: 100%;
    height: 20px;
    margin: 0;
    padding: 0;
    background: transparent;
    cursor: pointer;
    -webkit-appearance: none;
    appearance: none;
  }

  .seek-input::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--accent, #e8a87c);
    border: none;
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.15s;
  }

  .seek-container:hover .seek-input::-webkit-slider-thumb {
    opacity: 1;
  }

  .seek-input::-moz-range-thumb {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--accent, #e8a87c);
    border: none;
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.15s;
  }

  .seek-container:hover .seek-input::-moz-range-thumb {
    opacity: 1;
  }

  .seek-input::-webkit-slider-runnable-track {
    background: transparent;
  }

  .seek-input::-moz-range-track {
    background: transparent;
  }

  .seek-tooltip {
    position: absolute;
    bottom: 24px;
    background: rgba(0, 0, 0, 0.85);
    color: #fff;
    font-size: 12px;
    padding: 4px 6px;
    border-radius: 4px;
    pointer-events: none;
    white-space: nowrap;
    display: flex;
    flex-direction: column;
    align-items: center;
  }

  .thumb-preview {
    border-radius: 4px;
    border: 2px solid rgba(255, 255, 255, 0.3);
    margin-bottom: 4px;
  }

  .thumb-time {
    display: block;
    text-align: center;
  }

  .chapter-marker {
    position: absolute;
    top: 0;
    width: 3px;
    height: 100%;
    background: rgba(255, 255, 255, 0.5);
    border-radius: 1px;
    pointer-events: none;
    z-index: 1;
  }

  .seek-segment {
    position: absolute;
    top: 0;
    height: 100%;
    pointer-events: none;
  }

  .seek-segment.editable {
    pointer-events: auto;
    cursor: pointer;
  }

  .seek-segment.editable:hover {
    filter: brightness(1.25);
  }

  /* Controls row */
  .controls-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .controls-left {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .controls-right {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .ctrl-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 40px;
    height: 40px;
    border-radius: 50%;
    border: none;
    background: transparent;
    color: #fff;
    cursor: pointer;
    transition: background 0.15s;
  }

  .ctrl-btn:hover {
    background: rgba(255, 255, 255, 0.15);
  }

  .ctrl-btn:focus {
    outline: none;
  }

  .ctrl-btn:focus-visible {
    outline: 2px solid rgba(255, 255, 255, 0.6);
    outline-offset: 2px;
  }

  .ctrl-btn .material-icons-outlined {
    font-size: 26px;
  }

  .volume-group {
    display: flex;
    align-items: center;
  }

  .volume-slider-wrap {
    width: 80px;
    display: flex;
    align-items: center;
  }

  .volume-slider {
    width: 100%;
    height: 4px;
    margin: 0;
    cursor: pointer;
    -webkit-appearance: none;
    appearance: none;
    background: rgba(255, 255, 255, 0.3);
    border-radius: 2px;
    outline: none;
  }

  .volume-slider::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: #fff;
    border: none;
    cursor: pointer;
  }

  .volume-slider::-moz-range-thumb {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: #fff;
    border: none;
    cursor: pointer;
  }

  .time-display {
    color: rgba(255, 255, 255, 0.9);
    font-size: 13px;
    font-variant-numeric: tabular-nums;
    margin-left: 8px;
    user-select: none;
  }

  .live-badge {
    display: inline-flex;
    align-items: center;
    padding: 3px 10px;
    background: var(--danger, #ef4444);
    color: white;
    border-radius: 4px;
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.5px;
    text-transform: uppercase;
    margin-left: 8px;
    animation: pulse-live 2s ease-in-out infinite;
  }

  @keyframes pulse-live {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.7;
    }
  }

  .speed-group {
    position: relative;
  }

  .speed-btn {
    font-size: 14px;
    font-weight: 600;
    min-width: 40px;
  }

  .keep-btn {
    width: auto;
    min-width: 40px;
    padding: 0 8px;
    border-radius: 20px;
    gap: 6px;
  }

  .keep-btn:disabled {
    cursor: default;
  }

  .keep-status {
    font-size: 12px;
    white-space: nowrap;
  }

  .speed-menu {
    position: absolute;
    bottom: 100%;
    left: 50%;
    transform: translateX(-50%);
    margin-bottom: 8px;
    background: rgba(0, 0, 0, 0.9);
    border-radius: 8px;
    padding: 4px;
    display: flex;
    flex-direction: column;
    pointer-events: auto;
  }

  .speed-option {
    padding: 6px 16px;
    border: none;
    background: transparent;
    color: rgba(255, 255, 255, 0.8);
    font-size: 14px;
    cursor: pointer;
    border-radius: 4px;
    white-space: nowrap;
  }

  .speed-option:hover {
    background: rgba(255, 255, 255, 0.1);
  }

  .speed-option.active {
    color: var(--accent, #818cf8);
    font-weight: 600;
  }

  /* Scrubbing active state (thumb visible, track expanded) */
  .seek-container.scrubbing .seek-track {
    height: 6px;
  }

  .seek-container.scrubbing .seek-input::-webkit-slider-thumb {
    opacity: 1;
  }

  .seek-container.scrubbing .seek-input::-moz-range-thumb {
    opacity: 1;
  }

  /* Mobile layout */
  @media (max-width: 768px) {
    .controls-bar {
      padding: 30px calc(12px + env(safe-area-inset-right)) calc(8px + env(safe-area-inset-bottom))
        calc(12px + env(safe-area-inset-left));
    }

    .ctrl-btn {
      width: 44px;
      height: 44px;
    }

    .ctrl-btn .material-icons-outlined {
      font-size: 28px;
    }

    .controls-row {
      gap: 2px;
    }

    .time-display {
      font-size: 12px;
      margin-left: 4px;
    }

    .seek-container {
      height: 28px;
    }

    .seek-input {
      height: 28px;
    }

    .seek-track {
      height: 6px;
    }

    .seek-input::-webkit-slider-thumb {
      width: 18px;
      height: 18px;
      opacity: 1;
    }

    .seek-input::-moz-range-thumb {
      width: 18px;
      height: 18px;
      opacity: 1;
    }

    .media-info {
      top: calc(12px + env(safe-area-inset-top));
      left: calc(16px + env(safe-area-inset-left));
      max-width: 65%;
    }

    .thumb-preview {
      transform: scale(0.75);
      transform-origin: bottom center;
    }

    .seek-tooltip {
      bottom: 32px;
    }
  }
</style>
