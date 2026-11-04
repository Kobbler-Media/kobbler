<script lang="ts">
  import { onMount } from 'svelte';
  import { SearchSelect, MultiSearchSelect, type MultiSearchSelectItem } from '@koblibs/ui';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { getAuthState } from '#lib/stores/auth.svelte.js';
  import TranscriptRefresh from './TranscriptRefresh.svelte';
  import {
    getSystemConfig,
    updateSystemConfig,
    bulkChapterDetect,
    bulkRedetectChapters,
    bulkEbookExtract,
    repairEbooks,
    repairAudiobookChapters,
    kepubifySynced,
    getTtsStatus,
    synthesizeTts,
    type SystemConfig,
    type SystemConfigUpdate
  } from '#lib/api/client.js';

  // Local state
  let systemConfig = $state<SystemConfig | null>(null);
  let systemConfigLoading = $state(true);
  let systemConfigError = $state<string | null>(null);
  let systemConfigSaving = $state(false);
  let bulkDetecting = $state(false);
  let bulkRedetectingSilence = $state(false);
  let bulkExtracting = $state(false);
  let repairingEbooks = $state(false);
  let repairingChapters = $state(false);
  let kepubifying = $state(false);
  // TTS voice picker state
  let ttsVoiceItems = $state<MultiSearchSelectItem[]>([]);
  let ttsTestPlaying = $state(false);
  let ttsTestLoading = $state(false);
  let supportsEmotion = $state(false);
  let ttsTestAudioCtx: AudioContext | null = null;
  let ttsTestNextStartTime = 0;
  let ttsTestSources: AudioBufferSourceNode[] = [];

  const TTS_TEST_PARAGRAPH =
    'The old house stood at the end of a winding lane, its windows dark against the evening sky. A cool breeze stirred the curtains as she pushed open the creaking door. Somewhere in the distance, a church bell tolled the hour.';

  const auth = getAuthState();

  const transcriptModeOptions = [
    { value: 'off', label: 'Off' },
    { value: 'auto', label: 'Auto (on ingest)' },
    { value: 'manual', label: 'Manual (on demand)' }
  ];

  const ttsServiceTypeOptions = [
    { value: 'auto', label: 'Auto (detect capabilities)' },
    { value: 'kokoro', label: 'Kokoro' },
    { value: 'chatterbox', label: 'Chatterbox' },
    { value: 'breeze', label: 'BreezeTTS 2' },
    { value: 'openai', label: 'OpenAI-compatible' }
  ];

  const hwEncoderOptions = [
    { value: 'auto', label: 'Auto' },
    { value: 'vaapi', label: 'VAAPI' },
    { value: 'nvenc', label: 'NVENC' },
    { value: 'none', label: 'Software' }
  ];

  onMount(async () => {
    await loadSystemConfig();
    // Fetch TTS voices (non-blocking)
    getTtsStatus()
      .then((status) => {
        if (status.enabled && status.voices?.length) {
          ttsVoiceItems = status.voices.map((v) => ({
            value: v.id,
            label: v.name
          }));
        }
        supportsEmotion = status.supports_emotion ?? false;
      })
      .catch(() => {});
  });

  const loadSystemConfig = async () => {
    systemConfigLoading = true;
    systemConfigError = null;
    try {
      systemConfig = await getSystemConfig();
      if (systemConfig && !systemConfig.hw_encoder) {
        systemConfig.hw_encoder = 'auto';
      }
    } catch (e) {
      systemConfigError = e instanceof Error ? e.message : 'Failed to load config';
    } finally {
      systemConfigLoading = false;
    }
  };

  // TTS voice picker helpers
  const ttsSelectedVoices = $derived.by<MultiSearchSelectItem[]>(() => {
    const voiceStr = systemConfig?.tts_default_voice ?? '';
    if (!voiceStr) return [];
    return voiceStr
      .split('+')
      .filter((id: string) => id.length > 0)
      .map((id: string) => {
        const item = ttsVoiceItems.find((v) => v.value === id);
        return item ?? { value: id, label: id };
      });
  });

  function handleTtsVoiceChange(newValues: Array<string | { value: string; label: string }>) {
    if (!systemConfig) return;
    const ids = newValues.map((v) => (typeof v === 'string' ? v : v.value));
    systemConfig.tts_default_voice = ids.length > 0 ? ids.join('+') : null;
  }

  function cleanupTestAudio() {
    for (const src of ttsTestSources) {
      try {
        src.stop();
      } catch {
        /* already stopped */
      }
    }
    ttsTestSources = [];
    ttsTestAudioCtx?.close();
    ttsTestAudioCtx = null;
    ttsTestNextStartTime = 0;
  }

  async function testTtsVoice() {
    if (!systemConfig?.tts_default_voice) return;
    stopTtsTest();
    ttsTestLoading = true;

    try {
      const ctx = new AudioContext();
      ttsTestAudioCtx = ctx;
      ttsTestNextStartTime = ctx.currentTime;

      const resp = await synthesizeTts(TTS_TEST_PARAGRAPH, systemConfig.tts_default_voice, {
        temperature: systemConfig.tts_default_temperature ?? undefined
      });
      if (!resp.ok || !resp.body) throw new Error(`TTS failed: ${resp.status}`);

      const reader = resp.body.getReader();
      const decoder = new TextDecoder();
      let buffer = '';
      let chunkCount = 0;
      // Sample rate carried on `paragraph_start` for the streamed PCM path.
      // Defaults to 24 kHz, which is what tts-breeze emits; updated when the
      // server tells us otherwise. A mismatch here is a pitch shift, not an
      // exception, so the server value wins.
      let pcmSampleRate = 24_000;

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        const lines = buffer.split('\n');
        buffer = lines.pop() || '';

        for (const line of lines) {
          if (!line.trim()) continue;
          let chunk: Record<string, unknown>;
          try {
            chunk = JSON.parse(line);
          } catch {
            continue;
          }

          // Chatterbox / kokoro path: base64 MP3 per sentence.
          if (chunk.type === 'sentence' && chunk.audio_b64) {
            const binary = atob(chunk.audio_b64 as string);
            const bytes = new Uint8Array(binary.length);
            for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);

            const audioBuffer = await ctx.decodeAudioData(bytes.buffer.slice(0));
            const source = ctx.createBufferSource();
            source.buffer = audioBuffer;
            source.connect(ctx.destination);

            const startAt = Math.max(ttsTestNextStartTime, ctx.currentTime);
            source.start(startAt);
            ttsTestNextStartTime = startAt + audioBuffer.duration;
            ttsTestSources.push(source);
            chunkCount++;

            if (chunkCount === 1) {
              ttsTestLoading = false;
              ttsTestPlaying = true;
            }
            continue;
          }

          // Streamed paragraph path: raw 16-bit-LE PCM chunks with a
          // paragraph_start carrying the sample rate. Word-level alignment
          // events are ignored here — Test Voice just plays audio.
          if (chunk.type === 'paragraph_start') {
            pcmSampleRate = (chunk.sample_rate as number) ?? 24_000;
            continue;
          }

          if (chunk.type === 'audio' && chunk.pcm_b64) {
            const binary = atob(chunk.pcm_b64 as string);
            const bytes = new Uint8Array(binary.length);
            for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);

            const samples = new Int16Array(bytes.buffer.slice(0));
            const floats = new Float32Array(samples.length);
            for (let i = 0; i < samples.length; i++) floats[i] = samples[i] / 32768;
            const audioBuffer = ctx.createBuffer(1, samples.length, pcmSampleRate);
            audioBuffer.copyToChannel(floats, 0);

            const source = ctx.createBufferSource();
            source.buffer = audioBuffer;
            source.connect(ctx.destination);

            const startAt = Math.max(ttsTestNextStartTime, ctx.currentTime);
            source.start(startAt);
            ttsTestNextStartTime = startAt + audioBuffer.duration;
            ttsTestSources.push(source);
            chunkCount++;

            if (chunkCount === 1) {
              ttsTestLoading = false;
              ttsTestPlaying = true;
            }
            continue;
          }

          if (chunk.type === 'error') {
            console.warn('Test voice error:', chunk.message);
          }
        }
      }

      if (chunkCount === 0) throw new Error('No audio generated');

      // Set up end callback on the last scheduled source
      const lastSource = ttsTestSources[ttsTestSources.length - 1];
      if (lastSource) {
        lastSource.onended = () => {
          ttsTestPlaying = false;
          cleanupTestAudio();
        };
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'TTS test failed');
      ttsTestPlaying = false;
      cleanupTestAudio();
    } finally {
      ttsTestLoading = false;
    }
  }

  function stopTtsTest() {
    ttsTestPlaying = false;
    cleanupTestAudio();
  }

  const handleBulkChapterDetect = async () => {
    bulkDetecting = true;
    try {
      const result = await bulkChapterDetect();
      showMessage(`Queued ${result.queued} chapter detection job${result.queued !== 1 ? 's' : ''}`);
    } catch {
      showMessage('Failed to queue chapter detection jobs');
    } finally {
      bulkDetecting = false;
    }
  };

  const handleBulkRedetectChapters = async () => {
    bulkRedetectingSilence = true;
    try {
      const result = await bulkRedetectChapters();
      showMessage(
        `Queued ${result.queued} chapter re-detection job${result.queued !== 1 ? 's' : ''}`
      );
    } catch {
      showMessage('Failed to queue chapter re-detection jobs');
    } finally {
      bulkRedetectingSilence = false;
    }
  };

  const handleBulkEbookExtract = async () => {
    bulkExtracting = true;
    try {
      const result = await bulkEbookExtract();
      showMessage(`Queued ${result.queued} ebook extraction job${result.queued !== 1 ? 's' : ''}`);
    } catch {
      showMessage('Failed to queue ebook extraction jobs');
    } finally {
      bulkExtracting = false;
    }
  };

  const handleRepairEbooks = async () => {
    repairingEbooks = true;
    try {
      await repairEbooks();
      showMessage('Checking converted ebooks — see Jobs for what it finds');
    } catch {
      showMessage('Failed to queue the ebook repair sweep');
    } finally {
      repairingEbooks = false;
    }
  };

  const handleRepairAudiobookChapters = async () => {
    repairingChapters = true;
    try {
      await repairAudiobookChapters();
      showMessage('Checking audiobook editions and chapters — see Jobs for what it finds');
    } catch {
      showMessage('Failed to queue the audiobook repair sweep');
    } finally {
      repairingChapters = false;
    }
  };

  const handleKepubifySynced = async () => {
    kepubifying = true;
    try {
      const result = await kepubifySynced();
      showMessage(
        result.outstanding === 0
          ? 'Every synced book already has a KEPUB'
          : `Building ${result.outstanding} KEPUB${result.outstanding !== 1 ? 's' : ''} — those books will re-download`
      );
    } catch {
      showMessage('Failed to queue KEPUB conversion');
    } finally {
      kepubifying = false;
    }
  };

  const saveSystemConfig = async () => {
    if (!systemConfig) return;
    systemConfigSaving = true;
    systemConfigError = null;
    try {
      const updates: SystemConfigUpdate = {
        transcript_mode: systemConfig.transcript_mode,
        stt_service_url: systemConfig.stt_service_url,
        tts_enabled: systemConfig.tts_enabled,
        tts_service_url: systemConfig.tts_service_url,
        tts_service_type: systemConfig.tts_service_type,
        tts_default_voice: systemConfig.tts_default_voice,
        tts_default_temperature: systemConfig.tts_default_temperature,
        hw_encoder: systemConfig.hw_encoder,
        gpu_preempt_tts: systemConfig.gpu_preempt_tts,
        route_gpu_via_koblem: systemConfig.route_gpu_via_koblem
      };
      systemConfig = await updateSystemConfig(updates);
      showMessage('Configuration saved');
      // Refresh voice list in case TTS service URL changed
      getTtsStatus()
        .then((status) => {
          if (status.enabled && status.voices?.length) {
            ttsVoiceItems = status.voices.map((v) => ({
              value: v.id,
              label: v.name
            }));
          } else {
            ttsVoiceItems = [];
          }
        })
        .catch(() => {});
    } catch (e) {
      systemConfigError = e instanceof Error ? e.message : 'Failed to save config';
    } finally {
      systemConfigSaving = false;
    }
  };
