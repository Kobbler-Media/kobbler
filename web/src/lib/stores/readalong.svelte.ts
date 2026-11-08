// Read-along state store using Svelte 5 runes
//
// Manages two modes:
// 1. TTS mode: streams TTS audio and highlights sentences in the EPUB reader
// 2. Audiobook mode: syncs existing audiobook playback with EPUB text highlighting

import { synthesizeTts, getActivePreset } from '#lib/api/client.js';
import type {
  ReadAlongSyncPoint,
  ReadAlongTranscriptSegment,
  UserVoicePreset
} from '#lib/api/client.js';
import { getAudioPlayerState } from '#lib/stores/audioplayer.svelte.js';

export type ReadAlongMode = 'off' | 'tts' | 'audiobook';

// localStorage helpers (SSR-safe)
function lsGet(key: string): string | null {
  if (typeof window === 'undefined') return null;
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}
function lsSet(key: string, value: string) {
  if (typeof window === 'undefined') return;
  try {
    localStorage.setItem(key, value);
  } catch {
    // quota exceeded or private browsing
  }
}

/**
 * Default render rate: the engine's own.
 *
 * Nothing in the chain slows anything down — measured end to end on tts-breeze
 * with `speed` unset, 171-197 wpm on well-formed prose, and koblem's hop
 * returns byte-identical audio. Text that narrates slowly is text that arrived
 * broken into fragments, which `normalize_for_synthesis` now puts back together
 * on the server. `speed` is a real server-side WSOLA render (pitch-preserved,
 * never a client `playbackRate`) and stays available as a preference, but it is
 * not the fix for laboured narration and is not the baseline.
 */
const DEFAULT_SPEED = 1.0;

/**
 * Preroll on the first scheduled buffer of a render.
 *
 * Starting a buffer at `ctx.currentTime` on a cold AudioContext is a known
 * glitch source — the output device has not spun up when the samples are due,
 * so the opening clicks — and it leaves no slack for the rest: chunks arrive at
 * roughly the rate they are generated, close enough to real time that one
 * hiccup (a GPU turn taken by another job, a slow chunk) puts the next buffer's
 * start time in the past. Web Audio does not stall for that, it plays the
 * buffer late, and the shortfall becomes an audible drop-out.
 *
 * Same value and same reason as koblibs' `createFaPlayback`, which is the
 * playback the koblem playground uses and the reason it sounds clean on the
 * identical event stream.
 */
const SCHEDULE_LEAD_S = 0.5;

/**
 * How long to wait for the reader to report a page as ready after a turn.
 *
 * The wait exists because the new page's text is only readable once foliate has
 * relocated. If that signal never arrives the read-along must not hang forever
 * with the audio stopped — it reads whatever is on screen and carries on.
 */
const PAGE_READY_TIMEOUT_MS = 4000;

/**
 * How many consecutive pages with no readable text to skip before giving up.
 *
 * Blank pages are normal — plates, part titles, the gap at a chapter end — so
 * skipping is right. Skipping without a bound is not: a reader that reports no
 * text at all (the paginated renderer with no visible range, say) turns the
 * skip into a silent walk through the rest of the book at page-turn speed.
 */
const MAX_BLANK_PAGE_SKIPS = 12;

/**
 * How much text goes into ONE synth request.
 *
 * The engine's long-form mode rolls the previous chunk's generated audio and
 * text forward as the reference for the next one, so timbre, tone and pacing
 * carry across the seams inside a request — and reset at the boundary between
 * requests. Bigger requests therefore sound more like one reader. A request per
 * page, which is what this used to do, restarted the carry at every page turn.
 *
 * This is NOT the request size — the API cuts a span into engine-sized
 * passages itself, on the normalised text, and carries the offsets that keep
 * word indices lining up across them. Doing it here as well meant two
 * different chunking policies (and kobplayer a third), each picking seams the
 * others knew nothing about.
 *
 * What is left is a memory bound, and it is a client concern. Audio arrives
 * about three times faster than it plays, so everything past the playhead sits
 * in decoded AudioBuffers: a whole 30-minute chapter would be ~170 MB of
 * Float32 in the tab. 1200 words is roughly eight minutes of narration — under
 * 50 MB at the worst moment, and several pages of any book.
 */
const SPAN_BUFFER_WORDS = 1200;

// Module-level state (singleton) — initialized from localStorage when available
let mode = $state<ReadAlongMode>('off');
let playing = $state(false);
let currentSentence = $state('');
// Word-level highlight state, populated by the forced-aligner pipeline.
// `currentWordIndex` is the global word index across the whole synthesized
// passage (paragraph_start.word_offset + paragraph-local word_index).
let currentWordIndex = $state(-1);
let currentWord = $state('');
let speed = $state(
  parseFloat(lsGet('kobbler-tts-speed') ?? String(DEFAULT_SPEED)) || DEFAULT_SPEED
);
// Empty means "let the server resolve it" — active preset, then admin default,
// then the engine's own fallback. A hardcoded id here was a kokoro voice name
// that no other engine has, so it either 400'd or got silently rewritten.
let voice = $state(lsGet('kobbler-tts-voice') ?? '');
let lang = $state(lsGet('kobbler-tts-lang') ?? 'a');
let continuous = $state((lsGet('kobbler-tts-continuous') ?? 'true') === 'true');
let temperature = $state<number | null>(
  lsGet('kobbler-tts-temperature') ? parseFloat(lsGet('kobbler-tts-temperature')!) : null
);
let activePresetId = $state<string | null>(lsGet('kobbler-tts-active-preset'));
let presetLoaded = false;
let loading = $state(false);
let error = $state<string | null>(null);

// TTS mode state — AudioContext-based gapless playback.
// Output is routed through MediaStreamDestination → hidden HTMLAudioElement so
// the page holds an active audio session: keeps playing with the screen off,
// gets MediaSession lockscreen controls, and isn't throttled in background tabs.
let ttsAbortController: AbortController | null = null;
// The rate the CURRENT render was requested at, kept so the applied rate the
// server reports back can be compared against it.
//
// Rate is a server-side render parameter: the audio arrives already at speed and
// the aligner's timings are already in that timebase, so nothing here rescales
// either. What can happen is the engine refusing the rate and rendering at 1.0
// anyway — the one outcome that looks exactly like success — which is why the
// requested value is held rather than trusted.
let ttsRequestedSpeed = 1.0;
// `wordOffset` / `wordCount` are the entry's span in the whitespace-token
// sequence of the synth input, which is the same sequence the aligner reports
// `word_index` against. On the aligned path they are what selects the current
// sentence: the highlight follows the WORD the voice is on, not a time range,
// so it is correct from the first `alignment_partial` rather than waiting for
// the whole passage to finish. `startTime`/`endTime` are still filled in from
// the aligner (seek, and the unaligned engines' only mechanism), but are not
// what drives the highlight when word timings exist. Chatterbox/kokoro entries
// don't set the word fields.
let ttsSentences: Array<{
  text: string;
  startTime: number;
  endTime: number;
  wordOffset?: number;
  wordCount?: number;
}> = [];
// Word timings from the forced aligner. `t0Ms` / `t1Ms` are absolute ms
// from the start of TTS playback (paragraph `audio_offset_ms` already added
// on the server). Sorted by `globalWordIndex` ascending and contiguous
// across paragraphs.
let ttsWords: Array<{
  text: string;
  globalWordIndex: number;
  t0Ms: number;
  t1Ms: number;
}> = [];
/**
 * Chunk anchors: "this word range starts at this audio position".
 *
 * The engine knows where its own sentence groups begin without an aligner
 * having said anything, which is why these arrive seconds before the first
 * word timings do and keep arriving when alignment is off entirely. They are
 * what lets the reader move with the voice and turn its own pages — the part
 * of read-along that is not decoration. Offsets are already global to the
 * whole read; word timings overwrite them wherever the two overlap.
 */
