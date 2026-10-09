<script lang="ts">
  import {
    getAudioPlayerState,
    togglePlayPause,
    skipForward,
    skipBackward,
    setRate,
    setVolume,
    setSkipSilence,
    setNormalizeVolume,
    setNightMode
  } from '$lib/stores/audioplayer.svelte';
  import SleepTimer from './SleepTimer.svelte';

  interface Props {
    /** Show volume control (hidden on mini player) */
    showVolume?: boolean;
    /** Show sleep timer (full player only) */
    showSleepTimer?: boolean;
    /** Large mode for full player */
    large?: boolean;
  }

  let { showVolume = false, showSleepTimer = false, large = false }: Props = $props();

  const player = getAudioPlayerState();

  let showSpeedMenu = $state(false);
  let showVolumeSlider = $state(false);

  const speeds = [0.5, 0.75, 1, 1.25, 1.5, 1.75, 2, 2.5, 3];

  function handleSpeedSelect(speed: number) {
    setRate(speed);
    showSpeedMenu = false;
  }

  function handleVolumeChange(e: Event) {
    const target = e.target as HTMLInputElement;
    setVolume(parseFloat(target.value));
  }

  function toggleMute() {
    if (player.volume > 0) {
      setVolume(0);
    } else {
      setVolume(1);
    }
  }
</script>