</script>

<svelte:head>
  <title>Audio Settings — Kobbler</title>
</svelte:head>

{#if !auth.isAdmin}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <div class="audio-section">
    <div class="section-header">
      <div>
        <h2>Audio Settings</h2>
        <p class="section-subtitle">
          Text-to-speech, transcription, and video transcoding configuration
        </p>
      </div>
    </div>

    {#if systemConfigLoading}
      <div class="loading-card">Loading configuration...</div>
    {:else if systemConfigError}
      <div class="error">{systemConfigError}</div>
    {:else if systemConfig}
      <div class="config-form">
        <div class="config-card">
          <h3>Transcription</h3>
          <p class="field-hint" style="margin-bottom: 4px;">
            Speech-to-text for audiobooks via an OpenAI-compatible STT service. Transcripts enable
            search and text-based navigation in the player.
          </p>
          <div class="field" data-field="transcript_mode">
            <span class="field-label">Mode</span>
            <SearchSelect items={transcriptModeOptions} bind:value={systemConfig.transcript_mode} />
            <span class="field-hint"
              >Auto transcribes on ingest, manual requires triggering from the book page</span
            >
          </div>
          <div data-field="stt_service_url">
            <label>
              STT Service URL
              <input
                type="text"
                value={systemConfig.stt_service_url ?? ''}
                oninput={(e) => {
                  if (systemConfig)
                    systemConfig.stt_service_url = (e.target as HTMLInputElement).value || null;
                }}
                placeholder="e.g. http://stt-speaches:8000"
              />
              <span class="field-hint"
                >OpenAI-compatible STT service URL. See docker-compose.yml for service names.</span
              >
            </label>
          </div>
          <TranscriptRefresh transcriptionOff={systemConfig.transcript_mode === 'off'} />
          <div class="bulk-actions">
            <button
              type="button"
              class="btn-action"
              disabled={bulkDetecting}
              onclick={handleBulkChapterDetect}
            >
              {bulkDetecting ? 'Queuing...' : 'Detect Text Chapters'}
            </button>
            <button
              type="button"
              class="btn-action"
              disabled={bulkRedetectingSilence}
              onclick={handleBulkRedetectChapters}
            >
              {bulkRedetectingSilence ? 'Queuing...' : 'Detect Silence Chapters'}
            </button>
            <button
              type="button"
              class="btn-action"
              disabled={bulkExtracting}
              onclick={handleBulkEbookExtract}
            >
              {bulkExtracting ? 'Queuing...' : 'Extract Ebook Text'}
            </button>
            <button
              type="button"
              class="btn-action"
              disabled={repairingEbooks}
              onclick={handleRepairEbooks}
            >
              {repairingEbooks ? 'Queuing...' : 'Repair Converted Ebooks'}
            </button>
            <button
              type="button"
              class="btn-action"
              disabled={repairingChapters}
              onclick={handleRepairAudiobookChapters}
            >
              {repairingChapters ? 'Queuing...' : 'Repair Audiobooks'}
            </button>
            <button
              type="button"
              class="btn-action"
              disabled={kepubifying}
              onclick={handleKepubifySynced}
            >
              {kepubifying ? 'Queuing...' : 'Build KEPUBs for Synced Books'}
            </button>
          </div>
        </div>

        <div class="config-card">
          <h3>Text-to-Speech</h3>
          <p class="field-hint" style="margin-bottom: 4px;">
            Read-aloud for ebooks using an OpenAI-compatible TTS service (Kokoro, Chatterbox, etc).
            Enable a TTS service in docker-compose.yml first.
          </p>
          <div data-field="tts_enabled">
            <label class="checkbox-label">
              <input type="checkbox" bind:checked={systemConfig.tts_enabled} />
              Enable TTS
            </label>
          </div>
          <div data-field="tts_service_url">
            <label>
              TTS Service URL
              <input
                type="text"
                value={systemConfig.tts_service_url ?? ''}
                oninput={(e) => {
                  if (systemConfig)
                    systemConfig.tts_service_url = (e.target as HTMLInputElement).value || null;
                }}
                placeholder="http://tts:8880 (default)"
              />
              <span class="field-hint"
                >Used when NOT routing through koblem. Leave empty for Kokoro (http://tts:8880).
                Chatterbox: http://tts-chatterbox:8000. BreezeTTS 2: http://tts-breeze:8203.</span
              >
            </label>
          </div>
          <div class="field" data-field="tts_service_type">
            <span class="field-label">Service Type</span>
            <SearchSelect
              items={ttsServiceTypeOptions}
              value={systemConfig.tts_service_type ?? 'auto'}
              onchange={(v: string) => {
                if (systemConfig) systemConfig.tts_service_type = v;
              }}
            />
            <span class="field-hint"
              >Chatterbox and BreezeTTS 2 both support voice cloning and temperature control;
              BreezeTTS 2 also supports voice-design from a text description, and is the only engine
              with a forced aligner — so the reader's word highlight needs it. Auto probes the
              service on save.</span
            >
          </div>
          <div data-field="tts_default_voice">
            <label>
              Default Voice
              {#if ttsVoiceItems.length > 0}
                <MultiSearchSelect
                  items={ttsVoiceItems}
                  values={ttsSelectedVoices}
                  placeholder="Select voices..."
                  onchange={handleTtsVoiceChange}
                />
              {:else}
                <input
                  type="text"
                  value={systemConfig.tts_default_voice ?? ''}
                  oninput={(e) => {
                    if (systemConfig)
                      systemConfig.tts_default_voice = (e.target as HTMLInputElement).value || null;
                  }}
                  placeholder="af_heart"
                />
              {/if}
              <span class="field-hint"
                >Select one or more voices to blend. Multiple voices are mixed equally.</span
              >
            </label>
          </div>
          {#if supportsEmotion}
            <div data-field="tts_default_temperature">
              <label>
                Variation (Temperature)
                <div class="slider-row">
                  <input
                    type="range"
                    min="0"
                    max="1.5"
                    step="0.05"
                    value={systemConfig.tts_default_temperature ?? 0.8}
                    oninput={(e) => {
                      if (systemConfig)
                        systemConfig.tts_default_temperature = parseFloat(
                          (e.target as HTMLInputElement).value
                        );
                    }}
                  />
                  <span class="slider-value"
                    >{(systemConfig.tts_default_temperature ?? 0.8).toFixed(2)}</span
                  >
                </div>
                <span class="field-hint"
                  >Controls randomness in speech generation. Higher = more varied.</span
                >
              </label>
            </div>
          {/if}
          <div class="tts-test-row">
            {#if ttsTestPlaying}
              <button class="btn alt" type="button" onclick={stopTtsTest}>
                <span
                  class="material-icons-outlined"
                  style="font-size: 16px; vertical-align: middle;">stop</span
                >
                Stop
              </button>
            {:else}
              <button
                class="btn alt"
                type="button"
                onclick={testTtsVoice}
                disabled={ttsTestLoading || !systemConfig.tts_default_voice}
              >
                {#if ttsTestLoading}
                  Testing...
                {:else}
                  <span
                    class="material-icons-outlined"
                    style="font-size: 16px; vertical-align: middle;">play_arrow</span
                  >
                  Test Voice
                {/if}
              </button>
            {/if}
          </div>
        </div>

        <div class="config-card">
          <h3>Video Transcoding</h3>
          <p class="field-hint" style="margin-bottom: 4px;">
            Hardware-accelerated video encoding for streaming. Requires compatible GPU drivers.
          </p>
          <div class="field" data-field="hw_encoder">
            <span class="field-label">Hardware Encoder</span>
            <SearchSelect
              items={hwEncoderOptions}
              value={systemConfig.hw_encoder ?? 'auto'}
              onchange={(v: string) => {
                if (systemConfig) systemConfig.hw_encoder = v;
              }}
            />
            <span class="field-hint"
              >Auto detects available hardware encoders. Requires restart to take effect.</span
            >
          </div>
        </div>

        <div class="config-card">
          <h3>GPU Sharing</h3>
          <p class="field-hint" style="margin-bottom: 4px;">
            Controls how TTS, STT, and vision share the GPU when they run on the same card.
          </p>
          <div class="field" data-field="gpu_preempt_tts">
            <label class="toggle-label">
              <input
                type="checkbox"
                checked={systemConfig.gpu_preempt_tts}
                onchange={(e: Event) => {
                  if (systemConfig)
                    systemConfig.gpu_preempt_tts = (e.target as HTMLInputElement).checked;
                }}
              />
              <span class="field-label">TTS preempts STT</span>
            </label>
            <span class="field-hint"
              >When enabled, read-aloud pauses background transcription to free GPU VRAM. Disable if
              you have enough VRAM to run both concurrently (~7 GB for Chatterbox Turbo + Parakeet).</span
            >
          </div>
          {#if systemConfig.koblem_ui_enabled}
            <div class="field" data-field="route_gpu_via_koblem" style="margin-top: 12px;">
              <label class="toggle-label">
                <input
                  type="checkbox"
                  checked={systemConfig.route_gpu_via_koblem}
                  onchange={(e: Event) => {
                    if (systemConfig)
                      systemConfig.route_gpu_via_koblem = (e.target as HTMLInputElement).checked;
                  }}
                />
                <span class="field-label">Route GPU services through koblem</span>
              </label>
              <span class="field-hint"
                >When enabled (and <code>koblem_service_url</code> is configured), vision/TTS/STT requests
                go through koblem's priority GPU lock — so koblem's reviews, kobbler's skip-detect, TTS,
                and STT all serialise against one shared lock. Disable if running kobbler standalone (no
                koblem) — kobbler's local GpuLock still handles gating in that mode.</span
              >
            </div>
          {/if}
        </div>

        <div class="config-actions">
          <button
            class="btn"
            type="button"
            onclick={saveSystemConfig}
            disabled={systemConfigSaving}
          >
            {systemConfigSaving ? 'Saving...' : 'Save Configuration'}
          </button>
          <button
            class="btn alt"
            type="button"
            onclick={loadSystemConfig}
            disabled={systemConfigLoading}
          >
            Reset
          </button>
        </div>
      </div>
    {:else}
      <div class="muted">No configuration loaded</div>
    {/if}
  </div>
{/if}

<style>
  .permission-denied {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 4rem 2rem;
    color: var(--muted);
    text-align: center;
  }

  .permission-denied h2 {
    font-size: 1.25rem;
    margin-bottom: 0.5rem;
    color: var(--ink);
  }

  .audio-section {
    display: grid;
    gap: 16px;
  }

  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .section-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .section-subtitle {
    margin: 4px 0 0;
    font-size: 13px;
    color: var(--muted);
  }

  .config-form {
    display: grid;
    gap: 16px;
  }

  .config-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 16px;
    display: grid;
    gap: 12px;
  }

  .config-card h3 {
    margin: 0 0 4px;
    font-size: 15px;
    font-weight: 600;
  }

  .config-card label:not(.checkbox-label) {
    display: grid;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .config-card input[type='text'] {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .field {
    display: grid;
    gap: 6px;
  }

  .field-label {
    font-size: 12px;
    color: var(--muted);
  }

  .field-hint {
    font-size: 11px;
    color: var(--muted);
  }

  .bulk-actions {
    display: flex;
    gap: 8px;
    padding-top: 4px;
  }

  .btn-action {
    padding: 6px 12px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
  }

  .btn-action:hover:not(:disabled) {
    background: var(--ring);
  }

  .btn-action:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .config-actions {
    display: flex;
    gap: 12px;
    padding-top: 8px;
  }

  .btn {
    padding: 8px 16px;
    border-radius: 12px;
    border: none;
    background: var(--accent);
    color: white;
    font-weight: 600;
    font-size: 13px;
    cursor: pointer;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn.alt {
    background: var(--card);
    border: 1px solid var(--ring);
    color: var(--ink);
  }

  .loading-card {
    padding: 24px;
    text-align: center;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
  }

  .error {
    padding: 16px;
    text-align: center;
    border-radius: 12px;
    border: 1px solid var(--danger);
    background: var(--card);
    color: var(--danger);
  }

  .tts-test-row {
    margin-top: 8px;
  }

  .toggle-label {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
  }

  .toggle-label input[type='checkbox'] {
    width: 16px;
    height: 16px;
    cursor: pointer;
  }

  .slider-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .slider-row input[type='range'] {
    flex: 1;
  }

  .slider-value {
    font-size: 12px;
    min-width: 32px;
    text-align: right;
    color: var(--ink);
  }
</style>