let ttsAnchors: Array<{ wordOffset: number; wordCount: number; audioOffsetMs: number }> = [];
/** Reading rate to bound the newest anchor with, until a later one closes it. */
const ANCHOR_SEED_WPM = 175;
let ttsLastHighlightedWordIdx = -1;
let ttsAudioCtx: AudioContext | null = null;
let ttsBridgeDest: MediaStreamAudioDestinationNode | null = null;
let ttsBridgeEl: HTMLAudioElement | null = null;
let ttsNextStartTime = 0;
// Cumulative silence between scheduled buffers, in context seconds. Subtracted
// from elapsed so the highlight follows audio time, not wall time.
let ttsGapSeconds = 0;
let ttsSources: AudioBufferSourceNode[] = [];
let ttsSyncFrame: number | null = null;
let ttsLastHighlightedIdx = -1;
// Context time at which the first audio buffer of this render starts. -1 until
// one has been scheduled — the timebase is where the AUDIO begins, not where
// the request went out.
let ttsPlaybackStartCtxTime = -1;
let ttsVisibilityHandler: (() => void) | null = null;
let ttsNowPlaying: { title: string; artist?: string; artwork?: string } = {
  title: 'Kobbler Reader'
};
// Audiobook sync state
let syncPoints: ReadAlongSyncPoint[] = [];
let transcriptSegments: ReadAlongTranscriptSegment[] = [];
let lastHighlightedSegmentId = '';
let syncAnimFrame: number | null = null;

// Callbacks from the reader page
let highlightCallback: ((sentence: string) => void) | null = null;
let highlightWordCallback: ((localWordIndex: number, sentenceText: string) => void) | null = null;
let clearHighlightCallback: (() => void) | null = null;
let nextPageCallback: (() => void) | null = null;
let getPageTextCallback: (() => string) | null = null;
let getSpanTextCallback: ((maxWords: number) => Promise<string>) | null = null;
let goToCfiCallback: ((cfi: string) => void) | null = null;
let readingBookId: string | null = null;
let pageReadyResolve: (() => void) | null = null;

/** Register reader callbacks. Must be called before starting any mode. */
export function registerCallbacks(cbs: {
  highlight: (sentence: string) => void;
  highlightWord?: (localWordIndex: number, sentenceText: string) => void;
  clearHighlight: () => void;
  nextPage: () => void;
  getPageText: () => string;
  getSpanText?: (maxWords: number) => Promise<string>;
  goToCfi?: (cfi: string) => void;
  bookId?: string;
}) {
  readingBookId = cbs.bookId ?? null;
  highlightCallback = cbs.highlight;
  highlightWordCallback = cbs.highlightWord ?? null;
  clearHighlightCallback = cbs.clearHighlight;
  nextPageCallback = cbs.nextPage;
  getPageTextCallback = cbs.getPageText;
  getSpanTextCallback = cbs.getSpanText ?? null;
  goToCfiCallback = cbs.goToCfi ?? null;
}

/**
 * Called by the reader page when a new page/iframe is ready after navigation —
 * on first load AND on every relocate. Read-along page turns depend on the
 * relocate call: without it the continuation waits on a promise nothing ever
 * settles, and playback stops dead after exactly one turn.
 */
export function notifyPageReady() {
  if (pageReadyResolve) {
    pageReadyResolve();
    pageReadyResolve = null;
  }
}

function waitForPageReady(): Promise<void> {
  return new Promise((resolve) => {
    let settled = false;
    let timer: ReturnType<typeof setTimeout>;
    const finish = () => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      pageReadyResolve = null;
      resolve();
    };
    timer = setTimeout(finish, PAGE_READY_TIMEOUT_MS);
    pageReadyResolve = finish;
  });
}

/**
 * Turn to the next page and resolve once it is ready to read.
 *
 * The waiter is armed BEFORE the turn: `relocate` can fire before control
 * returns here, and a waiter registered after that has already missed it.
 */
function turnPageAndWait(): Promise<void> {
  const ready = waitForPageReady();
  nextPageCallback?.();
  return ready;
}

// ============================================================================
// TTS Mode
// ============================================================================

export async function startTts() {
  if (!getPageTextCallback) {
    error = 'Reader callbacks not registered';
    return;
  }

  mode = 'tts';
  loading = true;
  error = null;

  // Load user's active preset on first TTS start
  await loadActivePreset();

  try {
    playing = true;
    await synthesizeAndPlayPage();
  } catch (e) {
    error = e instanceof Error ? e.message : 'TTS failed';
    playing = false;
  } finally {
    loading = false;
  }
}

/**
 * The text ahead of the reader, or null when there is none to be had.
 *
 * "Continuous" off means stop at the end of this page, so there is no span to
 * read past it. A reader that cannot produce one (or throws parsing the pages
 * ahead) falls back to the page, which is what this used to do always.
 */
async function fetchSpanText(): Promise<string | null> {
  if (!continuous || !getSpanTextCallback) return null;
  try {
    const text = await getSpanTextCallback(SPAN_BUFFER_WORDS);
    return text.trim() ? text : null;
  } catch (e) {
    console.warn('read-along: could not read ahead of this page', e);
    return null;
  }
}

/** Send a whole span; the API decides where the request seams go. */
async function playSpan(text: string) {
  const trimmed = text.trim();
  if (trimmed) await synthesizeAndPlayText(trimmed);
}

async function synthesizeAndPlayPage(blanksSkipped = 0) {
  if (!getPageTextCallback || !playing) return;

  const spanText = await fetchSpanText();
  const pageText = spanText ?? getPageTextCallback();
  if (!pageText.trim()) {
    if (!continuous) {
      playing = false;
      return;
    }
    if (blanksSkipped >= MAX_BLANK_PAGE_SKIPS) {
      // Iterative rather than recursive so this is a stop, not a stack
      // overflow, and loud rather than silent — the reader turning pages
      // forever with no sound is indistinguishable from a crash.
      error = 'Read-along could not find any text to read on this page.';
      playing = false;
      clearHighlightCallback?.();
      return;
    }
    await turnPageAndWait();
    await synthesizeAndPlayPage(blanksSkipped + 1);
    return;
  }

  await playSpan(pageText);
}