<div class="audio-controls" class:large>
  <!-- Speed control -->
  <div class="speed-control">
    <button
      type="button"
      class="control-btn speed-btn"
      onclick={() => (showSpeedMenu = !showSpeedMenu)}
      title="Playback speed"
    >
      {player.playbackRate}x
    </button>
    {#if showSpeedMenu}
      <div class="speed-menu" role="presentation" onclick={() => (showSpeedMenu = false)}>
        {#each speeds as speed (speed)}
          <button
            type="button"
            class="speed-option"
            class:active={player.playbackRate === speed}
            onclick={() => handleSpeedSelect(speed)}
          >
            {speed}x
          </button>
        {/each}
      </div>
    {/if}
  </div>

  <!-- Skip back -->
  <button
    type="button"
    class="control-btn skip-btn"
    onclick={() => skipBackward(30)}
    title="Back 30 seconds"
  >
    <span class="material-icons-outlined" class:icon-lg={large}>replay_30</span>
  </button>

  <!-- Play/Pause -->
  <button
    type="button"
    class="control-btn play-btn"
    onclick={togglePlayPause}
    title={player.playing ? 'Pause' : 'Play'}
    disabled={player.loading}
  >
    {#if player.loading}
      <div class="spinner"></div>
    {:else}
      <span class="material-icons-outlined" class:icon-lg={large}>
        {player.playing ? 'pause' : 'play_arrow'}
      </span>
    {/if}
  </button>

  <!-- Skip forward -->
  <button
    type="button"
    class="control-btn skip-btn"
    onclick={() => skipForward(30)}
    title="Forward 30 seconds"
  >
    <span class="material-icons-outlined" class:icon-lg={large}>forward_30</span>
  </button>

  <!-- Skip silence toggle -->
  <button
    type="button"
    class="control-btn silence-btn"
    class:active={player.skipSilence}
    onclick={() => setSkipSilence(!player.skipSilence)}
    title={player.skipSilence ? 'Skip silence: on' : 'Skip silence: off'}
  >
    <span class="material-icons-outlined">speed</span>
  </button>

  <!-- Volume normalize -->
  <button
    type="button"
    class="control-btn silence-btn"
    class:active={player.normalizeVolume}
    onclick={() => setNormalizeVolume(!player.normalizeVolume)}
    title={player.normalizeVolume ? 'Volume normalization on' : 'Volume normalization off'}
  >
    <span class="material-icons-outlined">graphic_eq</span>
  </button>

  <!-- Night mode (dynamic range compression) -->
  <button
    type="button"
    class="control-btn silence-btn"
    class:active={player.nightMode}
    onclick={() => setNightMode(!player.nightMode)}
    title={player.nightMode
      ? 'Night mode on (compresses loud peaks)'
      : 'Night mode off (compress dynamic range)'}
  >
    <span class="material-icons-outlined">compress</span>
  </button>

  <!-- Sleep timer -->
  {#if showSleepTimer}
    <SleepTimer />
  {/if}

  <!-- Volume -->
  {#if showVolume}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="volume-control" onmouseleave={() => (showVolumeSlider = false)}>
      <button
        type="button"
        class="control-btn"
        onclick={toggleMute}
        onmouseenter={() => (showVolumeSlider = true)}
        title={player.volume === 0 ? 'Unmute' : 'Mute'}
      >
        <span class="material-icons-outlined">
          {player.volume === 0 ? 'volume_off' : 'volume_up'}
        </span>
      </button>
      {#if showVolumeSlider}
        <div class="volume-slider-container">
          <input
            type="range"
            min="0"
            max="1"
            step="0.05"
            value={player.volume}
            oninput={handleVolumeChange}
            class="volume-slider"
          />
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .audio-controls {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
  }

  .audio-controls.large {
    gap: 16px;
  }

  .control-btn {
    background: none;
    border: none;
    color: var(--ink);
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 8px;
    border-radius: 50%;
    transition:
      background 0.15s,
      opacity 0.15s;
    opacity: 0.85;
    position: relative;
  }

  .control-btn:hover {
    background: rgba(255, 255, 255, 0.08);
    opacity: 1;
  }

  .control-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .play-btn {
    background: var(--accent);
    color: #fff;
    opacity: 1;
    padding: 12px;
  }

  .large .play-btn {
    padding: 16px;
  }

  .play-btn:hover {
    background: var(--accent);
    filter: brightness(1.15);
  }

  .icon-lg {
    font-size: 32px;
  }

  .speed-control {
    position: relative;
  }

  .speed-btn {
    font-size: 0.8rem;
    font-weight: 600;
    min-width: 40px;
    border-radius: 6px;
    padding: 4px 8px;
  }

  .speed-menu {
    position: absolute;
    bottom: 100%;
    left: 50%;
    transform: translateX(-50%);
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 8px;
    padding: 4px;
    display: flex;
    flex-direction: column;
    z-index: 20;
    box-shadow: var(--shadow);
    min-width: 60px;
    margin-bottom: 4px;
  }

  .speed-option {
    background: none;
    border: none;
    color: var(--ink);
    cursor: pointer;
    padding: 6px 12px;
    border-radius: 4px;
    font-size: 0.8rem;
    text-align: center;
    white-space: nowrap;
  }

  .speed-option:hover {
    background: rgba(255, 255, 255, 0.08);
  }

  .speed-option.active {
    color: var(--accent);
    font-weight: 600;
  }

  .silence-btn {
    font-size: 0.7rem;
    border-radius: 6px;
    padding: 4px 6px;
    opacity: 0.5;
  }

  .silence-btn.active {
    opacity: 1;
    color: var(--accent);
  }

  .silence-btn .material-icons-outlined {
    font-size: 18px;
  }

  .volume-control {
    position: relative;
    display: flex;
    align-items: center;
  }

  .volume-slider-container {
    position: absolute;
    bottom: calc(100% + 4px);
    left: 50%;
    transform: translateX(-50%);
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 8px;
    padding: 12px 8px;
    box-shadow: var(--shadow);
    z-index: 20;
  }

  /* Transparent bridge so the wrapper mouseleave doesn't fire while
     crossing the visual gap between the volume button and the slider. */
  .volume-slider-container::after {
    content: '';
    position: absolute;
    top: 100%;
    left: 0;
    right: 0;
    height: 6px;
  }

  .volume-slider {
    writing-mode: vertical-lr;
    direction: rtl;
    width: 4px;
    height: 80px;
    appearance: none;
    background: var(--ring);
    border-radius: 2px;
    cursor: pointer;
  }

  .volume-slider::-webkit-slider-thumb {
    appearance: none;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--accent);
    cursor: pointer;
  }

  .spinner {
    width: 22px;
    height: 22px;
    border: 2px solid rgba(255, 255, 255, 0.3);
    border-top-color: #fff;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  .large .spinner {
    width: 32px;
    height: 32px;
    border-width: 3px;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
