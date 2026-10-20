import type { StreamOption, StreamResolveParams } from '$lib/api/client';
import { deleteStreamSession, streamResolve, streamSearch, isStreamCaching } from '$lib/api/client';
import type { PlaybackSessionResponse } from '$lib/types';
import { detectCapabilities } from '$lib/utils/media';
import { on, off } from '$lib/services/eventstream.svelte';
import {
  MAX_ATTEMPTS,
  newWalkState,
  optionKey,
  rankOptions,
  toFailure,
  untriedOptions,
  walkStreams
} from '$lib/streaming/walk';
import type { WalkState } from '$lib/streaming/walk';
import { waitForCache } from '$lib/streaming/cacheWait';
import {
  PLAYER_FAILURE_REASON,
  explainFailure,
  failureDetails,
  summarizeFailures,
  summarizeNoStreams
} from '$lib/streaming/failure';
import type { usePlayback } from './usePlayback.svelte';

export interface StreamHandoff {
  session: PlaybackSessionResponse;
  option: StreamOption;
  tried: string[];
}

export interface StreamPlayerFailure {
  sessionId: string;
  started: boolean;
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
  playback?: ReturnType<typeof usePlayback>;
  onMessage: (msg: string) => void;
  getAutoCreate?: () => StreamResolveParams['auto_create'];
  onResolved?: (handoff: StreamHandoff) => void;
  want?: () => Promise<void>;
  canWant?: () => boolean;
}

interface Current {
  option: StreamOption;
  sessionId: string;
  retried: boolean;
}