/**
 * The `voice` to put on a synth request.
 *
 * An active preset is resolved SERVER-side: it is the only place that knows
 * whether the preset is backed by a cloned clip (`preset-<uuid>`, which also
 * has to be re-registered with the engine before use) or by a plain named
 * voice. Naming a service voice here instead would silently beat the preset —
 * and on an engine whose voices are all clones, the id we happened to have
 * stored is not a voice the user ever chose.
 *
 * This matters beyond preference: an engine with a forced aligner picks its
 * narrator per REQUEST when no voice is named, and holds it only for that
 * request — so an unnamed voice means a new narrator at every span boundary.
 * A preset with a clip, or the server's minted narrator, is what pins it.
 */
function requestVoice(): string {
  return activePresetId ? '' : voice;
}

async function synthesizeAndPlayText(pageText: string) {
  loading = true;
  // Per render, not just per session: page-to-page continuation comes straight
  // here, and a stale "can't do that speed" would outlive the speed it was
  // complaining about.
  error = null;
  cleanupTtsAudio();
  ttsAbortController = new AbortController();
  // Local rather than store state: a continuation resets `error` on the way in, and
  // what decides whether to turn the page is what happened to THIS request.
  let failed = false;

  const ctx = new AudioContext({ latencyHint: 'interactive' });
  ttsAudioCtx = ctx;
  ttsNextStartTime = ctx.currentTime;
  // -1 until the first buffer is actually scheduled. Pegging the timebase to
  // context creation instead made the highlighter race through the alignment
  // words that arrive before any audio does.
  ttsPlaybackStartCtxTime = -1;
  ttsGapSeconds = 0;
  ttsRequestedSpeed = speed;

  setupBackgroundBridge(ctx);

  // Start highlight sync loop
  startTtsSyncLoop();

  try {
    const ttsParams =
      temperature != null
        ? {
            temperature: temperature ?? undefined
          }
        : undefined;
    const resp = await synthesizeTts(
      pageText,
      requestVoice(),
      { ...(ttsParams ?? {}), speed, bookId: readingBookId ?? undefined },
      ttsAbortController?.signal
    );
    if (!resp.ok || !resp.body) {
      throw new Error(`TTS request failed: ${resp.status}`);
    }

    const reader = resp.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';

    // The streamed paragraph path tracks the current paragraph's start in
    // playback time (AudioContext seconds) so word timings can be globalised.
    // One request yields SEVERAL of these — the API cuts a span into
    // engine-sized passages — and each carries the word and audio offset that
    // places it in the span.
    let currentParagraph: {
      index: number;
      wordOffset: number;
      audioOffsetMs: number;
      sampleRate: number;
      text: string;
    } | null = null;

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      if (ttsAbortController?.signal.aborted) break;

      buffer += decoder.decode(value, { stream: true });
      const lines = buffer.split('\n');
      buffer = lines.pop() || '';

      for (const line of lines) {
        if (!line.trim()) continue;
        let chunk: Record<string, unknown>;
        try {
          chunk = JSON.parse(line);
        } catch {
          continue; // Skip malformed lines
        }

        // ── Chatterbox / kokoro path (per-sentence MP3) ─────────────────────
        if (chunk.type === 'sentence' && chunk.audio_b64) {
          const audioData = base64ToArrayBuffer(chunk.audio_b64 as string);

          // Decode independent MP3 chunk into AudioBuffer
          const audioBuffer = await ctx.decodeAudioData(audioData.slice(0));
          const source = ctx.createBufferSource();
          source.buffer = audioBuffer;
          source.connect(ttsBridgeDest ?? ctx.destination);

          noteAppliedSpeed(chunk.speed);

          const startAt = scheduleStartTime(ctx);
          source.start(startAt);

          // Already at the rendered rate — the buffer's own duration is the
          // wall-clock duration.
          const playedDuration = audioBuffer.duration;
          const relStart = audioTimeOf(startAt);
          const relEnd = relStart + playedDuration;
          ttsSentences.push({
            text: chunk.text as string,
            startTime: relStart,
            endTime: relEnd
          });
          ttsNextStartTime = startAt + playedDuration;
          ttsSources.push(source);

          if (ttsSentences.length === 1) {
            loading = false;
          }
          continue;
        }

        // ── Streamed paragraph path (PCM + word timings) ────────────────────
        if (chunk.type === 'paragraph_start') {
          noteAppliedSpeed(chunk.speed);
          currentParagraph = {
            index: (chunk.index as number) ?? 0,
            wordOffset: (chunk.word_offset as number) ?? 0,
            audioOffsetMs: (chunk.audio_offset_ms as number) ?? 0,
            sampleRate: (chunk.sample_rate as number) ?? 24_000,
            text: (chunk.text as string) ?? ''
          };
          // A span arrives as a handful of paragraph events, each carrying
          // several hundred words, so their sentences have to be cut here —
          // highlighting an event's own text would light up half a page at a
          // time. The server echoes back the exact string it synthesised and
          // the offset it sits at, so token offsets counted off it line up
          // with the aligner's `word_index` without any matching or search.
          appendSentenceEntries(currentParagraph.text, currentParagraph.wordOffset);
          continue;
        }

        if (chunk.type === 'audio' && chunk.pcm_b64 && currentParagraph) {
          const pcmBytes = base64ToArrayBuffer(chunk.pcm_b64 as string);
          const audioBuffer = pcm16ToAudioBuffer(ctx, pcmBytes, currentParagraph.sampleRate);
          const source = ctx.createBufferSource();
          source.buffer = audioBuffer;
          source.connect(ttsBridgeDest ?? ctx.destination);

          const startAt = scheduleStartTime(ctx);
          source.start(startAt);
          ttsNextStartTime = startAt + audioBuffer.duration;
          ttsSources.push(source);

          if (ttsSources.length === 1) {
            loading = false;
          }
          continue;
        }

        // Position without an aligner. Arrives before the first alignment and
        // instead of it when alignment is off, so it is what the reader steers
        // by until (and unless) word timings replace it.
        if (chunk.type === 'chunk' && typeof chunk.word_offset === 'number') {
          ttsAnchors.push({
            wordOffset: chunk.word_offset as number,
            wordCount: (chunk.word_count as number) ?? 0,
            audioOffsetMs: (chunk.audio_offset_ms as number) ?? 0
          });
          applyAnchorTimes();
          // Anything the aligner has already resolved is better than an
          // interpolation over it, so it goes back on top.
          refreshSentenceTimesFromWords();
          continue;
        }

        if (
          (chunk.type === 'alignment_partial' || chunk.type === 'alignment_final') &&
          Array.isArray(chunk.words) &&
          currentParagraph
        ) {
          mergeWordTimings(
            chunk.words as Array<{
              word_index: number;
              text: string;
              t0_ms: number;
              t1_ms: number;
            }>,
            currentParagraph.wordOffset,
            currentParagraph.audioOffsetMs
          );

          // Times are refreshed on every pass, partial included: a partial
          // covers the prefix the voice has reached, which is exactly the part
          // a seek can land in.
          refreshSentenceTimesFromWords();
          continue;
        }

        if (chunk.type === 'error') {
          // Surfaced, not just logged: it otherwise presents as the reader
          // silently doing nothing. Whatever already streamed is still worth
          // hearing, so this plays out rather than cutting the audio short —
          // but `fatal` means no more is coming for this passage, and turning
          // the page on that walks the whole book at network speed.
          console.warn('TTS paragraph synth error:', chunk.message);
          if (typeof chunk.message === 'string') error = chunk.message;
          if (chunk.fatal === true) failed = true;
          continue;
        }

        // chunk.type === 'done' or unknown — fall through to outer loop
      }
    }

    let advanced = false;
    const advance = () => {
      if (advanced) return;
      advanced = true;
      if (!playing) return;
      if (failed) {
        playing = false;
        clearHighlightCallback?.();
        return;
      }
      if (continuous) {
        // Out of chapter. The highlight has walked the reader to its last
        // page, so one more turn lands on the next chapter's first page.
        void turnPageAndWait().then(() => synthesizeAndPlayPage());
      } else {
        playing = false;
        clearHighlightCallback?.();
      }
    };

    const lastSource = ttsSources[ttsSources.length - 1];
    if (!lastSource) {
      // Nothing played. A page with no text on it is worth turning past; a
      // request that failed is not — every chapter would fail the same way, and
      // continuous mode would read the book to its end in a few seconds.
      if (failed || error) {
        playing = false;
        clearHighlightCallback?.();
      } else if (continuous) {
        await turnPageAndWait();
        await synthesizeAndPlayPage();
      } else {
        playing = false;
      }
    } else if (ctx.currentTime >= ttsNextStartTime) {
      // Everything scheduled has already played out. This happens whenever the
      // tail of the stream is slower than the audio it is feeding — the last
      // buffer ends while we are still awaiting `done`. An `onended` attached
      // now would never fire, and the reader would simply stop mid-book with
      // no error, which is exactly how "it doesn't continue after a page turn"
      // presents.
      advance();
    } else {
      lastSource.onended = advance;
    }
  } catch (e) {
    if (!ttsAbortController?.signal.aborted) {
      error = e instanceof Error ? e.message : 'TTS streaming failed';
      playing = false;
    }
  } finally {
    loading = false;
  }
}

