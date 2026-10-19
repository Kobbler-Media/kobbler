<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import {
    SearchSelect,
    WaveformTrim,
    blobToMonoSamples,
    createFaPlayback,
    detectBestWindow,
    FaTranscript,
    parseKobblerNdjson,
    type FaEvent,
    type FaPlayback
  } from '@koblibs/ui';
  import { showMessage } from '$lib/context/app.svelte';
  import MetaLine from '$lib/components/shared/MetaLine.svelte';
  import {
    listVoicePresets,
    createVoicePreset,
    updateVoicePreset,
    deleteVoicePreset,
    uploadVoiceClip,
    deleteVoiceClip,
    getVoiceClipUrl,
    getActivePreset,
    setActivePreset,
    getTtsStatus,
    synthesizeTts,
    searchAudiobooks,
    getAudiobook,
    suggestAudiobookClip,
    audiobookClipPreviewUrl,
    createPresetClipFromAudiobook,
    transcribePresetClip,
    previewTtsSample,
    type UserVoicePreset,
    type TtsVoice,
    type TtsStatus,
    type AudiobookSummary,
    type AudiobookDetail
  } from '$lib/api/client';

  const MAX_PRESETS = 10;
  const TTS_TEST_TEXT =
    'The quick brown fox jumps over the lazy dog. Every voice has its own character and warmth.';

  let presets = $state<UserVoicePreset[]>([]);
  let activePresetId = $state<string | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);

  // TTS voices from service
  let ttsVoices = $state<{ value: string; label: string }[]>([]);
  let supportsEmotion = $state(false);
  /// Backend label from /api/v1/admin/tts/status — drives engine-specific UI.
  let engine = $state<TtsStatus['engine']>('auto');
  let isBreeze = $derived(engine === 'breeze');

  // Editor state
  let editingId = $state<string | null>(null);
  let creating = $state(false);
  let saving = $state(false);

  // Form fields
  let formName = $state('');
  let formVoice = $state<string>('');
  let formSpeed = $state(1.0);

  let formTemperature = $state<number | null>(null);

  // Engine-native form fields. `null` = use service default (don't persist).
  let formRefText = $state<string>('');
  let formInstructions = $state<string>('');
  /// Built-in narrator token. Empty = the engine's own default.
  let formSpeaker = $state<string>('');
  const SPEAKERS = ['S0', 'S1', 'S2', 'S3', 'S4', 'S5', 'S6', 'S7', 'S8', 'S9'];
  let formSeed = $state<number | null>(null);
  let formTopP = $state<number | null>(null);
  let formTopK = $state<number | null>(null);
  let formRepetitionPenalty = $state<number | null>(null);
  let formMaxNewFrames = $state<number | null>(null);

  let transcribingClip = $state(false);

  // Seed audition state — pre-save flow that fires three previewTtsSample
  // calls with different seeds and lets the user pick one.
  type AuditionSample = { seed: number; url: string };
  let auditionText = $state(
    'The quick brown fox jumps over the lazy dog. Every voice has its own character and warmth.'
  );
  let auditionSamples = $state<AuditionSample[]>([]);
  let auditioning = $state(false);

  // Clip upload
  let clipFile = $state<File | null>(null);
  let clipInputEl: HTMLInputElement | undefined = $state(undefined);

  // Audiobook clip source. `'description'` is breeze-only — voice-design from
  // a text prompt, no reference audio needed.
  //
  // Flow: pick book → pick chapter → fetch a 60s WAV preview at a sensible
  // offset → decode in the browser → drag a ≤AB_MAX_TRIM_SECONDS window over
  // a clean passage (ms-precision, snap-to-silence on release). On Save we
  // send the absolute offset+duration to the existing clip-cutter endpoint.
  type ClipSource = 'file' | 'audiobook' | 'description';
  const AB_PREVIEW_SECONDS = 60;
  let clipSource = $state<ClipSource>('file');
  let abQuery = $state('');
  let abResults = $state<AudiobookSummary[]>([]);
  let abSearching = $state(false);
  let abSelected = $state<AudiobookDetail | null>(null);
  let abChapterIndex = $state<number | null>(null);
  let abPreviewOffset = $state(0);
  let abSamples: Float32Array | null = $state(null);
  let abSampleRate = $state(0);
  let abTrimStart = $state(0);
  let abTrimEnd = $state(0);
  let abFetchingPreview = $state(false);
  /// Cap on trim selection — 10s for breeze (its reference budget),
  /// 30s for chatterbox where longer is fine.
  let abMaxTrimSeconds = $derived(isBreeze ? 10 : 30);
  /// Absolute offset/duration computed from the trim handles, in seconds.
  /// These are what we send to createPresetClipFromAudiobook on save.
  let abAbsoluteOffsetSeconds = $derived(
    abSampleRate > 0 ? abPreviewOffset + abTrimStart / abSampleRate : 0
  );
  let abAbsoluteDurationSeconds = $derived(
    abSampleRate > 0 ? (abTrimEnd - abTrimStart) / abSampleRate : 0
  );

  // Test playback — driven by the shared FA helper. activeWordIndex stays -1
  // for engines without forced alignment (chatterbox/kokoro/openai) or when
  // the user disables the FA highlight toggle.
  let testFa = $state<FaPlayback | null>(null);
  let testText = $state(TTS_TEST_TEXT);
  let faHighlightEnabled = $state(true);
  let testShowTimings = $state(false);
  let testError = $state<string | null>(null);
  let testLoading = $state(false);
  let testPlaying = $derived(
    testFa != null && (testFa.state === 'playing' || testFa.state === 'paused')
  );
  let testing = $derived(testLoading || testPlaying);
  /// Test button is enabled when there's *something* to synthesise: either a
  /// real voice id, or a non-empty voice-description (breeze-only path).
  let canTest = $derived(
    !!formVoice || (isBreeze && clipSource === 'description' && formInstructions.trim().length > 0)
  );

  let atLimit = $derived(presets.length >= MAX_PRESETS);

  onMount(async () => {
    await loadData();
    getTtsStatus()
      .then((status) => {
        if (status.enabled && status.voices?.length) {
          ttsVoices = status.voices.map((v: TtsVoice) => ({ value: v.id, label: v.name }));
        }
        supportsEmotion = status.supports_emotion ?? false;
        engine = status.engine ?? 'auto';
      })
      .catch(() => {});
  });

  onDestroy(() => {
    stopTest();
    cleanupAuditionSamples();
  });

  async function loadData() {
    loading = true;
    error = null;
    try {
      const [list, active] = await Promise.all([listVoicePresets(), getActivePreset()]);
      presets = list;
      activePresetId = active?.id ?? null;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load presets';
    } finally {
      loading = false;
    }
  }

  function resetForm() {
    formName = '';
    formVoice = '';
    formSpeed = 1.0;
    formTemperature = null;
    formRefText = '';
    formInstructions = '';
    formSpeaker = '';
    formSeed = null;
    formTopP = null;
    formTopK = null;
    formRepetitionPenalty = null;
    formMaxNewFrames = null;
    clipFile = null;
    clipSource = 'file';
    abQuery = '';
    abResults = [];
    abSelected = null;
    abChapterIndex = null;
    abPreviewOffset = 0;
    abSamples = null;
    abSampleRate = 0;
    abTrimStart = 0;
    abTrimEnd = 0;
    cleanupAuditionSamples();
  }

  function startCreate() {
    editingId = null;
    creating = true;
    resetForm();
  }

  function startEdit(preset: UserVoicePreset) {
    creating = false;
    editingId = preset.id;
    formName = preset.name;
    formVoice = preset.voice ?? '';
    formSpeed = preset.speed;
    formTemperature = preset.temperature;
    formRefText = preset.ref_text ?? '';
    formInstructions = preset.instructions ?? '';
    formSpeaker = preset.speaker ?? '';
    formSeed = preset.seed;
    formTopP = preset.top_p;
    formTopK = preset.top_k;
    formRepetitionPenalty = preset.repetition_penalty;
    formMaxNewFrames = preset.max_new_frames;
    clipFile = null;
    // Recover the original clip-source path so the form opens on the
    // section the user last used.
    if (preset.instructions) {
      clipSource = 'description';
    } else {
      clipSource = preset.clip_filename ? 'file' : 'file';
    }
    cleanupAuditionSamples();
  }

  function cancelEdit() {
    stopTest();
    editingId = null;
    creating = false;
    resetForm();
  }

  async function applyClipSource(presetId: string) {
    if (clipSource === 'file' && clipFile) {
      await uploadVoiceClip(presetId, clipFile);
    } else if (
      clipSource === 'audiobook' &&
      abSelected &&
      abChapterIndex != null &&
      abAbsoluteDurationSeconds > 0
    ) {
      await createPresetClipFromAudiobook(presetId, {
        audiobookId: abSelected.id,
        chapterIndex: abChapterIndex,
        offsetSeconds: abAbsoluteOffsetSeconds,
        durationSeconds: abAbsoluteDurationSeconds
      });
    }
    // 'description' path needs no clip — the instructions field is persisted
    // via createVoicePreset/updateVoicePreset directly.
  }

  function buildPresetPayload() {
    // Voice-design path: voice="default" implicit, no clip.
    const isDescription = isBreeze && clipSource === 'description';
    const voice = isDescription ? 'default' : formVoice || null;
    const instructions = isDescription ? formInstructions.trim() || null : null;

    // ref_text only makes sense on breeze with a clip.
    const refText = isBreeze && clipSource !== 'description' ? formRefText.trim() || null : null;

    return {
      name: formName.trim(),
      voice,
      speed: formSpeed,
      temperature: formTemperature,
      ref_text: refText,
      instructions,
      speaker: isBreeze ? formSpeaker.trim() || null : null,
      seed: isBreeze ? formSeed : null,
      top_p: isBreeze ? formTopP : null,
      top_k: isBreeze ? formTopK : null,
      repetition_penalty: isBreeze ? formRepetitionPenalty : null,
      max_new_frames: isBreeze ? formMaxNewFrames : null
    };
  }

  async function handleSave() {
    if (!formName.trim()) {
      showMessage('Name is required');
      return;
    }
    saving = true;
    try {
      const payload = buildPresetPayload();
      // Whether we should kick off STT after saving the clip to autofill
      // ref_text — only for the breeze audiobook flow, only if the user didn't
      // type ref_text themselves. Captured before resetForm clears state.
      const shouldAutoTranscribe =
        isBreeze && clipSource === 'audiobook' && !!abSelected && !payload.ref_text;

      let presetId: string | null = null;
      if (creating) {
        const preset = await createVoicePreset({ ...payload, name: payload.name });
        await applyClipSource(preset.id);
        presetId = preset.id;
        showMessage('Preset created');
      } else if (editingId) {
        await updateVoicePreset(editingId, payload);
        await applyClipSource(editingId);
        presetId = editingId;
        showMessage('Preset updated');
      }

      // STT runs against the saved WAV so the user picks up an
      // ICL-ready ref_text without a second round-trip. Best-effort —
      // a failure here doesn't unwind the preset save.
      if (presetId && shouldAutoTranscribe) {
        try {
          const { text } = await transcribePresetClip(presetId);
          if (text.trim()) {
            await updateVoicePreset(presetId, { ref_text: text });
          }
        } catch (e) {
          console.warn('Auto-transcribe failed', e);
        }
      }
      cancelEdit();
      await loadData();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to save preset');
    } finally {
      saving = false;
    }
  }

  async function handleAutoTranscribe(presetId: string) {
    transcribingClip = true;
    try {
      const { text } = await transcribePresetClip(presetId);
      formRefText = text;
      showMessage('Transcribed clip — ref_text autofilled');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Transcribe failed');
    } finally {
      transcribingClip = false;
    }
  }

  function cleanupAuditionSamples() {
    for (const s of auditionSamples) URL.revokeObjectURL(s.url);
    auditionSamples = [];
  }

  async function handleSeedAudition() {
    if (!auditionText.trim()) return;
    auditioning = true;
    cleanupAuditionSamples();
    // Three random seeds. Keep them in a recognisable range so the user can
    // jot one down without dealing with 18-digit values.
    const seeds = [
      Math.floor(Math.random() * 1_000_000) + 1,
      Math.floor(Math.random() * 1_000_000) + 1,
      Math.floor(Math.random() * 1_000_000) + 1
    ];
    try {
      const blobs = await Promise.all(
        seeds.map((seed) =>
          previewTtsSample(auditionText.trim(), {
            // Voice-design means NO cloned voice — there is no "default"
            // clone to name, and passing one would override the description.
            voice: clipSource === 'description' ? undefined : formVoice || undefined,
            speed: formSpeed,
            temperature: formTemperature ?? undefined,
            top_p: formTopP ?? undefined,
            top_k: formTopK ?? undefined,
            repetition_penalty: formRepetitionPenalty ?? undefined,
            seed,
            instructions:
              clipSource === 'description' ? formInstructions.trim() || undefined : undefined,
            speaker: formSpeaker.trim() || undefined,
            max_new_frames: formMaxNewFrames ?? undefined
          })
        )
      );
      auditionSamples = blobs.map((sample, i) => ({
        seed: seeds[i],
        url: URL.createObjectURL(sample.blob)
      }));
      noteRefusedSpeed(blobs[0]?.speedApplied);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Audition failed');
    } finally {
      auditioning = false;
    }
  }

  function applyAuditionSeed(seed: number) {
    formSeed = seed;
    showMessage(`Seed ${seed} selected — Save to lock it in.`);
  }

  async function handleAbSearch() {
    const q = abQuery.trim();
    if (!q) return;
    abSearching = true;
    try {
      abResults = await searchAudiobooks(q);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Audiobook search failed');
    } finally {
      abSearching = false;
    }
  }

  async function handleAbSelectBook(id: string) {
    try {
      const detail = await getAudiobook(id);
      abSelected = detail;
      abSamples = null;
      // Default to second chapter when there are 3+ chapters (skip likely intro).
      const defaultIdx = detail.chapters.length >= 3 ? 1 : 0;
      await handleAbPickChapter(defaultIdx);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load audiobook');
    }
  }

  async function handleAbPickChapter(idx: number) {
    if (!abSelected) return;
    abChapterIndex = idx;
    const ch = abSelected.chapters[idx];
    // Server-side `suggestAudiobookClip` picks the peakiest 30s window in the
    // chapter — a great starting offset for the 60s preview pan.
    let suggested = 0;
    try {
      const res = await suggestAudiobookClip({
        audiobookId: abSelected.id,
        chapterIndex: idx,
        durationSeconds: AB_PREVIEW_SECONDS
      });
      suggested = Math.max(0, Math.round(res.offset_seconds));
    } catch {
      // If the peaky pick fails, fall back to 30s in to skip credits/title.
      suggested = ch?.duration_seconds != null && ch.duration_seconds > AB_PREVIEW_SECONDS ? 30 : 0;
    }
    await handleAbFetchPreviewAt(suggested);
  }

  async function handleAbFetchPreviewAt(offsetSeconds: number) {
    if (!abSelected || abChapterIndex == null) return;
    abFetchingPreview = true;
    try {
      const url = audiobookClipPreviewUrl({
        audiobookId: abSelected.id,
        chapterIndex: abChapterIndex,
        offsetSeconds,
        durationSeconds: AB_PREVIEW_SECONDS
      });
      const resp = await fetch(url, { credentials: 'include' });
      if (!resp.ok) throw new Error(`Preview fetch failed: ${resp.status}`);
      const blob = await resp.blob();
      const { samples, sampleRate } = await blobToMonoSamples(blob);
      abPreviewOffset = offsetSeconds;
      abSamples = samples;
      abSampleRate = sampleRate;
      const { start, end } = detectBestWindow(samples, sampleRate, abMaxTrimSeconds);
      abTrimStart = start;
      abTrimEnd = end;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Preview fetch failed');
    } finally {
      abFetchingPreview = false;
    }
  }

  async function handleDelete(preset: UserVoicePreset) {
    if (!confirm(`Delete preset "${preset.name}"?`)) return;
    try {
      await deleteVoicePreset(preset.id);
      showMessage('Preset deleted');
      if (editingId === preset.id) cancelEdit();
      await loadData();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete preset');
    }
  }

  async function handleSetActive(presetId: string | null) {
    try {
      await setActivePreset(presetId);
      activePresetId = presetId;
      showMessage(presetId ? 'Active preset updated' : 'Active preset cleared');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to set active preset');
    }
  }

  async function handleDeleteClip(presetId: string) {
    try {
      await deleteVoiceClip(presetId);
      showMessage('Voice clip deleted');
      await loadData();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete clip');
    }
  }

  /// Track which preset's ref_text re-transcription is in flight so the
  /// per-card button can show a spinner without overwriting the editing
  /// form's `transcribingClip` state.
  let retranscribingPresetId = $state<string | null>(null);

  /// One-click ICL backfill for chatterbox-era presets that have a clip but
  /// no ref_text — needed to clone with high fidelity. Runs STT
  /// against the saved clip, persists the transcript, refreshes the list.
  async function handleRetranscribeForIcl(preset: UserVoicePreset) {
    retranscribingPresetId = preset.id;
    try {
      const { text } = await transcribePresetClip(preset.id);
      await updateVoicePreset(preset.id, { ref_text: text });
      showMessage('ICL ref_text saved');
      await loadData();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Re-transcribe failed');
    } finally {
      retranscribingPresetId = null;
    }
  }

  /// True when a preset has a clip but no ref_text AND the active engine is
  /// breeze — i.e. it would clone with degraded fidelity until backfilled.
  function needsIclBackfill(preset: UserVoicePreset): boolean {
    return isBreeze && !!preset.clip_filename && !preset.ref_text;
  }

  /// Watch the stream go by for the rate it was actually rendered at. Passing
  /// through rather than reading it off the finished player means the form is
  /// corrected as the first audio lands, not after the clip has played out.
  async function* tapRenderedSpeed(src: AsyncIterable<FaEvent>): AsyncGenerator<FaEvent> {
    for await (const ev of src) {
      if (ev.type === 'paragraph') noteRefusedSpeed(ev.speed);
      yield ev;
    }
  }

  /// A refused rate returns real audio at 1x, which is indistinguishable from
  /// success unless it is said out loud. Snap the form to what was rendered so
  /// the saved preset can't claim a rate the engine won't honour.
  function noteRefusedSpeed(applied: number | undefined) {
    if (applied == null || !Number.isFinite(applied) || applied <= 0) return;
    if (Math.abs(applied - formSpeed) < 1e-9) return;
    formSpeed = applied;
    showMessage(`The voice engine can't do that speed — rendered at ${applied}x.`);
  }

  function stopTest() {
    testFa?.stop();
    testFa = null;
    testLoading = false;
    testError = null;
  }

  /// Build a single-shot FaEvent stream from a preview-sample MP3 blob. Used
  /// for the description (voice-design) preview, which has no streaming/FA
  /// endpoint — we wrap the full clip in synthetic paragraph + audio + done
  /// events so the same FaPlayback controller drives it.
  async function* blobAsEvents(blob: Blob, text: string, speed: number): AsyncGenerator<FaEvent> {
    yield {
      type: 'paragraph',
      paragraphIndex: 0,
      wordOffset: 0,
      audioOffsetMs: 0,
      sampleRate: 0,
      text,
      speed
    };
    const ab = await blob.arrayBuffer();
    yield {
      type: 'audio',
      paragraphIndex: 0,
      format: 'mp3',
      pcm: new Uint8Array(ab),
      sampleRate: 0
    };
    yield { type: 'done' };
  }

  async function playPreview() {
    stopTest();
    const text = testText.trim() || TTS_TEST_TEXT;
    testLoading = true;
    testError = null;
    const abortController = new AbortController();

    try {
      let events: AsyncIterable<FaEvent>;

      if (clipSource === 'description') {
        if (!formInstructions.trim()) {
          showMessage('Enter a voice description to test');
          testLoading = false;
          return;
        }
        const sample = await previewTtsSample(
          text,
          {
            voice: undefined,
            speed: formSpeed,
            instructions: formInstructions.trim(),
            temperature: formTemperature ?? undefined,
            top_p: formTopP ?? undefined,
            top_k: formTopK ?? undefined,
            repetition_penalty: formRepetitionPenalty ?? undefined,
            seed: formSeed ?? undefined,
            speaker: formSpeaker.trim() || undefined,
            max_new_frames: formMaxNewFrames ?? undefined
          },
          abortController.signal
        );
        noteRefusedSpeed(sample.speedApplied);
        events = blobAsEvents(sample.blob, text, sample.speedApplied);
      } else {
        const voice = formVoice || undefined;
        if (!voice) {
          showMessage('Select a voice to test');
          testLoading = false;
          return;
        }
        const resp = await synthesizeTts(
          text,
          voice,
          { temperature: formTemperature ?? undefined, speed: formSpeed },
          abortController.signal
        );
        if (!resp.ok || !resp.body) throw new Error(`TTS failed: ${resp.status}`);
        events = tapRenderedSpeed(parseKobblerNdjson(resp.body, { text }));
      }

      const fa = createFaPlayback({
        events,
        text,
        abortController,
        onError: (m) => (testError = m)
      });
      testFa = fa;
      testLoading = false;
      await fa.start();
    } catch (e) {
      testLoading = false;
      // Manual stop aborts the fetch; that's not an error we need to surface.
      if (e instanceof DOMException && e.name === 'AbortError') return;
      const msg = e instanceof Error ? e.message : 'Test failed';
      testError = msg;
      showMessage(msg);
    }
  }

  async function handleTest() {
    await playPreview();
  }

  function voiceLabel(preset: UserVoicePreset): string {
    if (preset.clip_filename) return `Custom clip: ${preset.clip_filename}`;
    if (preset.voice) {
      const v = ttsVoices.find((tv) => tv.value === preset.voice);
      return v ? v.label : preset.voice;
    }
    return 'No voice selected';
  }

  function paramSummary(preset: UserVoicePreset): string[] {
    const parts: string[] = [];
    if (preset.temperature != null) parts.push(`Variation: ${preset.temperature.toFixed(2)}`);
    if (preset.instructions) parts.push('Voice description');
    if (preset.ref_text) parts.push('ICL ref-text');
    if (preset.seed != null) parts.push(`Seed: ${preset.seed}`);
    if (preset.top_p != null) parts.push(`top_p: ${preset.top_p.toFixed(2)}`);
    if (preset.top_k != null) parts.push(`top_k: ${preset.top_k}`);
    if (preset.repetition_penalty != null)
      parts.push(`rep penalty: ${preset.repetition_penalty.toFixed(2)}`);
    if (preset.max_new_frames != null) parts.push(`max frames: ${preset.max_new_frames}`);
    if (preset.speaker) parts.push(`Speaker: ${preset.speaker}`);
    return parts;
  }
