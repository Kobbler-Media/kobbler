// Audiobook player state store using Svelte 5 runes

import {
  getAudioStreamUrl,
  getBookChapters,
  getAudioPosition,
  saveAudioPosition
} from '#lib/api/client.js';
import type { AudiobookChapter, Book, PlaybackLoudness, UnifiedChapter } from '#lib/types.js';
import { levellingGain } from '#lib/streaming/loudness.js';
import { bumpWatchStateVersion } from '#lib/stores/watchState.svelte.js';

export type SleepTimerMode = 'off' | '15' | '30' | '45' | '60' | 'end_of_chapter' | 'inactivity';

// Module-level state (singleton)
let currentBook = $state<Book | null>(null);
let currentFileId = $state<string | null>(null);
let fileIds = $state<string[]>([]); // All audio file IDs for multi-file support
let fileDurations = $state<number[]>([]); // Per-file duration in seconds, parallel to fileIds
let currentTrackIndex = $state(0);
let playing = $state(false);
let currentTime = $state(0);
let duration = $state(0);
let playbackRate = $state(1);
let volume = $state(1);
let chapters = $state<AudiobookChapter[]>([]);
let unifiedChapters = $state<UnifiedChapter[]>([]);
let sleepTimerMode = $state<SleepTimerMode>('off');
let sleepTimerRemaining = $state(0);
let loading = $state(false);
let error = $state<string | null>(null);
let inactivityMinutes = $state(
  typeof localStorage !== 'undefined'
    ? parseInt(localStorage.getItem('audioplayer:inactivityMinutes') || '10', 10)
    : 10
);
let lastInteractionTimestamp = 0;
let inactivityCheckInterval: ReturnType<typeof setInterval> | null = null;

let skipSilence = $state(
  typeof localStorage !== 'undefined'
    ? localStorage.getItem('audioplayer:skipSilence') !== 'false'
    : true
);

let audioElement: HTMLAudioElement | null = null;
// Forward declaration for track advancement (assigned after advanceTrack is defined)
let advanceTrackFn: (trackIndex: number) => Promise<void> = async () => {};
let saveInterval: ReturnType<typeof setInterval> | null = null;
let sleepInterval: ReturnType<typeof setInterval> | null = null;
let lastSavedTime = 0;

// Web Audio API state
let audioContext: AudioContext | null = null;
let analyserNode: AnalyserNode | null = null;
let sourceNode: MediaElementAudioSourceNode | null = null;
let gainNode: GainNode | null = null;
let compressorNode: DynamicsCompressorNode | null = null;

// Volume normalization + night mode state
let normalizeVolume = $state(
  typeof localStorage !== 'undefined'
    ? localStorage.getItem('audioplayer:normalizeVolume') !== 'false'
    : true
);
let nightMode = $state(
  typeof localStorage !== 'undefined'
    ? localStorage.getItem('audioplayer:nightMode') === 'true'
    : false
);
let fileLoudness: Record<string, PlaybackLoudness> = {};

// Smart resume: track when playback was last paused
let lastPauseTimestamp = 0;
// The listener's pause intent. Crossing a file boundary rebuilds the element's
// source, and without this a chapter jump or a skip past the end of the current
// file would start playing under a listener who had deliberately paused. Only
// an explicit play clears it.
let userPaused = false;
let silenceCheckInterval: ReturnType<typeof setInterval> | null = null;
const SILENCE_THRESHOLD = 0.005;
const SILENCE_DURATION_MS = 700;
const SILENCE_SKIP_SPEED = 2;
// Min ms at normal rate after a skip ends — avoids flapping between
// 1× and skip-rate on tight inter-word gaps.
const REARM_COOLDOWN_MS = 250;
// Web Audio gain ramp times around the rate change. Brief duck masks the
// time-stretcher's resync click and any first-sample chipmunk artifact.
const DUCK_DOWN_S = 0.015;
const DUCK_UP_S = 0.06;
let silenceSince: number | null = null;
let normalRateUntil = 0;
let preSkipRate = 1;