/**
 * Where a buffer scheduled at context time `startAt` sits on the AUDIO clock.
 *
 * Everything the sync loop compares against is audio-relative — the aligner's
 * word timings are ms from the start of the audio, and `ttsElapsedSeconds`
 * matches them by discounting underrun silence. A span derived from raw context
 * deltas would drift past both by exactly the silence between them.
 */
function audioTimeOf(startAt: number): number {
  return startAt - ttsPlaybackStartCtxTime - ttsGapSeconds;
}

/**
 * Seconds of AUDIO played since this render started, or -1 before it has.
 *
 * Not wall clock: an underrun leaves real silence on the output that
 * `ctx.currentTime` runs straight through, and counting it would leave the
 * highlight permanently ahead of the voice. `ttsGapSeconds` is that silence.
 */
function ttsElapsedSeconds(): number {
  if (!ttsAudioCtx || ttsPlaybackStartCtxTime < 0) return -1;
  return ttsAudioCtx.currentTime - ttsPlaybackStartCtxTime - ttsGapSeconds;
}

function startTtsSyncLoop() {
  if (ttsSyncFrame) cancelAnimationFrame(ttsSyncFrame);

  function tick() {
    if (mode !== 'tts' || !ttsAudioCtx) return;

    const elapsed = ttsElapsedSeconds();
    if (elapsed < 0) {
      // Nothing has been scheduled yet. Alignment words can land before the
      // first audio chunk does; highlighting against a timebase that has not
      // started would race the whole prefix past before a sound is made.
      ttsSyncFrame = requestAnimationFrame(tick);
      return;
    }
    // Aligned path: the word the voice is on picks the sentence. Time only
    // enters through the word lookup, so a sentence lights up the moment the
    // aligner reaches it rather than when a time range says it should.
    //
    // Only while the voice is INSIDE what the aligner has resolved, though.
    // Alignment lands a chunk at a time and the voice can be well past its
    // frontier; the word lookup clamps to the last word it knows, so trusting
    // it out there would peg the highlight to wherever the aligner had got to
    // while the reader sat on a page the listener left minutes ago. Past the
    // frontier the anchors take over — coarser, and moving.
    const alignedThroughMs = ttsWords.length > 0 ? ttsWords[ttsWords.length - 1].t1Ms : -1;
    const wIdx = elapsed * 1000 < alignedThroughMs ? findTtsWordAtMs(elapsed * 1000) : -1;
    const idx = wIdx >= 0 ? findTtsSentenceAtWord(ttsWords[wIdx].globalWordIndex) : -1;
    // The time-span path: chunk-anchor interpolation, and for the unaligned
    // engines (chatterbox/kokoro) the one entry per rendered clip.
    const sentenceIdx = idx >= 0 ? idx : findTtsSentenceAtTime(elapsed);

    if (sentenceIdx >= 0 && sentenceIdx !== ttsLastHighlightedIdx) {
      ttsLastHighlightedIdx = sentenceIdx;
      const sentence = ttsSentences[sentenceIdx];
      currentSentence = sentence.text;
      highlightCallback?.(sentence.text);
      // The word span is re-wrapped by the sentence highlight, so a word that
      // was already current has to be re-emitted against the new sentence.
      ttsLastHighlightedWordIdx = -1;
    }

    // `localWordIndex` is the word's offset inside the highlighted sentence
    // (global − sentence.wordOffset), so the reader can wrap the Nth word
    // inside the sentence span without doing a text search.
    if (idx >= 0 && wIdx !== ttsLastHighlightedWordIdx) {
      ttsLastHighlightedWordIdx = wIdx;
      const w = ttsWords[wIdx];
      const sentence = ttsSentences[idx];
      currentWordIndex = w.globalWordIndex;
      currentWord = w.text;
      highlightWordCallback?.(w.globalWordIndex - (sentence.wordOffset ?? 0), sentence.text);
    }

    ttsSyncFrame = requestAnimationFrame(tick);
  }

  ttsSyncFrame = requestAnimationFrame(tick);
}

function findTtsSentenceAtTime(seconds: number): number {
  for (let i = 0; i < ttsSentences.length; i++) {
    if (seconds >= ttsSentences[i].startTime && seconds < ttsSentences[i].endTime) return i;
  }
  // If past all known boundaries, return last sentence
  return ttsSentences.length > 0 && seconds >= ttsSentences[ttsSentences.length - 1].startTime
    ? ttsSentences.length - 1
    : -1;
}

function cleanupTtsAudio() {
  if (ttsSyncFrame) {
    cancelAnimationFrame(ttsSyncFrame);
    ttsSyncFrame = null;
  }
  for (const src of ttsSources) {
    try {
      src.stop();
    } catch {
      /* already stopped */
    }
  }
  ttsSources = [];
  ttsGapSeconds = 0;
  teardownBackgroundBridge();
  ttsAudioCtx?.close();
  ttsAudioCtx = null;
  ttsNextStartTime = 0;
  ttsPlaybackStartCtxTime = -1;
  ttsSentences = [];
  ttsWords = [];
  ttsAnchors = [];
  ttsLastHighlightedIdx = -1;
  ttsLastHighlightedWordIdx = -1;
  currentWordIndex = -1;
  currentWord = '';
}

