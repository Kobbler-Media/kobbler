import type {
  StreamOption,
  StreamPrewarmParams,
  StreamResolveCachingResponse,
  StreamResolveParams
} from '#lib/api/client.js';
import {
  deleteStreamSession,
  isStreamCaching,
  reportStreamOutcome,
  streamLocal,
  streamResolve,
  streamSearch
} from '#lib/api/client.js';
import type { PlaybackSessionResponse } from '#lib/types.js';
import { detectCapabilities } from '#lib/utils/media.js';
import { on, off } from '#lib/services/eventstream.svelte.js';
import {
  newWalkState,
  optionKey,
  rankOptions,
  refillBudget,
  toFailure,
  untriedOptions,
  walkStreams
} from '#lib/streaming/walk.js';
import type { WalkResult, WalkState } from '#lib/streaming/walk.js';
import { waitForCache } from '#lib/streaming/cacheWait.js';
import { createPrewarm } from '#lib/streaming/prewarm.js';
import type { CachingState } from '#lib/streaming/cacheWait.js';
import {
  PLAYER_FAILURE_REASON,
  explainFailure,
  failureDetails,
  summarizeFailures,
  summarizeNoStreams
} from '#lib/streaming/failure.js';
import type { usePlayback } from './usePlayback.svelte';

export interface StreamPlayerFailure {
  sessionId: string;
  started: boolean;
  position: number;
  reason: string;
  reported: Promise<unknown>;
}

export interface StreamFailurePanel {
  title: string;
  summary: string;
  details: string[];
  canTryAnother: boolean;
  canChoose: boolean;
  canWant: boolean;
}

export interface UseStreamPlaybackOpts {
  mediaType: 'movie' | 'episode' | 'tv';
  getMediaId: () => string;
  getSearchParams: () => Record<string, unknown>;
  getResumePosition: () => number | undefined;
  getEstimatedDuration: () => number | undefined;
  playback: Pick<ReturnType<typeof usePlayback>, 'openPending' | 'playSession' | 'close'>;
  onMessage: (msg: string) => void;
  getAutoCreate?: () => StreamResolveParams['auto_create'];
  want?: () => Promise<void>;
  canWant?: () => boolean;
}

interface Current {
  option: StreamOption;
  sessionId: string;
  retried: boolean;
}

const FINDING_NOTE = 'Finding a stream…';
const NEXT_STREAM_NOTE = "That stream wouldn't play — trying another…";
const RECONNECT_NOTE = 'Lost the stream — reconnecting…';
const SWITCHED_NOTE = 'Switched to another source';
const QUIET = '';
// Most reconnects land inside a second; announcing those turns a hiccup into an incident.
const RECONNECT_NOTICE_DELAY_MS = 1_800;
// Another release is another encode with its own keyframes and cuts, so it
// picks up a little early rather than skip what the viewer had not seen.
const SWITCH_REWIND_SEC = 5;