function createAudioElement(): HTMLAudioElement {
  if (audioElement) return audioElement;
  audioElement = new Audio();
  audioElement.preload = 'auto';

  audioElement.addEventListener('timeupdate', () => {
    currentTime = audioElement!.currentTime;
    updateMediaSessionPosition();
  });

  audioElement.addEventListener('durationchange', () => {
    duration = audioElement!.duration || 0;
  });

  audioElement.addEventListener('ended', () => {
    // Auto-advance to next track in multi-file audiobooks
    if (fileIds.length > 1 && currentTrackIndex < fileIds.length - 1) {
      advanceTrackFn(currentTrackIndex + 1);
    } else {
      playing = false;
    }
  });

  audioElement.addEventListener('play', () => {
    playing = true;
    userPaused = false;
    updateMediaSession();
    if (skipSilence) startSilenceDetection();
  });

  audioElement.addEventListener('pause', () => {
    // Reaching the end of a file fires pause before ended — that's the source
    // running out, not the listener pausing, and auto-advance depends on it.
    if (!audioElement!.ended) userPaused = true;
    playing = false;
    updateMediaSession();
    stopSilenceDetection();
  });

  // While paused, timeupdate never fires, so these carry a seek (including one
  // applied as a default playback start position on a freshly-loaded file)
  // through to the UI.
  const syncTime = () => {
    currentTime = audioElement!.currentTime;
    updateMediaSessionPosition();
  };
  audioElement.addEventListener('seeked', syncTime);
  audioElement.addEventListener('loadedmetadata', syncTime);

  audioElement.addEventListener('error', () => {
    error = 'Failed to load audio';
    playing = false;
    loading = false;
  });

  audioElement.addEventListener('canplay', () => {
    loading = false;
  });

  setupMediaSessionHandlers();

  return audioElement;
}

// =============================================================================
// Media Session API (lock screen / notification controls)
// =============================================================================

function updateMediaSession() {
  if (typeof navigator === 'undefined' || !('mediaSession' in navigator) || !currentBook) return;

  const artwork: { src: string; sizes?: string; type?: string }[] = [];
  if (currentBook.cover_url) {
    artwork.push({ src: currentBook.cover_url, sizes: '512x512', type: 'image/jpeg' });
  }

  const chapter = getCurrentChapter();

  navigator.mediaSession.metadata = new MediaMetadata({
    title: chapter?.title ?? currentBook.title,
    artist: currentBook.narrator ?? (currentBook.authors?.[0]?.name || ''),
    album: currentBook.title,
    artwork
  });

  navigator.mediaSession.playbackState = playing ? 'playing' : 'paused';
}

function setupMediaSessionHandlers() {
  if (typeof navigator === 'undefined' || !('mediaSession' in navigator)) return;

  navigator.mediaSession.setActionHandler('play', () => {
    audioElement?.play();
    startSaveInterval();
  });
  navigator.mediaSession.setActionHandler('pause', () => pause());
  navigator.mediaSession.setActionHandler('seekbackward', () => skipBackward(30));
  navigator.mediaSession.setActionHandler('seekforward', () => skipForward(30));
  navigator.mediaSession.setActionHandler('previoustrack', () => {
    if (currentTrackIndex > 0) advanceTrackFn(currentTrackIndex - 1);
  });
  navigator.mediaSession.setActionHandler('nexttrack', () => {
    if (currentTrackIndex < fileIds.length - 1) advanceTrackFn(currentTrackIndex + 1);
  });
  navigator.mediaSession.setActionHandler('seekto', (details) => {
    if (details.seekTime != null) seek(details.seekTime);
  });
}

function updateMediaSessionPosition() {
  if (typeof navigator === 'undefined' || !('mediaSession' in navigator) || !audioElement) return;
  const total = totalBookDuration();
  // Prefer the global book timeline so OS-level controls reflect the whole audiobook;
  // fall back to the current file's duration if we don't have per-file durations yet.
  const exposedDuration = total > 0 ? total : duration;
  if (!isFinite(exposedDuration) || exposedDuration <= 0) return;
  const exposedPosition = total > 0 ? currentGlobalTime() : currentTime;
  navigator.mediaSession.setPositionState({
    duration: exposedDuration,
    playbackRate,
    position: Math.min(exposedPosition, exposedDuration)
  });
}