/**
 * Bridge AudioContext output through a hidden HTMLAudioElement so the page
 * holds a real audio session: keeps playing with the screen off, gets
 * MediaSession lockscreen controls, and isn't throttled in background tabs.
 */
function setupBackgroundBridge(ctx: AudioContext) {
  if (typeof document === 'undefined') return;
  const dest = ctx.createMediaStreamDestination();
  ttsBridgeDest = dest;

  const el = document.createElement('audio');
  el.srcObject = dest.stream;
  el.autoplay = true;
  // Keep it off the page but referenceable; some browsers want an attached node.
  el.style.position = 'fixed';
  el.style.width = '0';
  el.style.height = '0';
  el.style.opacity = '0';
  el.setAttribute('playsinline', '');
  document.body.appendChild(el);
  ttsBridgeEl = el;
  void el.play().catch(() => {
    /* user-gesture rules — first start() is always inside one */
  });

  setupMediaSession();

  // iOS/Chrome can suspend the AudioContext when the tab returns to foreground
  // after being hidden. Resume + replay the bridge element on visibility wake.
  ttsVisibilityHandler = () => {
    if (document.visibilityState === 'visible') {
      ttsAudioCtx?.resume().catch(() => {});
      ttsBridgeEl?.play().catch(() => {});
    }
  };
  document.addEventListener('visibilitychange', ttsVisibilityHandler);
}

function teardownBackgroundBridge() {
  if (ttsVisibilityHandler && typeof document !== 'undefined') {
    document.removeEventListener('visibilitychange', ttsVisibilityHandler);
  }
  ttsVisibilityHandler = null;

  if (ttsBridgeEl) {
    try {
      ttsBridgeEl.pause();
      ttsBridgeEl.srcObject = null;
      ttsBridgeEl.remove();
    } catch {
      /* element already detached */
    }
  }
  ttsBridgeEl = null;
  ttsBridgeDest?.disconnect();
  ttsBridgeDest = null;

  if (typeof navigator !== 'undefined' && 'mediaSession' in navigator) {
    navigator.mediaSession.metadata = null;
    navigator.mediaSession.playbackState = 'none';
    for (const action of ['play', 'pause', 'stop'] as const) {
      try {
        navigator.mediaSession.setActionHandler(action, null);
      } catch {
        /* unsupported action */
      }
    }
  }
}

function setupMediaSession() {
  if (typeof navigator === 'undefined' || !('mediaSession' in navigator)) return;

  const artwork = ttsNowPlaying.artwork
    ? [{ src: ttsNowPlaying.artwork, sizes: '512x512', type: 'image/jpeg' }]
    : undefined;
  navigator.mediaSession.metadata = new MediaMetadata({
    title: ttsNowPlaying.title,
    artist: ttsNowPlaying.artist ?? 'Kobbler Reader',
    artwork
  });
  navigator.mediaSession.playbackState = playing ? 'playing' : 'paused';
  navigator.mediaSession.setActionHandler('play', () => resumeTts());
  navigator.mediaSession.setActionHandler('pause', () => pauseTts());
  navigator.mediaSession.setActionHandler('stop', () => stop());
}

function refreshMediaSessionState() {
  if (typeof navigator === 'undefined' || !('mediaSession' in navigator)) return;
  navigator.mediaSession.playbackState = playing ? 'playing' : 'paused';
}

/** Set notification metadata for the current chapter/book; takes effect on next play. */
export function setReadAlongNowPlaying(info: { title: string; artist?: string; artwork?: string }) {
  ttsNowPlaying = info;
  if (mode === 'tts' && ttsAudioCtx) setupMediaSession();
}

/**
 * Skip TTS playback to the sentence whose text best matches the tapped
 * context, by aborting the current synth and re-synthesising the rest of
 * the page from that sentence onward. Earlier versions just re-anchored
 * the playback clock without rescheduling audio \u2014 silent no-op.
 */
export async function skipToSentence(contextText: string) {
  if (mode !== 'tts' || !ttsAudioCtx || ttsSentences.length === 0) return;
  if (!getPageTextCallback) return;

  const normalize = (s: string) =>
    s
      .normalize('NFC')
      .replace(/[\u2018\u2019\u201A\u201B]/g, "'")
      .replace(/[\u201C\u201D\u201E\u201F]/g, '"')
      .replace(/[\u2013\u2014]/g, '-')
      .replace(/\s+/g, ' ')
      .toLowerCase();

  const normalizedContext = normalize(contextText);

  // Find the sentence that best overlaps with the tapped text. Same fuzzy
  // scoring as before \u2014 prefer a sentence wholly contained in the tap
  // context, then a tap fully covering a sentence, then word-overlap.
  let bestIdx = -1;
  let bestScore = -1;
  for (let i = 0; i < ttsSentences.length; i++) {
    const sentNorm = normalize(ttsSentences[i].text);
    if (normalizedContext.includes(sentNorm)) {
      const score = 1000 - sentNorm.length;
      if (score > bestScore) {
        bestScore = score;
        bestIdx = i;
      }
    } else if (sentNorm.includes(normalizedContext)) {
      const score = 500 + normalizedContext.length;
      if (score > bestScore) {
        bestScore = score;
        bestIdx = i;
      }
    } else {
      const contextWordList = normalizedContext.split(/\s+/).filter((w) => w.length > 2);
      const sentWords = sentNorm.split(/\s+/).filter((w) => w.length > 2);
      let overlap = 0;
      for (const w of sentWords) {
        if (contextWordList.includes(w)) overlap++;
      }
      if (sentWords.length > 0) {
        const score = (overlap / sentWords.length) * 100;
        if (score > bestScore && score > 30) {
          bestScore = score;
          bestIdx = i;
        }
      }
    }
  }
  if (bestIdx < 0) return;

  const targetSentence = ttsSentences[bestIdx].text;
  // Span, not page: the tap re-anchors the read, and what follows it should be
  // read the same way the rest of the chapter would have been.
  const pageText = (await fetchSpanText()) ?? getPageTextCallback();
  const offset = locateSentenceOffsetInPage(pageText, targetSentence);
  if (offset < 0) {
    console.warn('skipToSentence: could not locate target sentence in page text');
    return;
  }
  const sliced = pageText.slice(offset);

  ttsAbortController?.abort();
  cleanupTtsAudio();
  clearHighlightCallback?.();

  try {
    playing = true;
    await playSpan(sliced);
  } catch (e) {
    error = e instanceof Error ? e.message : 'TTS failed';
    playing = false;
  }
}

/**
 * Find where the target sentence text starts in the full page text.
 * Direct substring first, then a relaxed multi-word probe to tolerate the
 * sentence-packing whitespace normalisation applied by `split_paragraphs`
 * on the server.
 */