export function useStreamPlayback(opts: UseStreamPlaybackOpts) {
  let streamOptions = $state<StreamOption[]>([]);
  let streamErrors = $state<string[]>([]);
  let searched = $state(false);
  let streamLoading = $state(false);
  let resolving = $state(false);
  let attempt = $state(0);
  let notice = $state<string | null>(null);
  let showStreamModal = $state(false);
  let cachingProgress = $state<number | null>(null);
  let failure = $state<StreamFailurePanel | null>(null);
  let wanting = $state(false);

  let run: WalkState | null = null;
  let current: Current | null = null;
  let controller = new AbortController();

  $effect(() => {
    const cb = on(
      'stream_warm_ready',
      (data: { info_hash: string; file_id?: number | null; debrid_provider_id: string }) =>
        markCached(data.info_hash, data.file_id ?? null, data.debrid_provider_id)
    );
    return () => {
      off('stream_warm_ready', cb);
      abort();
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

  async function ensureSearched(): Promise<void> {
    if (!searched) await search();
  }

  async function preWarmStream() {
    if (!searched && !streamLoading) await search();
  }

  function buildResolveParams(option: StreamOption): StreamResolveParams {
    const resumePos = opts.getResumePosition();
    const params: StreamResolveParams = {
      debrid_provider_id: option.debrid_provider_id,
      info_hash: option.info_hash,
      file_id: option.files[0]?.id,
      media_type: opts.mediaType,
      media_id: opts.getMediaId(),
      release_title: option.release_title,
      debrid_provider: option.debrid_provider,
      quality: option.quality,
      start_time: resumePos && resumePos > 5 ? resumePos : undefined,
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

  function beginAttempt(): AbortSignal {
    controller.abort();
    controller = new AbortController();
    failure = null;
    showStreamModal = false;
    opts.playback?.openPending();
    return controller.signal;
  }

  function walkDeps(signal: AbortSignal) {
    return {
      resolve: (option: StreamOption) => streamResolve(buildResolveParams(option)),
      waitForCache: (option: StreamOption, response: Parameters<typeof waitForCache>[0]) =>
        waitForCache(
          response,
          () => streamResolve(buildResolveParams(option)),
          (p) => (cachingProgress = p),
          signal
        ).finally(() => (cachingProgress = null)),
      isAborted: () => signal.aborted,
      onAttempt: (n: number) => (attempt = n)
    };
  }

  async function walk(signal: AbortSignal, options: StreamOption[]): Promise<boolean> {
    if (!run) return false;
    resolving = true;
    try {
      const result = await walkStreams(options, run, walkDeps(signal));
      if (result.kind === 'aborted') return true;
      if (result.kind === 'exhausted') return false;
      deliver(result.option, result.session, false);
      return true;
    } finally {
      if (!signal.aborted) resolving = false;
    }
  }

  async function continueWalk(signal: AbortSignal) {
    await ensureSearched();
    if (signal.aborted) return;
    if (streamOptions.length === 0) return showNoStreams();
    if (!(await walk(signal, streamOptions))) showExhausted();
  }

  function deliver(option: StreamOption, session: PlaybackSessionResponse, retried: boolean) {
    notice = null;
    current = { option, sessionId: session.session_id, retried };
    if (opts.onResolved) {
      opts.onResolved({ session, option: $state.snapshot(option), tried: [...(run?.tried ?? [])] });
    } else {
      opts.playback?.playSession(session);
    }
  }

  function wantable() {
    return !!opts.want && (opts.canWant?.() ?? true);
  }

  function showPanel(panel: Omit<StreamFailurePanel, 'canWant' | 'canChoose'>) {
    resolving = false;
    notice = null;
    current = null;
    opts.playback?.close();
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

  function showExhausted() {
    showPanel({
      title: "Couldn't play this",
      summary: summarizeFailures(run?.failures ?? [], run?.tried.size ?? 0),
      details: failureDetails(run?.failures ?? []),
      canTryAnother: run ? untriedOptions(streamOptions, run).length > 0 : false
    });
  }

  function showStopped(reason: string | undefined) {
    const failures = run?.failures ?? [];
    const last = failures[failures.length - 1];
    showPanel({
      title: 'Playback stopped',
      summary: `The stream stopped and couldn't be restarted.${last ? ` ${explainFailure(last)}` : ''}`,
      details: [...failureDetails(failures), ...(reason ? [`Player: ${reason}`] : [])],
      canTryAnother: run ? untriedOptions(streamOptions, run).length > 0 : false
    });
  }

  async function autoPlay() {
    const signal = beginAttempt();
    run = newWalkState();
    attempt = 0;
    await continueWalk(signal);
  }

  async function playOption(option: StreamOption) {
    const signal = beginAttempt();
    const key = optionKey(option);
    run = newWalkState([...(run?.tried ?? [])].filter((k) => k !== key));
    if (await walk(signal, [option])) return;
    if (!signal.aborted) await continueWalk(signal);
  }

  async function tryAnother() {
    const signal = beginAttempt();
    run ??= newWalkState();
    run.budget = MAX_ATTEMPTS;
    await continueWalk(signal);
  }

  function adoptSession(handoff: StreamHandoff) {
    run = newWalkState(handoff.tried);
    current = { option: handoff.option, sessionId: handoff.session.session_id, retried: false };
    opts.playback?.playSession(handoff.session);
  }

  async function retryRunning(signal: AbortSignal, option: StreamOption) {
    resolving = true;
    try {
      const response = await streamResolve(buildResolveParams(option));
      if (signal.aborted) return;
      if (isStreamCaching(response)) return showStopped(undefined);
      deliver(option, response, true);
    } catch (e) {
      if (signal.aborted) return;
      run?.failures.push(toFailure(e, option));
      showStopped(undefined);
    } finally {
      if (!signal.aborted) resolving = false;
    }
  }

  function handlePlayerFailure(f: StreamPlayerFailure): boolean {
    if (!current || current.sessionId !== f.sessionId) return false;
    const { option, retried } = current;
    run ??= newWalkState([optionKey(option)]);
    current = null;
    // The outcome is keyed by session, so it has to land before the session is gone.
    f.reported.finally(() => deleteStreamSession(f.sessionId).catch(() => {}));
    if (!f.started) {
      run.failures.push({
        message: "The player couldn't play that stream.",
        reason: PLAYER_FAILURE_REASON,
        detail: f.reason,
        release: option.release_title
      });
    }
    if (retried) {
      showStopped(f.reason);
      return true;
    }
    const signal = beginAttempt();
    if (f.started) {
      notice = 'Playback stopped — reconnecting…';
      void retryRunning(signal, option);
    } else {
      notice = "That stream wouldn't play — trying another…";
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
    opts.playback?.close();
  }

  function pickFromPlayer() {
    abort();
    opts.playback?.close();
    openModal();
  }

  function abort() {
    controller.abort();
    resolving = false;
    notice = null;
    cachingProgress = null;
    current = null;
  }

  function reset() {
    abort();
    run = null;
    searched = false;
    streamOptions = [];
    streamErrors = [];
    streamLoading = false;
    attempt = 0;
    showStreamModal = false;
    failure = null;
  }

  function pendingStatus(): string | undefined {
    if (streamLoading) return 'Searching for streams…';
    if (cachingProgress !== null) return `Caching stream… ${Math.round(cachingProgress * 100)}%`;
    if (notice) return notice;
    if (resolving) return attempt > 1 ? `Trying stream ${attempt}…` : 'Finding a stream…';
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
      return streamLoading || resolving || cachingProgress !== null;
    },
    get showStreamModal() {
      return showStreamModal;
    },
    get cachingProgress() {
      return cachingProgress;
    },
    get failure() {
      return failure;
    },
    get wanting() {
      return wanting;
    },
    get playerProps() {
      return {
        pendingStatus: pendingStatus(),
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
    adoptSession,
    handlePlayerFailure,
    openModal,
    closeModal,
    cancel,
    abort,
    reset
  };
}

export type StreamPlayback = ReturnType<typeof useStreamPlayback>;