// =============================================================================
// Skip Silence (Web Audio API)
// =============================================================================

async function ensureAudioContext() {
  if (audioContext || !audioElement) return;
  audioContext = new AudioContext();
  analyserNode = audioContext.createAnalyser();
  analyserNode.fftSize = 2048;
  gainNode = audioContext.createGain();
  compressorNode = audioContext.createDynamicsCompressor();
  sourceNode = audioContext.createMediaElementSource(audioElement);
  // Chain: source → gain (normalization) → compressor (night mode) → analyser (silence detect) → output
  sourceNode.connect(gainNode);
  gainNode.connect(compressorNode);
  compressorNode.connect(analyserNode);
  analyserNode.connect(audioContext.destination);
  applyNormalizationGain();
  applyNightMode();
  if (audioContext.state === 'suspended') {
    await audioContext.resume();
  }
}

function targetNormalizationGain(): number {
  if (!normalizeVolume || !currentFileId) return 1.0;
  return levellingGain(fileLoudness[currentFileId] ?? {});
}

function setSource(audio: HTMLAudioElement, bookId: string, fileId: string) {
  audio.src = getAudioStreamUrl(bookId, fileId);
  applyNormalizationGain();
}

function applyNormalizationGain() {
  if (!gainNode) return;
  gainNode.gain.value = targetNormalizationGain();
}

/// Briefly duck via the gainNode while the rate change happens, masking
/// the time-stretcher's resync click and the first-sample artifact.
function rampGainAroundRateChange(applyRateChange: () => void) {
  if (!gainNode || !audioContext) {
    applyRateChange();
    return;
  }
  const t = audioContext.currentTime;
  const target = targetNormalizationGain();
  gainNode.gain.cancelScheduledValues(t);
  gainNode.gain.setValueAtTime(gainNode.gain.value, t);
  gainNode.gain.linearRampToValueAtTime(0, t + DUCK_DOWN_S);
  setTimeout(applyRateChange, DUCK_DOWN_S * 1000);
  gainNode.gain.linearRampToValueAtTime(target, t + DUCK_DOWN_S + DUCK_UP_S);
}

function applyNightMode() {
  if (!compressorNode) return;
  if (nightMode) {
    compressorNode.threshold.value = -24;
    compressorNode.knee.value = 30;
    compressorNode.ratio.value = 12;
    compressorNode.attack.value = 0.003;
    compressorNode.release.value = 0.25;
  } else {
    compressorNode.threshold.value = 0;
    compressorNode.ratio.value = 1;
  }
}

async function startSilenceDetection() {
  stopSilenceDetection();
  if (!audioElement) return;
  await ensureAudioContext();
  if (!analyserNode) return;

  const dataArray = new Float32Array(analyserNode.fftSize);

  silenceCheckInterval = setInterval(() => {
    if (!playing || !audioElement || !analyserNode || !skipSilence) return;

    analyserNode.getFloatTimeDomainData(dataArray);

    // Calculate RMS amplitude
    let sum = 0;
    for (let i = 0; i < dataArray.length; i++) {
      sum += dataArray[i] * dataArray[i];
    }
    const rms = Math.sqrt(sum / dataArray.length);

    const now = performance.now();

    if (rms < SILENCE_THRESHOLD) {
      // Cooldown after a recent skip ended — don't re-arm yet.
      if (now < normalRateUntil) {
        silenceSince = null;
        return;
      }
      if (silenceSince === null) {
        silenceSince = now;
      } else if (now - silenceSince > SILENCE_DURATION_MS) {
        if (audioElement.playbackRate !== SILENCE_SKIP_SPEED) {
          preSkipRate = playbackRate;
          // Speeding up during real silence is inaudible — no duck needed.
          audioElement.playbackRate = SILENCE_SKIP_SPEED;
        }
      }
    } else {
      // Sound returned — revert immediately and gain-duck the rate change so
      // the time-stretcher's resync doesn't click. Cooldown prevents flapping
      // on tight inter-word gaps.
      if (audioElement.playbackRate === SILENCE_SKIP_SPEED) {
        rampGainAroundRateChange(() => {
          if (audioElement) audioElement.playbackRate = preSkipRate;
        });
        normalRateUntil = now + REARM_COOLDOWN_MS;
      }
      silenceSince = null;
    }
  }, 50); // Check every 50ms
}