function locateSentenceOffsetInPage(pageText: string, sentenceText: string): number {
  if (!sentenceText) return -1;
  const direct = pageText.indexOf(sentenceText);
  if (direct >= 0) return direct;
  // Relaxed: take the first ~5 distinctive words and stitch them with
  // any-whitespace gaps. Catches cases where the server collapsed newlines.
  const words = sentenceText
    .split(/\s+/)
    .filter((w) => w.length > 3)
    .slice(0, 5);
  if (words.length === 0) return -1;
  const escaped = words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
  const re = new RegExp(escaped.join('\\s+'), 'i');
  const m = re.exec(pageText);
  return m ? m.index : -1;
}

/**
 * Restart TTS from the beginning of the current page with current voice/speed settings.
 * Called when voice or other settings change mid-playback.
 */
export async function restartTtsWithCurrentSettings() {
  if (mode !== 'tts' || !getPageTextCallback) return;

  ttsAbortController?.abort();
  cleanupTtsAudio();
  clearHighlightCallback?.();

  try {
    playing = true;
    await synthesizeAndPlayPage();
  } catch (e) {
    error = e instanceof Error ? e.message : 'TTS failed';
    playing = false;
  }
}

export function pauseTts() {
  if (mode !== 'tts' || !ttsAudioCtx) return;
  playing = false;
  ttsAudioCtx.suspend();
  refreshMediaSessionState();
}

export function resumeTts() {
  if (mode !== 'tts' || !ttsAudioCtx) return;
  playing = true;
  ttsAudioCtx.resume();
  ttsBridgeEl?.play().catch(() => {});
  refreshMediaSessionState();
}

// ============================================================================
// Audiobook Sync Mode
// ============================================================================

export function startAudiobookSync(
  points: ReadAlongSyncPoint[],
  segments: ReadAlongTranscriptSegment[]
) {
  // Tear down any running TTS first — otherwise the AudioContext, bridge audio
  // element, and MediaSession metadata leak through the mode switch.
  if (mode === 'tts') {
    ttsAbortController?.abort();
    cleanupTtsAudio();
    clearHighlightCallback?.();
  }

  mode = 'audiobook';
  playing = true;
  syncPoints = points;
  transcriptSegments = segments;
  lastHighlightedSegmentId = '';

  startSyncLoop();
}

function startSyncLoop() {
  if (syncAnimFrame) cancelAnimationFrame(syncAnimFrame);

  const player = getAudioPlayerState();

  function tick() {
    if (mode !== 'audiobook') return;

    const time = player.globalCurrentTime;

    const segment = findSegmentAtGlobalTime(time, player);
    if (segment && segment.id !== lastHighlightedSegmentId) {
      lastHighlightedSegmentId = segment.id;
      currentSentence = segment.text;
      highlightCallback?.(segment.text);

      maybeAdvancePage(time, player);
    }

    syncAnimFrame = requestAnimationFrame(tick);
  }

  syncAnimFrame = requestAnimationFrame(tick);
}

type AudioPlayerLike = {
  globalTimeForFile: (fileId: string, seconds: number) => number;
};

function findSegmentAtGlobalTime(
  globalSeconds: number,
  player: AudioPlayerLike
): ReadAlongTranscriptSegment | null {
  let lo = 0;
  let hi = transcriptSegments.length - 1;
  let result: ReadAlongTranscriptSegment | null = null;

  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const seg = transcriptSegments[mid];
    const segGlobal = player.globalTimeForFile(seg.file_id, seg.start_seconds);
    if (segGlobal <= globalSeconds) {
      result = seg;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }

  return result;
}

function maybeAdvancePage(globalSeconds: number, player: AudioPlayerLike) {
  if (!goToCfiCallback || syncPoints.length === 0) return;

  let closestPoint: ReadAlongSyncPoint | null = null;
  for (const sp of syncPoints) {
    const spGlobal = player.globalTimeForFile(sp.audio_file_id, sp.audio_seconds);
    if (spGlobal <= globalSeconds) {
      closestPoint = sp;
    } else {
      break;
    }
  }

  if (closestPoint) {
    goToCfiCallback(closestPoint.ebook_cfi);
  }
}

// ============================================================================
// Shared Controls
// ============================================================================

export function stop() {
  const wasMode = mode;
  mode = 'off';
  playing = false;
  currentSentence = '';
  error = null;

  if (wasMode === 'tts') {
    ttsAbortController?.abort();
    cleanupTtsAudio();
  }

  if (syncAnimFrame) {
    cancelAnimationFrame(syncAnimFrame);
    syncAnimFrame = null;
  }

  clearHighlightCallback?.();
  lastHighlightedSegmentId = '';
}

export function togglePlayPause() {
  if (mode === 'tts') {
    if (playing) pauseTts();
    else resumeTts();
  }
  // Audiobook mode play/pause is handled by the audioplayer store
}

export function setVoice(v: string) {
  voice = v;
  lsSet('kobbler-tts-voice', v);
}

export function setSpeed(s: number) {
  // The engine's own WSOLA range. Clamping to it rather than something narrower
  // means a request never gets rejected and silently re-rendered at 1x.
  const next = Math.max(0.25, Math.min(4.0, s));
  const changed = next !== speed;
  speed = next;
  lsSet('kobbler-tts-speed', String(speed));
  // Rate is baked into the audio by the server, so there is nothing to adjust
  // on already-rendered buffers — the page is re-synthesised at the new rate.
  // That is the cost of pitch-preserved speed: a change means a re-request.
  if (changed && mode === 'tts' && playing) {
    void restartTtsWithCurrentSettings();
  }
}

export function setLang(l: string) {
  lang = l;
  lsSet('kobbler-tts-lang', l);
}

export function setContinuous(c: boolean) {
  continuous = c;
  lsSet('kobbler-tts-continuous', String(c));
}

export function setTemperature(v: number | null) {
  temperature = v;
  if (v != null) lsSet('kobbler-tts-temperature', String(v));
  else if (typeof window !== 'undefined') localStorage.removeItem('kobbler-tts-temperature');
}

/** Load the user's active voice preset from the API and apply its settings. */
export async function loadActivePreset(): Promise<UserVoicePreset | null> {
  if (presetLoaded) return null;
  presetLoaded = true;
  try {
    const preset = await getActivePreset();
    if (preset) {
      activePresetId = preset.id;
      lsSet('kobbler-tts-active-preset', preset.id);
      // Apply preset values (only if user hasn't overridden in localStorage)
      if (!lsGet('kobbler-tts-voice') && preset.voice) {
        voice = preset.voice;
      }
      if (!lsGet('kobbler-tts-speed')) {
        speed = preset.speed;
      }
      if (!lsGet('kobbler-tts-temperature') && preset.temperature != null) {
        temperature = preset.temperature;
      }
      return preset;
    }
  } catch {
    // API not available — use localStorage defaults
  }
  return null;
}

/**
 * Validate the currently-stored voice against the list of voices available
 * for the current language. Any selected IDs that aren't present are dropped.
 * If the result is empty, falls back to the first available voice.
 * Persists the corrected value back to localStorage.
 */
