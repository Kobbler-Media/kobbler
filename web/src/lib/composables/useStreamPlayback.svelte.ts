import type { StreamOption, StreamSearchResponse, StreamResolveParams } from '$lib/api/client';
import { streamSearch, streamResolve, streamResolveStatus, isStreamCaching } from '$lib/api/client';
import type { PlaybackSessionResponse } from '$lib/types';
import { detectCapabilities } from '$lib/utils/media';
import type { usePlayback } from './usePlayback.svelte';

/** How many options auto-play will try before giving up and showing progress.
 *  Keep at or below kobload's WARM_TOP_N (warm_cache.rs) — options past that
 *  point were never probed, so falling back onto them just trades one cold
 *  wait for another. */
const MAX_AUTO_FALLBACK = 5;

export interface UseStreamPlaybackOpts {
  mediaType: 'movie' | 'episode' | 'tv';
  getMediaId: () => string;
  getSearchParams: () => Record<string, unknown>;
  getResumePosition: () => number | undefined;
  getEstimatedDuration: () => number | undefined;
  /** Playback composable — optional. When absent (discover page), player won't open. */
  playback?: ReturnType<typeof usePlayback>;
  onMessage: (msg: string) => void;
  /** For discover page: auto_create data to include in resolve requests. */
  getAutoCreate?: () => StreamResolveParams['auto_create'];
  /** For discover page: called on successful resolve instead of playback.playSession. */
  onResolved?: (session: PlaybackSessionResponse) => void;
}