function stopSilenceDetection() {
  if (silenceCheckInterval) {
    clearInterval(silenceCheckInterval);
    silenceCheckInterval = null;
  }
  silenceSince = null;
  normalRateUntil = 0;
  if (audioElement && audioElement.playbackRate === SILENCE_SKIP_SPEED) {
    audioElement.playbackRate = playbackRate;
    if (gainNode) gainNode.gain.value = targetNormalizationGain();
  }
}

export function setSkipSilence(enabled: boolean) {
  skipSilence = enabled;
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem('audioplayer:skipSilence', String(enabled));
  }
  if (enabled && playing) {
    startSilenceDetection();
  } else {
    stopSilenceDetection();
  }
}

function startSaveInterval() {
  stopSaveInterval();
  saveInterval = setInterval(() => {
    if (currentBook && currentFileId && Math.abs(currentTime - lastSavedTime) > 2) {
      lastSavedTime = currentTime;
      const total = totalBookDuration();
      const progress = total > 0 ? (currentGlobalTime() / total) * 100 : 0;
      saveAudioPosition(currentBook.id, {
        file_id: currentFileId,
        position_seconds: Math.floor(currentTime),
        progress_percent: Math.round(progress)
      }).catch(() => {
        // Best effort
      });
    }
  }, 30_000);
}

function stopSaveInterval() {
  if (saveInterval) {
    clearInterval(saveInterval);
    saveInterval = null;
  }
}

function startSleepTimer(minutes: number) {
  stopSleepTimer();
  sleepTimerRemaining = minutes * 60;
  sleepInterval = setInterval(() => {
    sleepTimerRemaining -= 1;
    if (sleepTimerRemaining <= 0) {
      pause();
      setSleepTimer('off');
    }
  }, 1000);
}

function stopSleepTimer() {
  if (sleepInterval) {
    clearInterval(sleepInterval);
    sleepInterval = null;
  }
  sleepTimerRemaining = 0;
}

function computeFileOffsets(): number[] {
  const offsets = new Array<number>(fileDurations.length);
  let acc = 0;
  for (let i = 0; i < fileDurations.length; i++) {
    offsets[i] = acc;
    acc += fileDurations[i] || 0;
  }
  return offsets;
}

function totalBookDuration(): number {
  let total = 0;
  for (let i = 0; i < fileDurations.length; i++) total += fileDurations[i] || 0;
  return total;
}

function globalTimeForFile(fileId: string, fileSeconds: number): number {
  const idx = fileIds.indexOf(fileId);
  if (idx < 0) return fileSeconds;
  const offsets = computeFileOffsets();
  return (offsets[idx] || 0) + fileSeconds;
}

function currentGlobalTime(): number {
  const offsets = computeFileOffsets();
  return (offsets[currentTrackIndex] || 0) + currentTime;
}

function chaptersForFile(fileId: string): AudiobookChapter[] {
  return unifiedChapters
    .filter((c) => c.file_id === fileId)
    .map((c) => ({
      id: c.id,
      chapter_index: c.chapter_index,
      title: c.title,
      start_seconds: c.start_seconds,
      end_seconds: c.end_seconds
    }));
}

function getCurrentChapter(): AudiobookChapter | null {
  if (chapters.length === 0) return null;
  for (let i = chapters.length - 1; i >= 0; i--) {
    if (currentTime >= chapters[i].start_seconds) {
      return chapters[i];
    }
  }
  return chapters[0];
}