export function validateVoiceAgainst(availableIds: string[]) {
  if (availableIds.length === 0) return;
  // A preset owns the voice, and the server resolves it — a clip-backed preset
  // has no `voice` id in this list at all. Rewriting the stored voice here used
  // to also CLEAR the active preset, so every reader load silently dropped the
  // user's chosen narrator and fell back to whatever sorted first.
  if (activePresetId) return;
  const current = voice.split('+').filter((id) => id.length > 0);
  if (current.length === 0) return; // empty = "server picks"; nothing to validate
  const filtered = current.filter((id) => availableIds.includes(id));
  if (filtered.length === current.length) return; // all valid
  const next = filtered.length > 0 ? filtered.join('+') : availableIds[0];
  if (next === voice) return;
  voice = next;
  lsSet('kobbler-tts-voice', next);
}

/** Apply a preset's settings to the current session state. */
export function applyPreset(preset: UserVoicePreset) {
  activePresetId = preset.id;
  lsSet('kobbler-tts-active-preset', preset.id);
  if (preset.voice) {
    voice = preset.voice;
    lsSet('kobbler-tts-voice', preset.voice);
  }
  speed = preset.speed;
  lsSet('kobbler-tts-speed', String(preset.speed));
  temperature = preset.temperature;
  if (preset.temperature != null) lsSet('kobbler-tts-temperature', String(preset.temperature));
}

// ============================================================================
// State Accessor
// ============================================================================

export function getReadAlongState() {
  return {
    get mode() {
      return mode;
    },
    get playing() {
      return playing;
    },
    get currentSentence() {
      return currentSentence;
    },
    get speed() {
      return speed;
    },
    get voice() {
      return voice;
    },
    get lang() {
      return lang;
    },
    get continuous() {
      return continuous;
    },
    get loading() {
      return loading;
    },
    get error() {
      return error;
    },
    get temperature() {
      return temperature;
    },
    get activePresetId() {
      return activePresetId;
    },
    get ttsAudioCurrentTime() {
      return Math.max(0, ttsElapsedSeconds());
    },
    get currentWordIndex() {
      return currentWordIndex;
    },
    get currentWord() {
      return currentWord;
    }
  };
}

// ============================================================================
// Utilities
// ============================================================================

function base64ToArrayBuffer(base64: string): ArrayBuffer {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes.buffer;
}

/**
 * When the next buffer should start, and the book-keeping that goes with it.
 *
 * The first buffer of a render is placed `SCHEDULE_LEAD_S` ahead of the clock
 * and becomes the playback timebase — word timings are absolute ms from the
 * start of the AUDIO, not from when the request went out. After that it is
 * "when the previous one ends", clamped so a chunk that arrived too late is not
 * scheduled in the past.
 *
 * A clamped chunk leaves real silence on the output, and `ctx.currentTime` runs
 * straight through it. Accumulating that silence is what keeps the highlight
 * tracking audio rather than wall clock — without it every underrun leaves the
 * highlight permanently further ahead of the voice.
 */
function scheduleStartTime(ctx: AudioContext): number {
  if (ttsPlaybackStartCtxTime < 0) {
    const startAt = ctx.currentTime + SCHEDULE_LEAD_S;
    ttsPlaybackStartCtxTime = startAt;
    return startAt;
  }
  const startAt = Math.max(ttsNextStartTime, ctx.currentTime);
  if (startAt > ttsNextStartTime) {
    ttsGapSeconds += startAt - ttsNextStartTime;
  }
  return startAt;
}

/**
 * Decode 16-bit signed little-endian PCM (mono) into an AudioBuffer.
 * The engine emits `response_format: "pcm"` as raw s16le samples — no header,
 * no compression. Convert to Float32 in [-1, 1] and copy into a single-channel
 * AudioBuffer at the upstream sample rate (24 kHz today).
 */
function pcm16ToAudioBuffer(ctx: AudioContext, pcm: ArrayBuffer, sampleRate: number): AudioBuffer {
  const samples = new Int16Array(pcm);
  const floats = new Float32Array(samples.length);
  for (let i = 0; i < samples.length; i++) {
    floats[i] = samples[i] / 32768;
  }
  const buf = ctx.createBuffer(1, samples.length, sampleRate);
  buf.copyToChannel(floats, 0);
  return buf;
}

/**
 * JS-side sentence split mirroring the Rust `split_sentences`: paragraph
 * breaks first, then sentence-end punctuation followed by whitespace, with
 * short trailing fragments merged back into the sentence before them. Kept in
 * step with `api/src/tts/mod.rs` so both sides cut the passage the same way.
 */
function splitPassageSentences(text: string): string[] {
  const out: string[] = [];
  for (const paragraph of text.split('\n\n')) {
    const para = paragraph.trim();
    if (!para) continue;
    const re = /([.!?;])\s+/g;
    const chunks: string[] = [];
    let lastEnd = 0;
    let m: RegExpExecArray | null;
    while ((m = re.exec(para)) !== null) {
      const chunk = para.slice(lastEnd, m.index + m[1].length).trim();
      if (chunk) chunks.push(chunk);
      lastEnd = re.lastIndex;
    }
    const remainder = para.slice(lastEnd).trim();
    if (remainder) chunks.push(remainder);
    for (const chunk of chunks) {
      const prev = out.length > 0 ? out[out.length - 1] : null;
      if (prev !== null && chunk.length < 10 && !/[.!?]$/.test(chunk)) {
        out[out.length - 1] = `${prev} ${chunk}`;
      } else {
        out.push(chunk);
      }
    }
  }
  return out;
}

/** Whitespace tokens, counted the way the aligner counts them. */
function countWords(text: string): number {
  return text.split(/\s+/).filter((w) => w.length > 0).length;
}

/**
 * Cut a synthesised passage into sentence entries and append them.
 *
 * `wordOffset` is where this passage starts in the aligner's token sequence
 * (0 today — one request covers the passage). Each entry gets its own span in
 * that sequence, which is what the sync loop matches the current word against.
 * Times start out empty and are filled in by [refreshSentenceTimesFromWords]
 * as alignment arrives.
 */
function appendSentenceEntries(text: string, wordOffset: number) {
  let cursor = wordOffset;
  for (const sentence of splitPassageSentences(text)) {
    const words = countWords(sentence);
    ttsSentences.push({
      text: sentence,
      startTime: Number.POSITIVE_INFINITY,
      endTime: Number.POSITIVE_INFINITY,
      wordOffset: cursor,
      wordCount: words
    });
    cursor += words;
  }
  ttsLastHighlightedIdx = -1;
  ttsLastHighlightedWordIdx = -1;
}

/**
 * Give every sentence entry the start/end time of its first and last aligned
 * word. Entries the aligner has not reached yet keep their infinite span, so
 * nothing matches them by time until they are real.
 */
