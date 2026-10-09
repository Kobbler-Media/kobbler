<script lang="ts">
  import {
    getReadAlongState,
    togglePlayPause,
    stop,
    setSpeed,
    setVoice,
    setLang,
    setContinuous,
    setTemperature,
    restartTtsWithCurrentSettings,
    validateVoiceAgainst
  } from '$lib/stores/readalong.svelte';
  import type { TtsVoice } from '$lib/api/client';
  import {
    listVoicePresets,
    setActivePreset as apiSetActivePreset,
    type UserVoicePreset
  } from '$lib/api/client';
  interface Props {
    voices?: Record<string, TtsVoice[]>;
    supportsEmotion?: boolean;
  }

  let { voices = {}, supportsEmotion = false }: Props = $props();

  const readAlongState = getReadAlongState();

  let showSpeedMenu = $state(false);
  let showVoicePanel = $state(false);
  let presets = $state<UserVoicePreset[]>([]);
  let showTunePanel = $state(false);
  let presetsLoaded = $state(false);
  const speeds = [0.75, 1, 1.25, 1.5, 2];

  $effect(() => {
    if (!presetsLoaded) {
      presetsLoaded = true;
      listVoicePresets()
        .then((p) => {
          presets = p;
        })
        .catch(() => {});
    }
  });

  // Active preset name for display
  const activePresetName = $derived(() => {
    if (!readAlongState.activePresetId) return null;
    const p = presets.find((pr) => pr.id === readAlongState.activePresetId);
    return p?.name ?? null;
  });

  async function selectPreset(preset: UserVoicePreset) {
    try {
      await apiSetActivePreset(preset.id);
      if (preset.voice) setVoice(preset.voice);
      setSpeed(preset.speed);
      setTemperature(preset.temperature);
      restartTtsWithCurrentSettings();
      showVoicePanel = false;
    } catch {
      /* ignore */
    }
  }

  // Current language's voices
  const langVoices = $derived(voices[readAlongState.lang] ?? []);

  // When voices load (or lang changes), drop any stored voice IDs that
  // aren't actually available — otherwise we'd ship a stale voice to TTS.
  $effect(() => {
    if (langVoices.length > 0) {
      validateVoiceAgainst(langVoices.map((v) => v.id));
    }
  });

  // Parse the current voice string into IDs
  const selectedVoiceIds = $derived(readAlongState.voice.split('+').filter((id) => id.length > 0));

  // Display label for the current voice
  const voiceLabel = $derived(() => {
    if (selectedVoiceIds.length === 0) return 'Voice';
    const names = selectedVoiceIds.map((id) => {
      const v = langVoices.find((voice) => voice.id === id);
      return v?.name ?? id;
    });
    if (names.length === 1) return names[0];
    return `${names.length} voices`;
  });

  function toggleVoice(voiceId: string) {
    const current = selectedVoiceIds;
    let next: string[];
    if (current.includes(voiceId)) {
      next = current.filter((id) => id !== voiceId);
    } else {
      next = [...current, voiceId];
    }
    setVoice(next.join('+'));
    // Restart TTS with the new voice on next page
    restartTtsWithCurrentSettings();
  }

  function handleLangSwitch(newLang: string) {
    if (newLang === readAlongState.lang) return;
    setLang(newLang);
    setVoice('');
  }

  function handleWindowClick(e: MouseEvent) {
    const target = e.target as HTMLElement;
    if (showSpeedMenu && !target.closest('.speed-wrapper')) {
      showSpeedMenu = false;
    }
    if (showVoicePanel && !target.closest('.voice-wrapper')) {
      showVoicePanel = false;
    }
    if (showTunePanel && !target.closest('.tune-wrapper')) {
      showTunePanel = false;
    }
  }
</script>

<svelte:window onclick={handleWindowClick} />