export async function play(
  book: Book,
  audioFileIds: string[],
  audioFileDurations: number[] = [],
  audioFileLoudness: Record<string, PlaybackLoudness> = {}
) {
  if (audioFileIds.length === 0) return;
  const audio = createAudioElement();
  error = null;
  userPaused = false;

  const firstFileId = audioFileIds[0];

  // If same book and just paused, resume
  if (currentBook?.id === book.id && audio.src) {
    await audio.play();
    startSaveInterval();
    return;
  }

  // New audiobook
  loading = true;
  currentBook = book;
  fileIds = audioFileIds;
  fileLoudness = audioFileLoudness;
  fileDurations =
    audioFileDurations.length === audioFileIds.length
      ? audioFileDurations.map((d) => (isFinite(d) && d > 0 ? d : 0))
      : new Array(audioFileIds.length).fill(0);
  currentTrackIndex = 0;
  let fileId = firstFileId;
  currentFileId = fileId;

  // Load unified chapters (all files)
  try {
    unifiedChapters = await getBookChapters(book.id);
    chapters = chaptersForFile(fileId);
  } catch {
    unifiedChapters = [];
    chapters = [];
  }

  // Set source
  setSource(audio, book.id, fileId);
  audio.playbackRate = playbackRate;
  audio.volume = volume;

  // Restore saved position
  try {
    const saved = await getAudioPosition(book.id);
    if (saved && saved.file_id === fileId && saved.position_seconds > 0) {
      audio.currentTime = saved.position_seconds;
      currentTime = saved.position_seconds;
    } else if (
      saved &&
      saved.file_id &&
      fileIds.includes(saved.file_id) &&
      saved.file_id !== fileId
    ) {
      // Saved position is on a different track — switch to it
      const savedIndex = fileIds.indexOf(saved.file_id);
      if (savedIndex >= 0) {
        currentTrackIndex = savedIndex;
        currentFileId = saved.file_id;
        setSource(audio, book.id, saved.file_id);
        if (saved.position_seconds > 0) {
          audio.currentTime = saved.position_seconds;
          currentTime = saved.position_seconds;
        }
        chapters = chaptersForFile(saved.file_id);
      }
    }
  } catch {
    // Start from beginning
  }

  try {
    await audio.play();
  } catch {
    error = 'Failed to start playback';
    loading = false;
  }

  startSaveInterval();
}

async function advanceTrack(trackIndex: number) {
  if (!currentBook || trackIndex < 0 || trackIndex >= fileIds.length) return;

  flushPosition();
  const nextFileId = fileIds[trackIndex];
  currentTrackIndex = trackIndex;
  currentFileId = nextFileId;
  loading = true;

  const audio = createAudioElement();

  // Update chapters for the new track (unified chapters already loaded)
  chapters = chaptersForFile(nextFileId);

  setSource(audio, currentBook.id, nextFileId);
  audio.playbackRate = playbackRate;
  audio.volume = volume;
  currentTime = 0;

  // Paused listener: load the new file and leave it paused. The caller's seek
  // lands as a default playback start position and `canplay` clears `loading`.
  if (userPaused) return;

  try {
    await audio.play();
  } catch {
    error = 'Failed to start next track';
    loading = false;
  }
}
advanceTrackFn = advanceTrack;

export function pause() {
  userPaused = true;
  audioElement?.pause();
  lastPauseTimestamp = Date.now();
  flushPosition();
}

export function togglePlayPause() {
  if (!audioElement || !currentBook) return;
  if (playing) {
    pause();
  } else {
    // Smart resume: rewind a few seconds when resuming after a long pause
    if (lastPauseTimestamp > 0) {
      const elapsed = Date.now() - lastPauseTimestamp;
      let rewindSecs = 0;
      if (elapsed > 5 * 60 * 1000) rewindSecs = 10;
      else if (elapsed > 60 * 1000) rewindSecs = 5;
      if (rewindSecs > 0) {
        audioElement.currentTime = Math.max(0, audioElement.currentTime - rewindSecs);
      }
    }
    userPaused = false;
    audioElement.play();
    startSaveInterval();
  }
}