export function useStreamPlayback(opts: UseStreamPlaybackOpts) {
  let streamOptions = $state<StreamOption[]>([]);
  let streamErrors = $state<string[]>([]);
  let searched = $state(false);
  let streamLoading = $state(false);
  let resolving = $state(false);
  let notice = $state<string | null>(null);
  let showStreamModal = $state(false);
  let caching = $state<CachingState | null>(null);
  let failure = $state<StreamFailurePanel | null>(null);
  let wanting = $state(false);
  let resumeOverride = $state<number | undefined>(undefined);
  let reconnecting = $state(false);
  let reconnectNoted = $state(false);
  let onDisk = $state<StreamOption | null | undefined>(undefined);

  let run: WalkState | null = null;
  let current: Current | null = null;
  let controller = new AbortController();
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  let switching = false;
  const prewarm = createPrewarm();

  $effect(() => {
    const cb = on(
      'stream_warm_ready',
      (data: { info_hash: string; file_id?: number | null; debrid_provider_id: string }) =>
        markCached(data.info_hash, data.file_id ?? null, data.debrid_provider_id)
    );
    const unload = () => prewarm.release(true);
    window.addEventListener('beforeunload', unload);
    return () => {
      off('stream_warm_ready', cb);
      window.removeEventListener('beforeunload', unload);
      abort();
      prewarm.release();
    };
  });

  function markCached(info_hash: string, file_id: number | null, provider_id: string) {
    streamOptions = streamOptions.map((o) =>
      o.info_hash === info_hash &&
      o.debrid_provider_id === provider_id &&
      (o.files[0]?.id ?? null) === file_id
        ? { ...o, cached: true }
        : o
    );
  }

  async function search(): Promise<void> {
    streamLoading = true;
    try {
      const result = await streamSearch(opts.getSearchParams());
      streamOptions = rankOptions(result);
      streamErrors = result.errors;
      searched = true;
    } catch (e) {
      streamErrors = [e instanceof Error ? e.message : 'Stream search failed'];
    } finally {
      streamLoading = false;
    }
  }

  function startTime(): number | undefined {
    const resumePos = resumeOverride ?? opts.getResumePosition();
    return resumePos && resumePos > 5 ? resumePos : undefined;
  }

  function buildPrewarmParams(): StreamPrewarmParams {
    const { season, episode } = opts.getSearchParams() as { season?: number; episode?: number };
    const autoCreate = opts.getAutoCreate?.();
    return {
      media_type: opts.mediaType,
      ...(autoCreate ? { auto_create: autoCreate } : { media_id: opts.getMediaId() }),
      ...(season != null && episode != null ? { season, episode } : {}),
      capabilities: detectCapabilities(),
      start_time: startTime()
    };
  }

  async function preWarmStream() {
    prewarm.start(buildPrewarmParams());
    if (searched || streamLoading) return;
    if (!(await findOnDisk())) await search();
  }

  // A title can only have a release on disk once it is in the library.
  async function findOnDisk(): Promise<StreamOption | null> {
    if (onDisk !== undefined) return onDisk;
    if (opts.mediaType === 'tv' || opts.getAutoCreate?.()) return (onDisk = null);
    const mediaId = opts.getMediaId();
    if (!mediaId) return (onDisk = null);
    onDisk = await streamLocal(opts.mediaType, mediaId).catch(() => null);
    return onDisk;
  }

  function buildResolveParams(option: StreamOption): StreamResolveParams {
    const params: StreamResolveParams = {
      debrid_provider_id: option.debrid_provider_id,
      info_hash: option.info_hash,
      file_id: option.files[0]?.id,
      media_type: opts.mediaType,
      media_id: opts.getMediaId(),
      release_title: option.release_title,
      debrid_provider: option.debrid_provider,
      quality: option.quality,
      start_time: startTime(),
      capabilities: detectCapabilities(),
      estimated_duration: opts.getEstimatedDuration()
    };
    const autoCreate = opts.getAutoCreate?.();
    if (autoCreate) {
      params.auto_create = autoCreate;
      // The discover entity doesn't exist yet; the server creates it from auto_create.
      delete params.media_id;
    }
    return params;
  }

  const resolveOption = (option: StreamOption) => streamResolve(buildResolveParams(option));

  function beginAttempt(): AbortSignal {
    controller.abort();
    controller = new AbortController();
    endReconnect();
    failure = null;
    showStreamModal = false;
    opts.playback.openPending();
    return controller.signal;
  }

  async function walk(signal: AbortSignal, options: StreamOption[]): Promise<WalkResult> {
    run ??= newWalkState();
    resolving = true;
    try {
      return await walkStreams(options, run, {
        resolve: resolveOption,
        isAborted: () => signal.aborted,
        onAttemptFailed: () => (notice = NEXT_STREAM_NOTE)
      });
    } finally {
      if (!signal.aborted) resolving = false;
    }
  }

  async function waitCaching(
    signal: AbortSignal,
    option: StreamOption,
    response: StreamResolveCachingResponse
  ) {
    notice = null;
    try {
      const session = await waitForCache(
        response,
        () => resolveOption(option),
        (state) => (caching = state),
        signal,
        option.debrid_provider
      );
      if (!signal.aborted) deliver(option, session, false);
    } catch (e) {
      if (signal.aborted) return;
      run?.failures.push(toFailure(e, option));
      showExhausted(false);
    } finally {
      if (!signal.aborted) caching = null;
    }
  }

  async function settle(signal: AbortSignal, result: WalkResult) {
    if (result.kind === 'aborted') return;
    if (result.kind === 'ready') return deliver(result.option, result.session, false);
    if (result.kind === 'caching') return waitCaching(signal, result.option, result.response);
    if (switching) return showStopped(undefined);
    showExhausted(result.stopped);
  }

  async function continueWalk(signal: AbortSignal) {
    if (!searched) await search();
    if (signal.aborted) return;
    if (streamOptions.length === 0) return showNoStreams();
    await settle(signal, await walk(signal, streamOptions));
  }

  function deliver(option: StreamOption, session: PlaybackSessionResponse, retried: boolean) {
    notice = null;
    endReconnect();
    current = { option, sessionId: session.session_id, retried };
    opts.playback.playSession(session);
    if (switching) opts.onMessage(SWITCHED_NOTE);
    switching = false;
  }

  function startReconnect(noted: boolean) {
    reconnecting = true;
    reconnectNoted = noted;
    if (noted) return;
    reconnectTimer = setTimeout(() => (reconnectNoted = true), RECONNECT_NOTICE_DELAY_MS);
  }

  function endReconnect() {
    if (reconnectTimer) clearTimeout(reconnectTimer);
    reconnectTimer = null;
    reconnecting = false;
    reconnectNoted = false;
  }

  function wantable() {
    return !!opts.want && (opts.canWant?.() ?? true);
  }

  function canTryAnother() {
    return run ? untriedOptions(streamOptions, run).length > 0 : false;
  }

  function showPanel(panel: Omit<StreamFailurePanel, 'canWant' | 'canChoose'>) {
    resolving = false;
    notice = null;
    endReconnect();
    switching = false;
    current = null;
    opts.playback.close();
    failure = { ...panel, canChoose: streamOptions.length > 0, canWant: wantable() };
  }

  function showNoStreams() {
    showPanel({
      title: 'No streams found',
      summary: summarizeNoStreams(streamErrors),
      details: streamErrors.slice(1),
      canTryAnother: false
    });
  }

  function showExhausted(stopped: boolean) {
    const failures = run?.failures ?? [];
    // A rate limit or rejected key ended the walk, whatever else came before it.
    const cause = stopped ? failures.slice(-1) : failures;
    showPanel({
      title: "Couldn't play this",
      summary: summarizeFailures(cause, run?.tried.size ?? 0),
      details: failureDetails(failures),
      canTryAnother: !stopped && canTryAnother()
    });
  }

  function showStopped(reason: string | undefined) {
    const failures = run?.failures ?? [];
    const last = failures[failures.length - 1];
    showPanel({
      title: 'Playback stopped',
      summary: `The stream stopped and couldn't be restarted.${last ? ` ${explainFailure(last)}` : ''}`,
      details: [...failureDetails(failures), ...(reason ? [`Player: ${reason}`] : [])],
      canTryAnother: canTryAnother()
    });
  }

  async function claimPrewarm(signal: AbortSignal) {
    resolving = true;
    try {
      return await prewarm.claim(buildPrewarmParams());
    } finally {
      if (!signal.aborted) resolving = false;
    }
  }

  async function autoPlay() {
    const signal = beginAttempt();
    resumeOverride = undefined;
    run = newWalkState();
    const claimed = await claimPrewarm(signal);
    if (signal.aborted) return;
    if (claimed) {
      run.tried.add(optionKey(claimed.option));
      return deliver(claimed.option, claimed.session, false);
    }
    const local = await findOnDisk();
    if (signal.aborted) return;
    if (local) {
      const result = await walk(signal, [local]);
      if (result.kind === 'ready') return settle(signal, result);
      if (signal.aborted) return;
    }
    return continueWalk(signal);
  }

  async function playOption(option: StreamOption) {
    const signal = beginAttempt();
    resumeOverride = undefined;
    prewarm.release();
    const key = optionKey(option);
    run = newWalkState([...(run?.tried ?? [])].filter((k) => k !== key));
    const result = await walk(signal, [option]);
    if (result.kind === 'exhausted' && !result.stopped && !signal.aborted) {
      return continueWalk(signal);
    }
    await settle(signal, result);
  }

  async function tryAnother() {
    const signal = beginAttempt();
    run ??= newWalkState();
    refillBudget(run);
    await continueWalk(signal);
  }

  async function retryRunning(signal: AbortSignal, option: StreamOption) {
    resolving = true;
    try {
      const response = await resolveOption(option);
      if (signal.aborted) return;
      if (!isStreamCaching(response)) return deliver(option, response, true);
    } catch (e) {
      if (signal.aborted) return;
      run?.failures.push(toFailure(e, option));
    } finally {
      if (!signal.aborted) resolving = false;
    }
    await switchSource(signal);
  }

  function switchSource(signal: AbortSignal) {
    switching = true;
    resumeOverride = Math.max(0, (resumeOverride ?? 0) - SWITCH_REWIND_SEC);
    return continueWalk(signal);
  }

  function handlePlayerFailure(f: StreamPlayerFailure): boolean {
    if (!current || current.sessionId !== f.sessionId) return false;
    const { option, retried } = current;
    run ??= newWalkState([optionKey(option)]);
    current = null;
    const noted = reconnectNoted;
    // A release that played and then dropped is more likely an expired link
    // than a bad file, so only a second loss is held against it.
    const reported =
      f.started && retried
        ? f.reported.then(() =>
            reportStreamOutcome(f.sessionId, { ok: false, reason: f.reason }).catch(() => {})
          )
        : f.reported;
    // The outcome is keyed by session, so it has to land before the session is gone.
    reported.finally(() => deleteStreamSession(f.sessionId).catch(() => {}));
    if (!f.started || retried) {
      run.failures.push({
        message: "The player couldn't play that stream.",
        reason: PLAYER_FAILURE_REASON,
        detail: f.reason,
        release: option.release_title
      });
      run.attempts++;
    }
    if (f.started) resumeOverride = f.position;
    const signal = beginAttempt();
    if (retried) {
      startReconnect(noted);
      void switchSource(signal);
    } else if (f.started) {
      startReconnect(false);
      void retryRunning(signal, option);
    } else {
      notice = NEXT_STREAM_NOTE;
      void continueWalk(signal);
    }
    return true;
  }

  async function want() {
    if (!opts.want || wanting) return;
    wanting = true;
    try {
      await opts.want();
      failure = null;
    } catch (e) {
      opts.onMessage(e instanceof Error ? e.message : 'Failed to want this');
    } finally {
      wanting = false;
    }
  }

  function openModal() {
    showStreamModal = true;
    if (!searched && !streamLoading) void search();
  }

  function closeModal() {
    showStreamModal = false;
  }

  function chooseRelease() {
    failure = null;
    openModal();
  }

  function dismissFailure() {
    failure = null;
  }

  function cancel() {
    abort();
    opts.playback.close();
  }

  function pickFromPlayer() {
    cancel();
    openModal();
  }

  function abort() {
    controller.abort();
    resolving = false;
    notice = null;
    caching = null;
    current = null;
    switching = false;
    resumeOverride = undefined;
    endReconnect();
  }

  function reset() {
    abort();
    prewarm.release();
    run = null;
    searched = false;
    onDisk = undefined;
    streamOptions = [];
    streamErrors = [];
    streamLoading = false;
    showStreamModal = false;
    failure = null;
  }

  function pendingStatus(): string | undefined {
    if (caching) return undefined;
    if (reconnecting) return reconnectNoted ? RECONNECT_NOTE : QUIET;
    if (notice) return notice;
    if (streamLoading || resolving) return FINDING_NOTE;
    return undefined;
  }

  return {
    get streamOptions() {
      return streamOptions;
    },
    get streamErrors() {
      return streamErrors;
    },
    get streamLoading() {
      return streamLoading;
    },
    get busy() {
      return streamLoading || resolving || caching !== null;
    },
    get showStreamModal() {
      return showStreamModal;
    },
    get caching() {
      return caching;
    },
    get failure() {
      return failure;
    },
    get wanting() {
      return wanting;
    },
    get searched() {
      return searched;
    },
    /** What is known to be playable: the search's options, or the release on disk before any search. */
    get knownOptions(): StreamOption[] | null {
      if (searched) return streamOptions;
      return onDisk ? [onDisk] : null;
    },
    get playerProps() {
      return {
        pendingStatus: pendingStatus(),
        resumePosition: resumeOverride,
        pendingCaching: caching,
        onSwitchStream: pickFromPlayer,
        onPickStream: pickFromPlayer,
        onStreamFailed: handlePlayerFailure
      };
    },
    preWarmStream,
    autoPlay,
    playOption,
    tryAnother,
    chooseRelease,
    want,
    dismissFailure,
    handlePlayerFailure,
    openModal,
    closeModal,
    cancel,
    abort,
    reset
  };
}

export type StreamPlayback = ReturnType<typeof useStreamPlayback>;