function refreshSentenceTimesFromWords() {
  if (ttsWords.length === 0) return;
  for (const entry of ttsSentences) {
    if (entry.wordOffset === undefined || entry.wordCount === undefined) continue;
    const first = findWordByGlobalIndex(entry.wordOffset);
    const last = findWordByGlobalIndex(entry.wordOffset + entry.wordCount - 1);
    if (first) entry.startTime = first.t0Ms / 1000;
    if (last) entry.endTime = last.t1Ms / 1000;
  }
}

/**
 * Time the sentence entries from the chunk anchors.
 *
 * A chunk's duration is spread across its words by count, which gives every
 * sentence in it a provisional span. Coarse — it cannot know that one word took
 * longer than another — but it tracks the voice to within a sentence, which is
 * all the reader needs to keep the right page in front of the listener.
 *
 * The newest anchor has no successor to bound it, so it is closed with the rate
 * the earlier anchors actually read at (seeded at [ANCHOR_SEED_WPM] until there
 * is one). Every anchor before it is bounded exactly, by where the next one
 * said its audio starts.
 *
 * Callers re-run [refreshSentenceTimesFromWords] afterwards: a real alignment
 * beats an interpolation everywhere the two cover the same words.
 */
function applyAnchorTimes() {
  if (ttsAnchors.length === 0) return;

  // Every closed pair, so the estimate sharpens as the read goes on.
  let closedWords = 0;
  let closedMs = 0;
  for (let i = 0; i + 1 < ttsAnchors.length; i++) {
    closedWords += ttsAnchors[i].wordCount;
    closedMs += ttsAnchors[i + 1].audioOffsetMs - ttsAnchors[i].audioOffsetMs;
  }
  const wpm = closedMs > 0 ? (closedWords / closedMs) * 60_000 : ANCHOR_SEED_WPM;

  const endOf = (i: number): number => {
    const a = ttsAnchors[i];
    const next = ttsAnchors[i + 1];
    return next ? next.audioOffsetMs : a.audioOffsetMs + (a.wordCount / wpm) * 60_000;
  };

  for (const entry of ttsSentences) {
    if (entry.wordOffset === undefined || entry.wordCount === undefined) continue;
    const i = findAnchorAtWord(entry.wordOffset);
    if (i < 0) continue;
    const a = ttsAnchors[i];
    if (a.wordCount <= 0) continue;
    const spanMs = endOf(i) - a.audioOffsetMs;
    if (spanMs <= 0) continue;
    const perWordMs = spanMs / a.wordCount;
    // Clamped to the anchor: a sentence that runs past the chunk boundary ends
    // where the chunk does rather than borrowing time the next one owns.
    const from = Math.max(0, entry.wordOffset - a.wordOffset);
    const to = Math.min(a.wordCount, from + entry.wordCount);
    entry.startTime = (a.audioOffsetMs + from * perWordMs) / 1000;
    entry.endTime = (a.audioOffsetMs + to * perWordMs) / 1000;
  }
}

/** Which anchor owns this word index, or -1. */
function findAnchorAtWord(globalWordIndex: number): number {
  for (let i = 0; i < ttsAnchors.length; i++) {
    const a = ttsAnchors[i];
    if (globalWordIndex >= a.wordOffset && globalWordIndex < a.wordOffset + a.wordCount) return i;
  }
  return -1;
}

function findWordByGlobalIndex(globalWordIndex: number) {
  let lo = 0;
  let hi = ttsWords.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const w = ttsWords[mid];
    if (w.globalWordIndex === globalWordIndex) return w;
    if (w.globalWordIndex < globalWordIndex) lo = mid + 1;
    else hi = mid - 1;
  }
  return null;
}

/**
 * Which sentence entry owns this token index. Returns -1 when no entry does —
 * the unaligned engines' entries carry no word span at all.
 */
function findTtsSentenceAtWord(globalWordIndex: number): number {
  for (let i = 0; i < ttsSentences.length; i++) {
    const e = ttsSentences[i];
    if (e.wordOffset === undefined || e.wordCount === undefined) continue;
    if (globalWordIndex >= e.wordOffset && globalWordIndex < e.wordOffset + e.wordCount) return i;
  }
  return -1;
}

/**
 * Merge an FA word list into `ttsWords` keyed by globalWordIndex.
 *
 * ⚠️ Overwrite by index, NEVER append. A partial carries only the prefix the
 * voice has reached. The prefix is monotonic — the engine clamps it to a
 * high-water mark — but a word's TIMINGS are re-estimated on every pass and do
 * change, which is exactly what overwriting absorbs. Appending instead would
 * duplicate the passage many times over.
 */
/// Reconcile the rate the server actually rendered at with the one the UI is
/// claiming.
///
/// A refused rate is not an error — real audio comes back, just at 1x — so the
/// only thing that goes wrong is the speed control sitting on "2x" over audio
/// that plainly isn't. Snap the control to the truth and say why once.
function noteAppliedSpeed(applied: unknown) {
  if (typeof applied !== 'number' || !Number.isFinite(applied) || applied <= 0) return;
  if (Math.abs(applied - ttsRequestedSpeed) < 1e-9) return;
  ttsRequestedSpeed = applied;
  speed = applied;
  lsSet('kobbler-tts-speed', String(applied));
  error = `The voice engine can't do that speed — playing at ${applied}x.`;
}

function mergeWordTimings(
  words: Array<{ word_index: number; text: string; t0_ms: number; t1_ms: number }>,
  wordOffset: number,
  audioOffsetMs: number
) {
  // Binary search, not a linear scan: every partial re-sends the whole prefix
  // it has reached, so over a span of a thousand words a scan per word is
  // quadratic in the length of the passage and starves the highlight loop.
  let outOfOrder = false;
  for (const w of words) {
    const globalWordIndex = wordOffset + w.word_index;
    // No rate arithmetic: the aligner ran against the audio as rendered, so
    // these are already in the same timebase the sync loop measures.
    const t0 = audioOffsetMs + w.t0_ms;
    const t1 = audioOffsetMs + w.t1_ms;
    const existing = findWordByGlobalIndex(globalWordIndex);
    if (existing) {
      existing.text = w.text;
      existing.t0Ms = t0;
      existing.t1Ms = t1;
      continue;
    }
    const last = ttsWords[ttsWords.length - 1];
    if (last && last.globalWordIndex > globalWordIndex) outOfOrder = true;
    ttsWords.push({ text: w.text, globalWordIndex, t0Ms: t0, t1Ms: t1 });
  }
  if (outOfOrder) ttsWords.sort((a, b) => a.globalWordIndex - b.globalWordIndex);
}

/**
 * Binary-search ttsWords for the word active at the given playback time (ms).
 * Returns -1 when before the first word or after the last word's end.
 */
function findTtsWordAtMs(ms: number): number {
  if (ttsWords.length === 0) return -1;
  let lo = 0;
  let hi = ttsWords.length - 1;
  let result = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const w = ttsWords[mid];
    if (ms < w.t0Ms) {
      hi = mid - 1;
    } else if (ms >= w.t1Ms) {
      result = mid; // remember last-seen-so-far in case we're between words
      lo = mid + 1;
    } else {
      return mid;
    }
  }
  // Between words → return the most-recent past word so the highlight sticks
  // through pause gaps rather than flickering off.
  return result;
}