export function seek(seconds: number) {
  if (!audioElement) return;
  // A file that hasn't reported its metadata yet has duration 0 — clamping to
  // it would collapse every seek into the newly-loaded file to position 0.
  audioElement.currentTime = Math.max(0, Math.min(seconds, duration > 0 ? duration : seconds));
}

export function seekToGlobal(globalSeconds: number) {
  const total = totalBookDuration();
  if (total <= 0 || fileIds.length === 0) {
    seek(globalSeconds);
    return;
  }
  const target = Math.max(0, Math.min(globalSeconds, total));
  const offsets = computeFileOffsets();
  let idx = 0;
  for (let i = 0; i < offsets.length; i++) {
    const end = offsets[i] + (fileDurations[i] || 0);
    if (target < end || i === offsets.length - 1) {
      idx = i;
      break;
    }
  }
  const localSeconds = target - offsets[idx];
  const targetFileId = fileIds[idx];
  if (targetFileId !== currentFileId) {
    advanceTrackFn(idx).then(() => seek(localSeconds));
    return;
  }
  seek(localSeconds);
}

export function skipForward(seconds = 30) {
  if (!audioElement) return;
  if (totalBookDuration() > 0 && fileIds.length > 1) {
    seekToGlobal(currentGlobalTime() + seconds);
    return;
  }
  audioElement.currentTime = Math.min(audioElement.currentTime + seconds, duration);
}

export function skipBackward(seconds = 30) {
  if (!audioElement) return;
  if (totalBookDuration() > 0 && fileIds.length > 1) {
    seekToGlobal(Math.max(0, currentGlobalTime() - seconds));
    return;
  }
  audioElement.currentTime = Math.max(audioElement.currentTime - seconds, 0);
}

export function setRate(rate: number) {
  playbackRate = Math.max(0.5, Math.min(3, rate));
  if (audioElement) {
    audioElement.playbackRate = playbackRate;
  }
}

export function setVolume(vol: number) {
  volume = Math.max(0, Math.min(1, vol));
  if (audioElement) {
    audioElement.volume = volume;
  }
}

export function setNormalizeVolume(enabled: boolean) {
  normalizeVolume = enabled;
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem('audioplayer:normalizeVolume', String(enabled));
  }
  applyNormalizationGain();
}

export function setNightMode(enabled: boolean) {
  nightMode = enabled;
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem('audioplayer:nightMode', String(enabled));
  }
  applyNightMode();
}

function startInactivityTimer() {
  stopInactivityTimer();
  lastInteractionTimestamp = Date.now();
  inactivityCheckInterval = setInterval(() => {
    if (!playing) return;
    const idleMs = Date.now() - lastInteractionTimestamp;
    if (idleMs >= inactivityMinutes * 60_000) {
      pause();
      setSleepTimer('off');
    }
  }, 30_000);
}

function stopInactivityTimer() {
  if (inactivityCheckInterval) {
    clearInterval(inactivityCheckInterval);
    inactivityCheckInterval = null;
  }
}

export function recordInteraction() {
  lastInteractionTimestamp = Date.now();
}

export function setInactivityMinutes(minutes: number) {
  inactivityMinutes = Math.max(1, Math.min(120, minutes));
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem('audioplayer:inactivityMinutes', String(inactivityMinutes));
  }
}

export function setSleepTimer(mode: SleepTimerMode) {
  sleepTimerMode = mode;
  stopSleepTimer();
  stopInactivityTimer();
  if (mode === 'off') return;
  if (mode === 'end_of_chapter') {
    // Handled in timeupdate via effect
    return;
  }
  if (mode === 'inactivity') {
    startInactivityTimer();
    return;
  }
  startSleepTimer(parseInt(mode));
}

export function seekToChapter(chapter: AudiobookChapter) {
  seek(chapter.start_seconds);
}

/** Seek to a unified chapter, switching tracks if needed. */
export function seekToUnifiedChapter(chapter: UnifiedChapter) {
  seekToFilePosition(chapter.file_id, chapter.start_seconds);
}

/** Seek to a specific position in a specific file, switching tracks if needed.
 *
 * Used by the transcript reader and subtitle view, where each segment carries
 * its file_id and a file-relative start time. seek() alone only operates on
 * the currently loaded audio element, so a click on a segment from a
 * different file would otherwise seek inside the wrong file.
 */