export function useStreamPlayback(opts: UseStreamPlaybackOpts) {
  let streamOptions = $state<StreamOption[]>([]);
  let streamErrors = $state<string[]>([]);
  let preWarmed = $state(false);
  let streamLoading = $state(false);
  let resolving = $state(false);
  let resolvingAttempt = $state(0);
  let showStreamModal = $state(false);
  let cachingProgress = $state<number | null>(null);
  let cachingProviderId = $state<string | null>(null);
  let cachingTorrentId = $state<string | null>(null);
  let cachingOption = $state<StreamOption | null>(null);
  let cachingError = $state<string | null>(null);
  let cachingPollTimer: ReturnType<typeof setInterval> | null = null;
  // Track whether autoPlay opened the player so we can close it on failure
  let autoPlayPending = false;
  // Abort flag — set when player is closed during resolve to stop the async loop
  let aborted = false;

  async function preWarmStream() {
    try {
      const result = await streamSearch(opts.getSearchParams());
      streamOptions = applyPreferredOrder(result);
      streamErrors = result.errors;
      preWarmed = true;
    } catch {
      // Silent failure — pre-warming is best-effort
    }
  }

  /** Reorder options to put the preferred (last-used) stream first. */
  function applyPreferredOrder(result: StreamSearchResponse): StreamOption[] {
    const opts = [...result.options];
    if (!result.preferred_info_hash) return opts;
    const idx = opts.findIndex(
      (o) =>
        o.info_hash === result.preferred_info_hash &&
        (result.preferred_file_id == null || o.files[0]?.id === result.preferred_file_id)
    );
    if (idx > 0) {
      const [preferred] = opts.splice(idx, 1);
      opts.unshift(preferred);
    }
    return opts;
  }

  async function ensureSearchResults(): Promise<boolean> {
    if (preWarmed) return streamOptions.length > 0;
    streamLoading = true;
    try {
      const result: StreamSearchResponse = await streamSearch(opts.getSearchParams());
      streamOptions = applyPreferredOrder(result);
      streamErrors = result.errors;
      preWarmed = true;
      return streamOptions.length > 0;
    } catch (e) {
      streamErrors = [e instanceof Error ? e.message : 'Stream search failed'];
      return false;
    } finally {
      streamLoading = false;
    }
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
      // Discover page: no media_id (entity doesn't exist yet), media_type comes from search params
      delete params.media_id;
    }
    return params;
  }

  /** Mark an option as verified-cached and promote it ahead of unverified ones.
   *
   *  Cache warming runs in the background AFTER stream search has already
   *  returned, so the ordering the user sees was computed before we knew what
   *  was actually cached. Without promoting, auto-play still walks options that
   *  warming has since proven blocked -- which is exactly the stall this is
   *  meant to avoid. Verified-cached options move to the front, preserving
   *  relative order within each group so quality ranking still applies. */
  function markCached(info_hash: string, file_id: number | null, provider_id: string) {
    const updated = streamOptions.map((opt) =>
      opt.info_hash === info_hash &&
      opt.debrid_provider_id === provider_id &&
      (opt.files[0]?.id ?? null) === (file_id ?? null)
        ? { ...opt, cached: true }
        : opt
    );
    // Stable partition: cached first, original order preserved within groups.
    streamOptions = [...updated.filter((o) => o.cached), ...updated.filter((o) => !o.cached)];
  }

  /** The next option to try after `option` fails.
   *
   *  Identity is (info_hash, file_id), not info_hash alone: a season pack
   *  appears once per episode file, so matching on hash alone can land on a
   *  sibling entry and hand back an option we already tried. */
  function nextOptionAfter(option: StreamOption): StreamOption | undefined {
    const idx = streamOptions.findIndex(
      (o) => o.info_hash === option.info_hash && o.files[0]?.id === option.files[0]?.id
    );
    return idx >= 0 ? streamOptions[idx + 1] : undefined;
  }

  async function tryPlayWithFallback(option: StreamOption, attempt = 0) {
    if (aborted) return;
    resolving = true;
    resolvingAttempt = attempt;
    try {
      const result = await streamResolve(buildResolveParams(option));
      if (aborted) return;

      if (isStreamCaching(result)) {
        const nextOption = nextOptionAfter(option);

        if (nextOption && attempt < MAX_AUTO_FALLBACK) {
          await tryPlayWithFallback(nextOption, attempt + 1);
          return;
        }

        // All options exhausted — show caching progress
        resolving = false;
        showStreamModal = false;
        cachingProgress = 0;
        cachingProviderId = result.debrid_provider_id;
        cachingTorrentId = result.torrent_id;
        cachingOption = option;
        cachingError = null;
        startCachingPoll();
        return;
      }

      // Success
      resolving = false;
      autoPlayPending = false;
      showStreamModal = false;
      if (opts.onResolved) {
        opts.onResolved(result);
      } else {
        opts.playback?.playSession(result);
      }
    } catch (e) {
      if (aborted) return;
      const nextOption = nextOptionAfter(option);

      if (nextOption && attempt < MAX_AUTO_FALLBACK) {
        await tryPlayWithFallback(nextOption, attempt + 1);
        return;
      }

      resolving = false;
      if (autoPlayPending) {
        autoPlayPending = false;
        opts.playback?.closePending();
      }
      opts.onMessage(e instanceof Error ? e.message : 'Failed to resolve stream');
    }
  }

  async function autoPlay() {
    aborted = false;
    // Open player immediately in pending state (only if we have a playback composable)
    if (opts.playback) {
      opts.playback.openPending();
      autoPlayPending = true;
    }

    const hasResults = await ensureSearchResults();
    if (aborted) return;
    if (!hasResults) {
      if (autoPlayPending) {
        autoPlayPending = false;
        opts.playback?.closePending();
      }
      opts.onMessage(streamErrors.length > 0 ? streamErrors[0] : 'No streams found');
      return;
    }

    await tryPlayWithFallback(streamOptions[0]);
  }

  function openModal() {
    showStreamModal = true;
    if (!preWarmed) {
      streamLoading = true;
      streamSearch(opts.getSearchParams())
        .then((result) => {
          streamOptions = applyPreferredOrder(result);
          streamErrors = result.errors;
          preWarmed = true;
        })
        .catch((e) => {
          streamErrors = [e instanceof Error ? e.message : 'Stream search failed'];
        })
        .finally(() => {
          streamLoading = false;
        });
    }
  }

  function closeModal() {
    showStreamModal = false;
  }

  async function playOption(option: StreamOption) {
    aborted = false;
    // When picking manually from the modal, open player in pending state too
    if (opts.playback) {
      opts.playback.openPending();
      autoPlayPending = true;
    }
    showStreamModal = false;
    await tryPlayWithFallback(option);
  }

  /** For discover page: search for a specific episode and open the modal. */
  function searchEpisode(searchParams: Record<string, unknown>) {
    showStreamModal = true;
    preWarmed = false;
    streamLoading = true;
    streamOptions = [];
    streamErrors = [];
    streamSearch(searchParams)
      .then((result) => {
        streamOptions = applyPreferredOrder(result);
        streamErrors = result.errors;
        preWarmed = true;
      })
      .catch((e) => {
        streamErrors = [e instanceof Error ? e.message : 'Stream search failed'];
      })
      .finally(() => {
        streamLoading = false;
      });
  }

  function startCachingPoll() {
    stopCachingPoll();
    cachingPollTimer = setInterval(async () => {
      if (!cachingProviderId || !cachingTorrentId) return;
      try {
        const status = await streamResolveStatus(cachingProviderId, cachingTorrentId);
        if (status.status === 'ready') {
          stopCachingPoll();
          cachingProgress = 1;
          if (cachingOption) {
            const session = await streamResolve(buildResolveParams(cachingOption));
            const wasAutoPlayPending = autoPlayPending;
            closeCachingState();
            if (!isStreamCaching(session)) {
              autoPlayPending = false;
              if (opts.onResolved) {
                opts.onResolved(session);
              } else if (opts.playback) {
                if (wasAutoPlayPending) {
                  // Player is already open in pending state — deliver session
                  opts.playback.playSession(session);
                } else {
                  opts.playback.playSession(session);
                }
              }
            }
          }
        } else if (status.status === 'downloading') {
          cachingProgress = status.progress ?? 0;
        } else if (status.status === 'error') {
          stopCachingPoll();
          cachingError = status.message ?? 'Download failed on debrid provider';
        }
      } catch (e) {
        stopCachingPoll();
        cachingError = e instanceof Error ? e.message : 'Failed to check status';
      }
    }, 3000);
  }

  function stopCachingPoll() {
    if (cachingPollTimer) {
      clearInterval(cachingPollTimer);
      cachingPollTimer = null;
    }
  }

  /** Clear caching state without closing the player's pending state. */
  function closeCachingState() {
    stopCachingPoll();
    cachingProgress = null;
    cachingProviderId = null;
    cachingTorrentId = null;
    cachingOption = null;
    cachingError = null;
  }

  function closeCachingOverlay() {
    closeCachingState();
    if (autoPlayPending) {
      autoPlayPending = false;
      aborted = true;
      opts.playback?.closePending();
    }
  }

  function retryCaching() {
    cachingError = null;
    closeCachingOverlay();
    showStreamModal = true;
  }

  function handleSwitchStream() {
    aborted = true;
    opts.playback?.close();
    showStreamModal = true;
  }

  /** Called when the player is closed externally (e.g. user clicks close). */
  function abort() {
    aborted = true;
    autoPlayPending = false;
    resolving = false;
    closeCachingState();
  }

  function reset() {
    abort();
    preWarmed = false;
    streamOptions = [];
    streamErrors = [];
    streamLoading = false;
    resolvingAttempt = 0;
    showStreamModal = false;
  }

  function teardown() {
    stopCachingPoll();
  }

  return {
    get streamOptions() {
      return streamOptions;
    },
    set streamOptions(val: StreamOption[]) {
      streamOptions = val;
    },
    get streamErrors() {
      return streamErrors;
    },
    get preWarmed() {
      return preWarmed;
    },
    get streamLoading() {
      return streamLoading;
    },
    get resolving() {
      return resolving;
    },
    get resolvingAttempt() {
      return resolvingAttempt;
    },
    get showStreamModal() {
      return showStreamModal;
    },
    get cachingProgress() {
      return cachingProgress;
    },
    get cachingError() {
      return cachingError;
    },
    get cachingOption() {
      return cachingOption;
    },
    /** Whether caching is active (for consumers that show caching outside the player). */
    get isCaching() {
      return cachingProgress !== null;
    },
    get pendingStatus(): string | null {
      if (streamLoading) return 'Searching for streams...';
      if (resolving) {
        if (resolvingAttempt > 0)
          return `Trying stream ${resolvingAttempt + 1}/${MAX_AUTO_FALLBACK}...`;
        return 'Resolving stream...';
      }
      if (cachingProgress !== null && cachingError === null) {
        const pct = Math.round((cachingProgress ?? 0) * 100);
        return `Caching stream... ${pct}%`;
      }
      if (cachingError) {
        return null; // Let the player show the error state
      }
      return null;
    },
    markCached,
    preWarmStream,
    autoPlay,
    openModal,
    closeModal,
    playOption,
    searchEpisode,
    closeCachingOverlay,
    retryCaching,
    handleSwitchStream,
    abort,
    reset,
    teardown
  };
}