<div class="readalong-controls">
  <div class="controls-row">
    <!-- Mode indicator -->
    <span
      class="mode-icon"
      title={readAlongState.mode === 'tts' ? 'Text-to-Speech' : 'Audiobook sync'}
    >
      <span class="material-icons-outlined">
        {readAlongState.mode === 'tts' ? 'record_voice_over' : 'headphones'}
      </span>
    </span>

    <!-- Play/Pause (TTS mode only - audiobook mode uses its own controls) -->
    {#if readAlongState.mode === 'tts'}
      <button
        type="button"
        class="ctrl-btn"
        onclick={togglePlayPause}
        disabled={readAlongState.loading}
        title={readAlongState.playing ? 'Pause' : 'Resume'}
      >
        <span class="material-icons-outlined">
          {#if readAlongState.loading}
            hourglass_empty
          {:else if readAlongState.playing}
            pause
          {:else}
            play_arrow
          {/if}
        </span>
      </button>
    {/if}

    <!-- Speed -->
    <div class="speed-wrapper">
      <button
        type="button"
        class="ctrl-btn speed-btn"
        onclick={() => (showSpeedMenu = !showSpeedMenu)}
        title="Reading speed — the voice is re-synthesised at the new speed"
      >
        {readAlongState.speed}x
      </button>
      {#if showSpeedMenu}
        <div class="speed-menu">
          <span class="speed-note">Re-reads the page at the new speed</span>
          {#each speeds as s (s)}
            <button
              type="button"
              class="speed-option"
              class:active={readAlongState.speed === s}
              onclick={() => {
                setSpeed(s);
                showSpeedMenu = false;
              }}
            >
              {s}x
            </button>
          {/each}
        </div>
      {/if}
    </div>

    <!-- Voice picker (TTS only) -->
    {#if readAlongState.mode === 'tts'}
      <div class="voice-wrapper">
        <button
          type="button"
          class="ctrl-btn voice-btn"
          onclick={() => (showVoicePanel = !showVoicePanel)}
          title="Change voice"
        >
          <span class="material-icons-outlined" style="font-size: 16px">mic</span>
          <span class="voice-label">{activePresetName() ?? voiceLabel()}</span>
        </button>
        {#if showVoicePanel}
          <div class="voice-panel">
            {#if presets.length > 0}
              <div class="preset-section">
                <div class="voice-section-label">Your Presets</div>
                {#each presets as preset (preset.id)}
                  <button
                    type="button"
                    class="preset-item"
                    class:active={readAlongState.activePresetId === preset.id}
                    onclick={() => selectPreset(preset)}
                  >
                    <span class="preset-item-check">
                      {#if readAlongState.activePresetId === preset.id}
                        <span class="material-icons-outlined" style="font-size: 16px">check</span>
                      {/if}
                    </span>
                    {preset.name}
                  </button>
                {/each}
              </div>
            {/if}
            {#if presets.length > 0}
              <div class="voice-section-label">Service Voices</div>
            {/if}
            <div class="voice-panel-header">
              <span class="voice-panel-title">Voice</span>
              <div class="lang-toggle">
                <button
                  type="button"
                  class="lang-btn"
                  class:active={readAlongState.lang === 'a'}
                  onclick={() => handleLangSwitch('a')}
                  title="American English">US</button
                >
                <button
                  type="button"
                  class="lang-btn"
                  class:active={readAlongState.lang === 'b'}
                  onclick={() => handleLangSwitch('b')}
                  title="British English">GB</button
                >
              </div>
            </div>
            <div class="voice-list">
              {#each langVoices as v (v.id)}
                <button
                  type="button"
                  class="voice-item"
                  class:selected={selectedVoiceIds.includes(v.id)}
                  onclick={() => toggleVoice(v.id)}
                >
                  <span class="voice-item-check">
                    {#if selectedVoiceIds.includes(v.id)}
                      <span class="material-icons-outlined" style="font-size: 16px">check</span>
                    {/if}
                  </span>
                  {v.name}
                </button>
              {/each}
            </div>
            {#if selectedVoiceIds.length > 1}
              <div class="voice-panel-hint">Multiple voices blend together</div>
            {/if}
          </div>
        {/if}
      </div>
    {/if}

    <!-- Quick-tune (TTS + Chatterbox only) -->
    {#if readAlongState.mode === 'tts' && supportsEmotion}
      <div class="tune-wrapper">
        <button
          type="button"
          class="ctrl-btn tune-btn"
          onclick={() => (showTunePanel = !showTunePanel)}
          title="Voice tuning"
        >
          <span class="material-icons-outlined" style="font-size: 18px">tune</span>
        </button>
        {#if showTunePanel}
          <div class="tune-panel">
            <div class="tune-title">Voice Tuning</div>
            <label class="tune-slider">
              <span class="tune-label">Variation</span>
              <input
                type="range"
                min="0"
                max="1.5"
                step="0.05"
                value={readAlongState.temperature ?? 0.8}
                oninput={(e) => setTemperature(parseFloat((e.target as HTMLInputElement).value))}
              />
              <span class="tune-value">{(readAlongState.temperature ?? 0.8).toFixed(2)}</span>
            </label>
          </div>
        {/if}
      </div>
    {/if}

    <!-- Continuous toggle -->
    <button
      type="button"
      class="ctrl-btn"
      class:active={readAlongState.continuous}
      onclick={() => setContinuous(!readAlongState.continuous)}
      title={readAlongState.continuous ? 'Continuous reading (on)' : 'Page-at-a-time (on)'}
    >
      <span class="material-icons-outlined">
        {readAlongState.continuous ? 'repeat' : 'looks_one'}
      </span>
    </button>

    <!-- Stop -->
    <button type="button" class="ctrl-btn" onclick={stop} title="Stop read-along">
      <span class="material-icons-outlined">close</span>
    </button>
  </div>

  {#if readAlongState.error}
    <div class="readalong-error">{readAlongState.error}</div>
  {/if}
</div>

<style>
  .readalong-controls {
    position: fixed;
    bottom: 0;
    left: 0;
    right: 0;
    z-index: 100;
    background: var(--readalong-bg, rgba(0, 0, 0, 0.85));
    backdrop-filter: blur(12px);
    border-top: 1px solid var(--readalong-border, rgba(255, 255, 255, 0.1));
    padding: 8px calc(16px + env(safe-area-inset-right)) calc(8px + env(safe-area-inset-bottom))
      calc(16px + env(safe-area-inset-left));
  }

  .controls-row {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 12px;
    max-width: 700px;
    margin: 0 auto;
  }

  .mode-icon {
    display: flex;
    align-items: center;
    color: var(--accent, #4a90d9);
    opacity: 0.8;
  }

  .mode-icon .material-icons-outlined {
    font-size: 18px;
  }

  .ctrl-btn {
    background: none;
    border: 1px solid var(--readalong-border, rgba(255, 255, 255, 0.15));
    color: inherit;
    cursor: pointer;
    padding: 6px 10px;
    border-radius: 8px;
    display: flex;
    align-items: center;
    font-size: 0.85rem;
    transition:
      background 0.15s,
      opacity 0.15s;
  }

  .ctrl-btn:hover:not(:disabled) {
    background: rgba(128, 128, 128, 0.15);
  }

  .ctrl-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .ctrl-btn.active {
    color: var(--accent, #4a90d9);
    border-color: var(--accent, #4a90d9);
  }

  .ctrl-btn .material-icons-outlined {
    font-size: 20px;
  }

  .speed-wrapper {
    position: relative;
  }

  .speed-btn {
    font-variant-numeric: tabular-nums;
    min-width: 42px;
    justify-content: center;
  }

  .speed-menu {
    position: absolute;
    bottom: 100%;
    left: 50%;
    transform: translateX(-50%);
    margin-bottom: 6px;
    background: var(--card, #1a1a1a);
    border: 1px solid var(--ring, #333);
    border-radius: 8px;
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    box-shadow: 0 -4px 16px rgba(0, 0, 0, 0.4);
  }

  .speed-note {
    max-width: 12ch;
    padding: 4px 8px 6px;
    font-size: 0.7rem;
    line-height: 1.3;
    color: var(--muted, #888);
    border-bottom: 1px solid var(--ring, #333);
  }

  .speed-option {
    background: none;
    border: none;
    color: inherit;
    padding: 6px 16px;
    border-radius: 6px;
    cursor: pointer;
    font-size: 0.85rem;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }

  .speed-option:hover {
    background: rgba(128, 128, 128, 0.15);
  }

  .speed-option.active {
    color: var(--accent, #4a90d9);
    font-weight: 600;
  }

  .lang-toggle {
    display: flex;
    gap: 2px;
    border: 1px solid var(--readalong-border, rgba(255, 255, 255, 0.15));
    border-radius: 8px;
    overflow: hidden;
  }

  .lang-btn {
    background: none;
    border: none;
    color: inherit;
    padding: 4px 8px;
    font-size: 0.75rem;
    font-weight: 600;
    cursor: pointer;
    opacity: 0.5;
    transition:
      opacity 0.15s,
      background 0.15s;
  }

  .lang-btn:hover {
    opacity: 0.8;
    background: rgba(128, 128, 128, 0.1);
  }

  .lang-btn.active {
    opacity: 1;
    color: var(--accent, #4a90d9);
    background: rgba(74, 144, 217, 0.1);
  }

  .voice-wrapper {
    position: relative;
  }

  .voice-btn {
    gap: 4px;
  }

  .voice-label {
    font-size: 0.8rem;
    max-width: 100px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .voice-panel {
    position: absolute;
    bottom: 100%;
    left: 50%;
    transform: translateX(-50%);
    margin-bottom: 8px;
    background: var(--card, #1a1a1a);
    border: 1px solid var(--ring, #333);
    border-radius: 12px;
    padding: 8px;
    min-width: 220px;
    max-width: 300px;
    box-shadow: 0 -4px 24px rgba(0, 0, 0, 0.4);
    z-index: 20;
  }

  .voice-panel-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 4px 8px;
  }

  .voice-panel-title {
    font-size: 0.8rem;
    font-weight: 600;
    opacity: 0.7;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .voice-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-height: 240px;
    overflow-y: auto;
  }

  .voice-item {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: inherit;
    cursor: pointer;
    font-size: 0.85rem;
    text-align: left;
  }

  .voice-item:hover {
    background: rgba(128, 128, 128, 0.15);
  }

  .voice-item.selected {
    color: var(--accent, #4a90d9);
  }

  .voice-item-check {
    width: 16px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .voice-panel-hint {
    font-size: 0.7rem;
    opacity: 0.5;
    text-align: center;
    padding-top: 6px;
  }

  .readalong-error {
    text-align: center;
    color: #e74c3c;
    font-size: 0.75rem;
    margin-top: 4px;
  }

  .tune-wrapper {
    position: relative;
  }

  .tune-panel {
    position: absolute;
    bottom: 100%;
    left: 50%;
    transform: translateX(-50%);
    margin-bottom: 8px;
    background: var(--card, #1a1a1a);
    border: 1px solid var(--ring, #333);
    border-radius: 12px;
    padding: 12px;
    min-width: 220px;
    box-shadow: 0 -4px 24px rgba(0, 0, 0, 0.4);
    z-index: 20;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .tune-title {
    font-size: 0.8rem;
    font-weight: 600;
    opacity: 0.7;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .tune-slider {
    display: grid;
    grid-template-columns: 70px 1fr 36px;
    align-items: center;
    gap: 6px;
    font-size: 0.8rem;
  }

  .tune-label {
    color: var(--muted, #888);
    font-size: 0.75rem;
  }
  .tune-value {
    font-size: 0.75rem;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .tune-slider input[type='range'] {
    width: 100%;
  }

  .preset-section {
    padding-bottom: 8px;
    border-bottom: 1px solid var(--ring, #333);
    margin-bottom: 4px;
  }
  .preset-item {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: inherit;
    cursor: pointer;
    font-size: 0.85rem;
    text-align: left;
    width: 100%;
  }
  .preset-item:hover {
    background: rgba(128, 128, 128, 0.15);
  }
  .preset-item.active {
    color: var(--accent, #4a90d9);
  }
  .preset-item-check {
    width: 16px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .voice-section-label {
    font-size: 0.7rem;
    opacity: 0.5;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 4px 4px 2px;
  }
</style>