export function seekToFilePosition(fileId: string, seconds: number) {
  if (fileId !== currentFileId) {
    const targetIndex = fileIds.indexOf(fileId);
    if (targetIndex >= 0) {
      advanceTrackFn(targetIndex).then(() => {
        seek(seconds);
      });
      return;
    }
  }
  seek(seconds);
}

export function stop() {
  flushPosition();
  stopSilenceDetection();
  if (audioElement) {
    audioElement.pause();
    audioElement.src = '';
  }
  currentBook = null;
  currentFileId = null;
  fileIds = [];
  fileDurations = [];
  currentTrackIndex = 0;
  playing = false;
  currentTime = 0;
  duration = 0;
  chapters = [];
  unifiedChapters = [];
  stopSaveInterval();
  stopSleepTimer();
  stopInactivityTimer();
  sleepTimerMode = 'off';
  error = null;
  loading = false;
}

function flushPosition() {
  if (currentBook && currentFileId && currentTime > 0) {
    const total = totalBookDuration();
    const progress = total > 0 ? (currentGlobalTime() / total) * 100 : 0;
    saveAudioPosition(currentBook.id, {
      file_id: currentFileId,
      position_seconds: Math.floor(currentTime),
      progress_percent: Math.round(progress)
    }).catch(() => {});
    lastSavedTime = currentTime;
    bumpWatchStateVersion();
  }
}

// Handle end-of-chapter sleep timer
if (typeof window !== 'undefined') {
  // Check at each time update whether we've passed the end of the current chapter
  // This is checked within the timeupdate handler effectively by polling
  setInterval(() => {
    if (sleepTimerMode !== 'end_of_chapter' || !playing) return;
    const chapter = getCurrentChapter();
    if (chapter && currentTime >= chapter.end_seconds - 0.5) {
      pause();
      setSleepTimer('off');
    }
  }, 500);
}

export function getAudioPlayerState() {
  return {
    get currentBook() {
      return currentBook;
    },
    get currentFileId() {
      return currentFileId;
    },
    get playing() {
      return playing;
    },
    get currentTime() {
      return currentTime;
    },
    get duration() {
      return duration;
    },
    get totalDuration() {
      return totalBookDuration();
    },
    get globalCurrentTime() {
      return currentGlobalTime();
    },
    get fileIds() {
      return [...fileIds];
    },
    get fileDurations() {
      return [...fileDurations];
    },
    get fileOffsets() {
      return computeFileOffsets();
    },
    globalTimeForFile(fileId: string, fileSeconds: number): number {
      return globalTimeForFile(fileId, fileSeconds);
    },
    get playbackRate() {
      return playbackRate;
    },
    get volume() {
      return volume;
    },
    get chapters() {
      return chapters;
    },
    get unifiedChapters() {
      return unifiedChapters;
    },
    get sleepTimerMode() {
      return sleepTimerMode;
    },
    get sleepTimerRemaining() {
      return sleepTimerRemaining;
    },
    get loading() {
      return loading;
    },
    get error() {
      return error;
    },
    get currentChapter(): AudiobookChapter | null {
      return getCurrentChapter();
    },
    get isActive(): boolean {
      return currentBook !== null;
    },
    get trackCount(): number {
      return fileIds.length;
    },
    get currentTrackIndex(): number {
      return currentTrackIndex;
    },
    get skipSilence(): boolean {
      return skipSilence;
    },
    get inactivityMinutes(): number {
      return inactivityMinutes;
    },
    get normalizeVolume(): boolean {
      return normalizeVolume;
    },
    get nightMode(): boolean {
      return nightMode;
    }
  };
}

export function getFileIds(): string[] {
  return [...fileIds];
}

export function formatTime(seconds: number): string {
  if (!isFinite(seconds) || seconds < 0) return '0:00';
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);
  if (h > 0) {
    return `${h}:${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
  }
  return `${m}:${s.toString().padStart(2, '0')}`;
}