</script>

<svelte:head>
  <title>Voice Settings — Kobbler</title>
</svelte:head>

{#snippet voicePickerField(hasClip: boolean)}
  <div class="form-field">
    <span class="form-label-text">Voice</span>
    {#if ttsVoices.length > 0}
      <SearchSelect
        items={ttsVoices}
        value={formVoice}
        disabled={hasClip}
        onchange={(v: string) => {
          formVoice = v;
        }}
      />
    {:else}
      <input
        type="text"
        bind:value={formVoice}
        placeholder="Voice ID (e.g. af_heart)"
        disabled={hasClip}
      />
    {/if}
    {#if hasClip}
      <span class="form-hint"
        >Custom clip overrides this — built-in voice has no effect when a clip is attached.</span
      >
    {/if}
  </div>
{/snippet}

{#snippet voiceClipPicker()}
  <div class="form-field">
    <span class="form-label-text">Voice Clip Source</span>
    <div class="clip-source-tabs">
      <label class="clip-source-tab">
        <input type="radio" bind:group={clipSource} value="file" />
        <span>Upload file</span>
      </label>
      <label class="clip-source-tab">
        <input type="radio" bind:group={clipSource} value="audiobook" />
        <span>From audiobook</span>
      </label>
      {#if isBreeze}
        <label class="clip-source-tab">
          <input type="radio" bind:group={clipSource} value="description" />
          <span>Voice description</span>
        </label>
      {/if}
    </div>

    {#if clipSource === 'description'}
      <div class="form-field">
        <textarea
          rows="3"
          bind:value={formInstructions}
          placeholder="A warm, friendly young woman with a gentle American accent."></textarea>
        <span class="form-hint">
          Describe the voice in plain English. No reference clip needed.
        </span>
      </div>
    {/if}

    {#if clipSource === 'file'}
      <div class="clip-upload-row">
        <input
          type="file"
          accept="audio/*"
          bind:this={clipInputEl}
          onchange={(e) => {
            const target = e.target as HTMLInputElement;
            clipFile = target.files?.[0] ?? null;
          }}
        />
      </div>
      <span class="form-hint">Upload a reference audio clip for voice cloning.</span>
    {:else if clipSource === 'audiobook'}
      <div class="audiobook-picker">
        <div class="ab-search-row">
          <input
            type="text"
            bind:value={abQuery}
            placeholder="Search audiobook..."
            onkeydown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                handleAbSearch();
              }
            }}
          />
          <button
            class="btn alt small"
            type="button"
            disabled={abSearching}
            onclick={handleAbSearch}
          >
            {abSearching ? 'Searching...' : 'Search'}
          </button>
        </div>
        {#if abResults.length > 0 && !abSelected}
          <ul class="ab-results">
            {#each abResults as r (r.id)}
              <li>
                <button
                  type="button"
                  class="ab-result-btn"
                  onclick={() => handleAbSelectBook(r.id)}
                >
                  <span class="ab-result-title">{r.title}</span>
                  {#if r.author}<span class="ab-result-author">— {r.author}</span>{/if}
                </button>
              </li>
            {/each}
          </ul>
        {/if}
        {#if abSelected}
          <div class="ab-selected">
            <div class="ab-selected-header">
              <div class="ab-selected-title">
                <MetaLine>
                  <span>
                    <strong>{abSelected.title}</strong>
                    {#if abSelected.author}<span class="muted"> — {abSelected.author}</span>{/if}
                  </span>
                  {#if abSelected.narrator}
                    <span class="muted small">narrated by {abSelected.narrator}</span>
                  {/if}
                </MetaLine>
              </div>
              <button
                type="button"
                class="btn-icon"
                title="Pick a different audiobook"
                onclick={() => {
                  abSelected = null;
                  abSamples = null;
                  abChapterIndex = null;
                  abResults = [];
                }}
              >
                <span class="material-icons-outlined" style="font-size: 16px;">close</span>
              </button>
            </div>

            <div class="ab-chapter-list">
              {#each abSelected.chapters as ch, i (ch.index)}
                <button
                  type="button"
                  class="ab-chapter-btn"
                  class:active={abChapterIndex === i}
                  onclick={() => handleAbPickChapter(i)}
                  disabled={abFetchingPreview}
                >
                  <span class="ab-chapter-title">{ch.title}</span>
                  {#if ch.duration_seconds != null}
                    <span class="muted small">
                      {Math.floor(ch.duration_seconds / 60)}:{String(
                        Math.round(ch.duration_seconds % 60)
                      ).padStart(2, '0')}
                    </span>
                  {/if}
                </button>
              {/each}
            </div>

            {#if abFetchingPreview}
              <div class="ab-preview-status">
                <span class="muted">Fetching {AB_PREVIEW_SECONDS}s preview…</span>
              </div>
            {:else if abSamples}
              <div class="ab-trim-block">
                <div class="ab-pan-row">
                  <span class="muted small">
                    {AB_PREVIEW_SECONDS}s window from {Math.floor(abPreviewOffset / 60)}:{String(
                      Math.floor(abPreviewOffset % 60)
                    ).padStart(2, '0')}.
                  </span>
                  <button
                    type="button"
                    class="link-btn"
                    onclick={() =>
                      handleAbFetchPreviewAt(Math.max(0, abPreviewOffset - AB_PREVIEW_SECONDS))}
                    disabled={abPreviewOffset <= 0}
                  >
                    ← prev
                  </button>
                  <button
                    type="button"
                    class="link-btn"
                    onclick={() => handleAbFetchPreviewAt(abPreviewOffset + AB_PREVIEW_SECONDS)}
                  >
                    next →
                  </button>
                </div>
                <WaveformTrim
                  samples={abSamples}
                  sampleRate={abSampleRate}
                  maxSeconds={abMaxTrimSeconds}
                  initialStart={abTrimStart}
                  initialEnd={abTrimEnd}
                  onchange={(s, e) => {
                    abTrimStart = s;
                    abTrimEnd = e;
                  }}
                />
                <span class="form-hint">
                  Drag the handles to trim. {isBreeze
                    ? '~10s of clean speech works best for voice cloning.'
                    : ''}
                </span>
              </div>
            {/if}
          </div>
        {/if}
      </div>
    {/if}
  </div>
{/snippet}

{#snippet refTextField(presetId: string | null, hasClip: boolean)}
  {#if isBreeze && clipSource !== 'description' && hasClip}
    <div class="form-field">
      <div class="label-with-reset">
        <span class="form-label-text">Reference transcript (ref_text)</span>
        {#if presetId}
          <button
            class="reset-btn"
            type="button"
            disabled={transcribingClip}
            title="Auto-transcribe via STT"
            onclick={() => handleAutoTranscribe(presetId)}
          >
            <span class="material-icons-outlined" style="font-size: 14px;">
              {transcribingClip ? 'hourglass_top' : 'transcribe'}
            </span>
          </button>
        {/if}
      </div>
      <textarea rows="2" bind:value={formRefText} placeholder="Known transcript of the clip"
      ></textarea>
      <span class="form-hint">
        Optional but improves clone fidelity (reference transcript). Save the preset first to enable
        auto-transcribe.
      </span>
    </div>
  {/if}
{/snippet}

{#snippet previewPanel()}
  <div class="preview-panel">
    <div class="preview-controls">
      <label class="form-label">
        <span class="form-label-text">Preview text</span>
        <textarea rows="2" bind:value={testText}></textarea>
      </label>
      <label class="toggle-inline">
        <input type="checkbox" bind:checked={faHighlightEnabled} disabled={!isBreeze} />
        <span>FA word highlight</span>
        {#if !isBreeze}
          <span class="form-hint">— breeze only</span>
        {/if}
      </label>
    </div>
    {#if testFa}
      <div class="preview-transcript">
        <FaTranscript
          text={testText}
          activeWordIndex={faHighlightEnabled ? testFa.activeWordIndex : -1}
        />
      </div>
      <div class="preview-meta">
        <MetaLine>
          <span class="preview-state">{testFa.state}</span>
          {#if testFa.words.length > 0}
            <span>{testFa.words.length} words</span>
          {/if}
          {#if testFa.timings.ttfaMs != null}
            <span>TTFA {Math.round(testFa.timings.ttfaMs)} ms</span>
          {/if}
          {#if testFa.timings.firstAlignMs != null}
            <span>first align {Math.round(testFa.timings.firstAlignMs)} ms</span>
          {/if}
          {#if testFa.timings.audioMs != null}
            <span>audio {(testFa.timings.audioMs / 1000).toFixed(2)} s</span>
          {/if}
        </MetaLine>
        {#if testFa.state === 'playing' || testFa.state === 'paused'}
          <button class="btn alt small" type="button" onclick={stopTest}>Stop</button>
        {/if}
      </div>
      {#if testFa.words.length > 0}
        <details bind:open={testShowTimings} class="advanced-details">
          <summary>Per-word timings</summary>
          <table class="word-table">
            <thead>
              <tr><th>idx</th><th>text</th><th>t0 (ms)</th><th>t1 (ms)</th></tr>
            </thead>
            <tbody>
              {#each testFa.words as w (w.globalWordIndex)}
                <tr class:active={w.globalWordIndex === testFa.activeWordIndex}>
                  <td>{w.globalWordIndex}</td>
                  <td>{w.text}</td>
                  <td>{w.t0Ms}</td>
                  <td>{w.t1Ms}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </details>
      {/if}
    {/if}
    {#if testError}
      <div class="preview-error">{testError}</div>
    {/if}
  </div>
{/snippet}

{#snippet seedAudition()}
  {#if isBreeze}
    <details class="advanced-details">
      <summary>Audition seeds</summary>
      <div class="advanced-body">
        <label class="form-label">
          Test text
          <textarea rows="2" bind:value={auditionText}></textarea>
        </label>
        <div class="editor-actions">
          <button
            class="btn alt small"
            type="button"
            disabled={auditioning || !auditionText.trim()}
            onclick={handleSeedAudition}
          >
            {auditioning ? 'Generating...' : 'Generate 3 samples'}
          </button>
          {#if formSeed != null}
            <span class="form-hint">Saved seed: {formSeed}</span>
            <button
              class="btn alt small"
              type="button"
              onclick={() => {
                formSeed = null;
              }}
            >
              Clear
            </button>
          {/if}
        </div>
        {#if auditionSamples.length > 0}
          <div class="audition-samples">
            {#each auditionSamples as s (s.seed)}
              <div class="audition-sample">
                <!-- Rendered at the preset's speed server-side, so the element
                     plays it back untouched. -->
                <audio controls preload="auto" src={s.url}></audio>
                <button
                  class="btn alt small"
                  type="button"
                  onclick={() => applyAuditionSeed(s.seed)}
                  disabled={formSeed === s.seed}
                >
                  {formSeed === s.seed ? `Picked (${s.seed})` : `Use seed ${s.seed}`}
                </button>
              </div>
            {/each}
          </div>
        {/if}
      </div>
    </details>
  {/if}
{/snippet}

{#snippet advancedKnobs()}
  {#if isBreeze}
    <details class="advanced-details">
      <summary>Advanced</summary>
      <div class="advanced-body">
        <div class="form-field">
          <label class="form-label">
            Speaker
            <select bind:value={formSpeaker}>
              <option value="">engine default</option>
              {#each SPEAKERS as sp (sp)}
                <option value={sp}>{sp}</option>
              {/each}
            </select>
          </label>
          <p class="field-hint">
            Built-in narrator token. Changes the default voice; for a stable one, clone a voice
            above.
          </p>
        </div>
        <div class="form-field">
          <div class="label-with-reset">
            <span class="form-label-text">top_p</span>
            {#if formTopP != null}
              <button
                class="reset-btn"
                type="button"
                onclick={() => {
                  formTopP = null;
                }}
                title="Reset to default"
              >
                <span class="material-icons-outlined" style="font-size: 14px;">restart_alt</span>
              </button>
            {/if}
          </div>
          <div class="slider-row">
            <input
              type="range"
              min="0.1"
              max="1.0"
              step="0.05"
              value={formTopP ?? 0.95}
              oninput={(e) => {
                formTopP = parseFloat((e.target as HTMLInputElement).value);
              }}
            />
            <span class="slider-value">{formTopP != null ? formTopP.toFixed(2) : 'default'}</span>
          </div>
        </div>
        <div class="form-field">
          <div class="label-with-reset">
            <span class="form-label-text">top_k</span>
            {#if formTopK != null}
              <button
                class="reset-btn"
                type="button"
                onclick={() => {
                  formTopK = null;
                }}
                title="Reset to default"
              >
                <span class="material-icons-outlined" style="font-size: 14px;">restart_alt</span>
              </button>
            {/if}
          </div>
          <div class="slider-row">
            <input
              type="range"
              min="1"
              max="200"
              step="1"
              value={formTopK ?? 50}
              oninput={(e) => {
                formTopK = parseInt((e.target as HTMLInputElement).value, 10);
              }}
            />
            <span class="slider-value">{formTopK ?? 'default'}</span>
          </div>
        </div>
        <div class="form-field">
          <div class="label-with-reset">
            <span class="form-label-text">repetition_penalty</span>
            {#if formRepetitionPenalty != null}
              <button
                class="reset-btn"
                type="button"
                onclick={() => {
                  formRepetitionPenalty = null;
                }}
                title="Reset to default"
              >
                <span class="material-icons-outlined" style="font-size: 14px;">restart_alt</span>
              </button>
            {/if}
          </div>
          <div class="slider-row">
            <input
              type="range"
              min="1.0"
              max="2.0"
              step="0.05"
              value={formRepetitionPenalty ?? 1.1}
              oninput={(e) => {
                formRepetitionPenalty = parseFloat((e.target as HTMLInputElement).value);
              }}
            />
            <span class="slider-value">
              {formRepetitionPenalty != null ? formRepetitionPenalty.toFixed(2) : 'default'}
            </span>
          </div>
        </div>
        <div class="form-field">
          <div class="label-with-reset">
            <span class="form-label-text">max_new_frames</span>
            {#if formMaxNewFrames != null}
              <button
                class="reset-btn"
                type="button"
                onclick={() => {
                  formMaxNewFrames = null;
                }}
                title="Reset to default"
              >
                <span class="material-icons-outlined" style="font-size: 14px;">restart_alt</span>
              </button>
            {/if}
          </div>
          <div class="slider-row">
            <input
              type="range"
              min="256"
              max="8192"
              step="64"
              value={formMaxNewFrames ?? 2048}
              oninput={(e) => {
                formMaxNewFrames = parseInt((e.target as HTMLInputElement).value, 10);
              }}
            />
            <span class="slider-value">{formMaxNewFrames ?? 'default'}</span>
          </div>
        </div>
      </div>
    </details>
  {/if}
{/snippet}

<div class="voice-section">
  <div class="section-header">
    <div>
      <h2>Voice Presets</h2>
      <p class="section-subtitle">
        Create named voice profiles with custom voice selection{supportsEmotion
          ? ', clips, and voice settings'
          : ' and speed'}.
      </p>
    </div>
  </div>

  {#if loading}
    <div class="loading-card">Loading voice presets...</div>
  {:else if error}
    <div class="error">{error}</div>
  {:else if presets.length === 0 && !creating}
    <div class="empty-state">
      <span class="material-icons-outlined empty-icon">record_voice_over</span>
      <p>No voice presets yet. Create one to customize how text-to-speech sounds for you.</p>
      <button class="btn" type="button" onclick={startCreate}>
        <span class="material-icons-outlined" style="font-size: 16px; vertical-align: middle;"
          >add</span
        >
        New Voice Preset
      </button>
    </div>
  {:else}
    <div class="preset-list">
      {#each presets as preset (preset.id)}
        {#if editingId === preset.id}
          <div class="preset-card editing">
            <div class="editor-form">
              <label class="form-label">
                Name
                <input type="text" bind:value={formName} placeholder="My voice preset" />
              </label>

              {@render voicePickerField(!!(preset.clip_filename || clipFile))}

              {#if supportsEmotion}
                {#if preset.clip_filename}
                  <div class="form-field">
                    <span class="form-label-text">Current Clip</span>
                    <div class="clip-row">
                      <span class="clip-name">{preset.clip_filename}</span>
                      <audio controls preload="none" src={getVoiceClipUrl(preset.id)}></audio>
                      <button
                        class="btn-icon danger"
                        type="button"
                        onclick={() => handleDeleteClip(preset.id)}
                        title="Delete clip"
                      >
                        <span class="material-icons-outlined">delete</span>
                      </button>
                    </div>
                  </div>
                {/if}
                {@render voiceClipPicker()}
                {@render refTextField(preset.id, !!preset.clip_filename || !!clipFile)}
              {/if}

              <label class="form-label">
                Speed
                <div class="slider-row">
                  <input type="range" min="0.5" max="2.0" step="0.05" bind:value={formSpeed} />
                  <span class="slider-value">{formSpeed.toFixed(2)}</span>
                </div>
              </label>

              {#if supportsEmotion}
                <div class="form-field">
                  <div class="label-with-reset">
                    <span class="form-label-text">Variation (Temperature)</span>
                    {#if formTemperature != null}
                      <button
                        class="reset-btn"
                        type="button"
                        onclick={() => {
                          formTemperature = null;
                        }}
                        title="Reset to default"
                      >
                        <span class="material-icons-outlined" style="font-size: 14px;"
                          >restart_alt</span
                        >
                      </button>
                    {/if}
                  </div>
                  <div class="slider-row">
                    <input
                      type="range"
                      min="0"
                      max="1.5"
                      step="0.05"
                      value={formTemperature ?? 0.8}
                      oninput={(e) => {
                        formTemperature = parseFloat((e.target as HTMLInputElement).value);
                      }}
                    />
                    <span class="slider-value"
                      >{formTemperature != null ? formTemperature.toFixed(2) : 'default'}</span
                    >
                  </div>
                </div>
              {/if}

              {@render seedAudition()}
              {@render advancedKnobs()}
              {@render previewPanel()}

              <div class="editor-actions">
                <button class="btn" type="button" disabled={saving} onclick={handleSave}>
                  {saving ? 'Saving...' : 'Save'}
                </button>
                <button
                  class="btn alt"
                  type="button"
                  disabled={testing || !canTest}
                  onclick={handleTest}
                >
                  {#if testing}
                    <span
                      class="material-icons-outlined"
                      style="font-size: 14px; vertical-align: middle;">stop</span
                    >
                    Testing...
                  {:else}
                    <span
                      class="material-icons-outlined"
                      style="font-size: 14px; vertical-align: middle;">play_arrow</span
                    >
                    Test
                  {/if}
                </button>
                <button class="btn alt" type="button" onclick={cancelEdit}>Cancel</button>
              </div>
            </div>
          </div>
        {:else}
          <div class="preset-card" class:active={preset.id === activePresetId}>
            <div class="preset-header">
              <span class="preset-name">{preset.name}</span>
              {#if preset.id === activePresetId}
                <span class="active-badge">Active</span>
              {/if}
            </div>
            <div class="preset-meta">
              <span class="preset-voice">{voiceLabel(preset)}</span>
              <span class="preset-speed">Speed: {preset.speed.toFixed(2)}x</span>
            </div>
            {#if paramSummary(preset).length > 0}
              <div class="preset-params">
                {#each paramSummary(preset) as param (param)}
                  <span class="param-tag">{param}</span>
                {/each}
              </div>
            {/if}
            {#if needsIclBackfill(preset)}
              <div class="icl-hint">
                <span class="icl-hint-text">
                  The engine clones this preset with higher fidelity if we transcribe the reference
                  clip for ICL ref-text.
                </span>
                <button
                  class="btn alt small"
                  type="button"
                  disabled={retranscribingPresetId === preset.id}
                  onclick={() => handleRetranscribeForIcl(preset)}
                >
                  <span
                    class="material-icons-outlined"
                    style="font-size: 14px; vertical-align: middle;">transcribe</span
                  >
                  {retranscribingPresetId === preset.id
                    ? 'Transcribing…'
                    : 'Auto-transcribe reference'}
                </button>
              </div>
            {/if}
            <div class="preset-actions">
              {#if preset.id !== activePresetId}
                <button
                  class="btn alt small"
                  type="button"
                  onclick={() => handleSetActive(preset.id)}
                >
                  <span
                    class="material-icons-outlined"
                    style="font-size: 14px; vertical-align: middle;">check_circle</span
                  >
                  Set Active
                </button>
              {:else}
                <button class="btn alt small" type="button" onclick={() => handleSetActive(null)}>
                  <span
                    class="material-icons-outlined"
                    style="font-size: 14px; vertical-align: middle;">remove_circle_outline</span
                  >
                  Unset Active
                </button>
              {/if}
              <button class="btn alt small" type="button" onclick={() => startEdit(preset)}>
                <span
                  class="material-icons-outlined"
                  style="font-size: 14px; vertical-align: middle;">edit</span
                >
                Edit
              </button>
              <button
                class="btn-icon danger"
                type="button"
                onclick={() => handleDelete(preset)}
                title="Delete preset"
              >
                <span class="material-icons-outlined">delete</span>
              </button>
            </div>
          </div>
        {/if}
      {/each}
    </div>

    {#if creating}
      <div class="preset-card editing">
        <div class="editor-form">
          <label class="form-label">
            Name
            <input type="text" bind:value={formName} placeholder="My voice preset" />
          </label>

          {@render voicePickerField(!!clipFile)}

          <label class="form-label">
            Speed
            <div class="slider-row">
              <input type="range" min="0.5" max="2.0" step="0.05" bind:value={formSpeed} />
              <span class="slider-value">{formSpeed.toFixed(2)}</span>
            </div>
          </label>

          {#if supportsEmotion}
            <div class="form-field">
              <div class="label-with-reset">
                <span class="form-label-text">Variation (Temperature)</span>
                {#if formTemperature != null}
                  <button
                    class="reset-btn"
                    type="button"
                    onclick={() => {
                      formTemperature = null;
                    }}
                    title="Reset to default"
                  >
                    <span class="material-icons-outlined" style="font-size: 14px;">restart_alt</span
                    >
                  </button>
                {/if}
              </div>
              <div class="slider-row">
                <input
                  type="range"
                  min="0"
                  max="1.5"
                  step="0.05"
                  value={formTemperature ?? 0.8}
                  oninput={(e) => {
                    formTemperature = parseFloat((e.target as HTMLInputElement).value);
                  }}
                />
                <span class="slider-value"
                  >{formTemperature != null ? formTemperature.toFixed(2) : 'default'}</span
                >
              </div>
            </div>
          {/if}

          {#if supportsEmotion}
            {@render voiceClipPicker()}
            {@render refTextField(null, !!clipFile)}
          {/if}

          {@render seedAudition()}
          {@render advancedKnobs()}
          {@render previewPanel()}

          <div class="editor-actions">
            <button class="btn" type="button" disabled={saving} onclick={handleSave}>
              {saving ? 'Creating...' : 'Create'}
            </button>
            <button
              class="btn alt"
              type="button"
              disabled={testing || !canTest}
              onclick={handleTest}
            >
              {#if testing}
                <span
                  class="material-icons-outlined"
                  style="font-size: 14px; vertical-align: middle;">stop</span
                >
                Testing...
              {:else}
                <span
                  class="material-icons-outlined"
                  style="font-size: 14px; vertical-align: middle;">play_arrow</span
                >
                Test
              {/if}
            </button>
            <button class="btn alt" type="button" onclick={cancelEdit}>Cancel</button>
          </div>
        </div>
      </div>
    {/if}

    {#if !creating && !editingId}
      <button
        class="btn alt new-preset-btn"
        type="button"
        disabled={atLimit}
        onclick={startCreate}
        title={atLimit ? `Maximum of ${MAX_PRESETS} presets reached` : ''}
      >
        <span class="material-icons-outlined" style="font-size: 16px; vertical-align: middle;"
          >add</span
        >
        New Voice Preset
        {#if atLimit}
          <span class="limit-note">(limit reached)</span>
        {/if}
      </button>
    {/if}
  {/if}
</div>

<style>
  .voice-section {
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

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    padding: 48px 24px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    text-align: center;
    color: var(--muted);
  }

  .empty-icon {
    font-size: 48px;
    opacity: 0.4;
  }

  .empty-state p {
    margin: 0;
    max-width: 360px;
    font-size: 13px;
    line-height: 1.5;
  }

  .preset-list {
    display: grid;
    gap: 12px;
  }

  .preset-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 16px;
    display: grid;
    gap: 8px;
  }

  .preset-card.active {
    border-color: var(--accent);
  }

  .preset-card.editing {
    border-color: var(--accent);
    border-style: dashed;
  }

  .preset-header {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .preset-name {
    font-weight: 600;
    font-size: 14px;
    color: var(--ink);
  }

  .active-badge {
    font-size: 11px;
    font-weight: 600;
    padding: 2px 8px;
    border-radius: 8px;
    background: var(--accent);
    color: white;
  }

  .preset-meta {
    display: flex;
    gap: 12px;
    font-size: 12px;
    color: var(--muted);
  }

  .preset-voice {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .preset-speed {
    white-space: nowrap;
  }

  .preset-params {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }

  .param-tag {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 8px;
    background: var(--ring);
    color: var(--muted);
  }

  .preset-actions {
    display: flex;
    gap: 8px;
    padding-top: 4px;
    align-items: center;
  }

  .editor-form {
    display: grid;
    gap: 12px;
  }

  .form-label {
    display: grid;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .form-label-text {
    font-size: 12px;
    color: var(--muted);
  }

  .label-with-reset {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .reset-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: 6px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    padding: 0;
  }

  .reset-btn:hover {
    background: var(--ring);
    color: var(--ink);
  }

  .form-field {
    display: grid;
    gap: 6px;
  }

  .editor-form input[type='text'] {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .clip-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .clip-name {
    font-size: 12px;
    color: var(--ink);
  }

  .clip-row audio {
    height: 32px;
    flex: 1;
    min-width: 150px;
  }

  .clip-upload-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .clip-upload-row input[type='file'] {
    font-size: 12px;
    color: var(--muted);
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

  .editor-actions {
    display: flex;
    gap: 12px;
    padding-top: 4px;
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
    display: inline-flex;
    align-items: center;
    gap: 4px;
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

  .btn.alt:hover:not(:disabled) {
    background: var(--ring);
  }

  .btn.small {
    padding: 4px 10px;
    font-size: 12px;
    border-radius: 10px;
  }

  .btn-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--muted);
    cursor: pointer;
    padding: 0;
  }

  .btn-icon:hover {
    background: var(--ring);
  }

  .btn-icon.danger {
    color: var(--danger);
  }

  .btn-icon.danger:hover {
    border-color: var(--danger);
  }

  .btn-icon .material-icons-outlined {
    font-size: 16px;
  }

  .form-hint {
    font-size: 11px;
    color: var(--muted);
    line-height: 1.4;
  }

  .new-preset-btn {
    justify-self: start;
  }

  .preview-panel {
    margin-top: 8px;
    padding: 12px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    display: grid;
    gap: 10px;
  }

  .preview-controls {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 12px;
    align-items: end;
  }

  .toggle-inline {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
  }

  .preview-transcript {
    padding: 10px 12px;
    background: var(--bg);
    border: 1px solid var(--ring);
    border-radius: 8px;
    line-height: 1.7;
  }

  .preview-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    align-items: center;
    font-size: 12px;
    color: var(--muted);
  }

  .preview-state {
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--ink);
  }

  .preview-error {
    padding: 8px 10px;
    background: color-mix(in srgb, var(--danger) 12%, transparent);
    border: 1px solid var(--danger);
    border-radius: 8px;
    color: var(--danger);
    font-size: 12px;
  }

  .word-table {
    margin-top: 8px;
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  }

  .word-table th,
  .word-table td {
    text-align: left;
    padding: 4px 8px;
    border-bottom: 1px solid var(--ring);
  }

  .word-table tr.active {
    background: color-mix(in srgb, var(--accent) 25%, transparent);
  }

  .limit-note {
    font-size: 11px;
    color: var(--muted);
    font-weight: 400;
  }

  .clip-source-tabs {
    display: flex;
    gap: 16px;
    margin-bottom: 8px;
  }

  .clip-source-tab {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
    font-size: 13px;
  }

  .audiobook-picker {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 12px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
  }

  .ab-search-row {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .ab-search-row input[type='text'] {
    flex: 1;
    padding: 6px 8px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--bg);
    color: inherit;
  }

  .ab-results {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 220px;
    overflow-y: auto;
    border: 1px solid var(--ring);
    border-radius: 8px;
  }

  .ab-results li {
    border-bottom: 1px solid var(--ring);
  }

  .ab-results li:last-child {
    border-bottom: none;
  }

  .ab-result-btn {
    display: block;
    width: 100%;
    text-align: left;
    padding: 8px 12px;
    background: transparent;
    border: none;
    cursor: pointer;
    color: inherit;
    font-size: 13px;
  }

  .ab-result-btn:hover {
    background: var(--bg);
  }

  .ab-result-author {
    color: var(--muted);
    font-size: 12px;
  }

  .ab-selected-header {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
  }

  .ab-selected-header .muted {
    color: var(--muted);
  }

  .ab-selected-header .btn-icon {
    margin-left: auto;
  }

  .ab-selected-title {
    flex: 1;
    min-width: 0;
  }

  .ab-chapter-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-height: 220px;
    overflow-y: auto;
    border: 1px solid var(--ring);
    border-radius: 10px;
    padding: 4px;
  }

  .ab-chapter-btn {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    padding: 6px 10px;
    border-radius: 6px;
    cursor: pointer;
    font-size: 13px;
    color: var(--ink);
  }
  .ab-chapter-btn:hover:not(:disabled) {
    background: var(--bg-2, rgba(0, 0, 0, 0.05));
  }
  .ab-chapter-btn:disabled {
    opacity: 0.6;
    cursor: progress;
  }
  .ab-chapter-btn.active {
    background: var(--accent-soft, rgba(30, 111, 255, 0.12));
    color: var(--accent, var(--link, #4d8fff));
    font-weight: 500;
  }
  .ab-chapter-title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .ab-trim-block {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .ab-preview-status,
  .ab-pan-row {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
    font-size: 12px;
  }

  .link-btn {
    background: transparent;
    border: none;
    padding: 2px 6px;
    color: var(--link, var(--accent, #4d8fff));
    cursor: pointer;
    font-size: 12px;
  }
  .link-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .link-btn:hover:not(:disabled) {
    text-decoration: underline;
  }

  .editor-form textarea {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
    font-family: inherit;
    resize: vertical;
  }

  .advanced-details {
    border: 1px solid var(--ring);
    border-radius: 12px;
    padding: 8px 12px;
    background: var(--card);
  }

  .advanced-details summary {
    cursor: pointer;
    font-size: 13px;
    font-weight: 500;
    color: var(--ink);
    padding: 4px 0;
  }

  .advanced-details[open] summary {
    margin-bottom: 8px;
  }

  .advanced-body {
    display: grid;
    gap: 12px;
    padding-top: 4px;
  }

  .audition-samples {
    display: grid;
    gap: 8px;
  }

  .audition-sample {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .audition-sample audio {
    flex: 1;
    height: 32px;
    min-width: 0;
  }

  .icl-hint {
    display: flex;
    gap: 10px;
    align-items: center;
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px dashed var(--accent);
    background: rgb(from var(--accent) r g b / 0.06);
    font-size: 12px;
  }

  .icl-hint-text {
    flex: 1;
    color: var(--ink);
    line-height: 1.4;
  }
</style>
