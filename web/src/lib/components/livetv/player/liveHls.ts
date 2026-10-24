import type Hls from 'hls.js';
import { liveSyncRate, liveTargetLatency, tunerFollowLimit } from '$lib/streaming/liveLatency';

type HlsCtor = typeof Hls;

let modulePromise: Promise<HlsCtor> | null = null;

export function preloadHls(): Promise<HlsCtor> {
  if (!modulePromise) {
    modulePromise = import('hls.js').then((m) => m.default);
  }
  return modulePromise;
}

// The tuner emits a segment about every second, and the server publishes the
// playlist the moment the first one lands. An empty or 404 playlist in the
// first second is expected, not an error — retry hard and briefly instead of
// failing the session (Decision 12).
const MANIFEST_POLICY = {
  default: {
    maxTimeToFirstByteMs: 8000,
    maxLoadTimeMs: 12000,
    timeoutRetry: { maxNumRetry: 6, retryDelayMs: 250, maxRetryDelayMs: 1000 },
    errorRetry: { maxNumRetry: 12, retryDelayMs: 250, maxRetryDelayMs: 1000 }
  }
};

const FRAG_POLICY = {
  default: {
    maxTimeToFirstByteMs: 8000,
    maxLoadTimeMs: 20000,
    timeoutRetry: { maxNumRetry: 4, retryDelayMs: 250, maxRetryDelayMs: 1000 },
    errorRetry: { maxNumRetry: 6, retryDelayMs: 250, maxRetryDelayMs: 1000 }
  }
};

const LIVE_SYNC_DURATION_S = 1;

const RECOVERY_WINDOW_MS = 60_000;
const MAX_RECOVERIES = 4;

export interface LiveHlsHandle {
  load(playlistUrl: string): void;
  destroy(): void;
}

export function createLiveHls(
  Ctor: HlsCtor,
  video: HTMLVideoElement,
  hooks: { onFatal: (message: string) => void; onFirstFragment?: () => void }
): LiveHlsHandle {
  let instance: Hls | null = null;
  let recoveries: number[] = [];
  let destroyed = false;

  function tearDown() {
    if (!instance) return;
    instance.destroy();
    instance = null;
    video.playbackRate = 1;
  }

  // A tuner a hair slower than realtime eats a 1s holdback within minutes, and with
  // no cushion every segment lands just in time and playback stalls once per second.
  function steer() {
    const details = instance?.latestLevelDetails;
    if (!instance || video.paused || !details?.live) return;
    const target = liveTargetLatency(
      LIVE_SYNC_DURATION_S,
      details.totalduration,
      details.targetduration
    );
    const rate = liveSyncRate({
      latency: Number.isFinite(instance.latency) ? instance.latency : null,
      targetLatency: target,
      targetDurationS: details.targetduration,
      currentRate: video.playbackRate,
      followLimitS: tunerFollowLimit(target, details.targetduration)
    });
    if (rate !== video.playbackRate) video.playbackRate = rate;
  }
  video.addEventListener('timeupdate', steer);

  function budgetLeft(): boolean {
    const cutoff = Date.now() - RECOVERY_WINDOW_MS;
    recoveries = recoveries.filter((t) => t > cutoff);
    if (recoveries.length >= MAX_RECOVERIES) return false;
    recoveries.push(Date.now());
    return true;
  }

  function load(playlistUrl: string) {
    if (destroyed) return;
    tearDown();

    const hls = new Ctor({
      lowLatencyMode: false,
      // The whole point of the timeshift buffer is scrubbing back through it,
      // so nothing already played may be evicted.
      backBufferLength: Infinity,
      // One GOP of holdback. No client-side pre-buffer guard — on a LAN tuner
      // it buys nothing (Decision 12). liveMaxLatencyDuration is deliberately
      // unset: capping latency makes hls.js drag the playhead back to the live
      // edge, which would undo every scrub-back.
      liveSyncDuration: LIVE_SYNC_DURATION_S,
      maxBufferLength: 30,
      maxMaxBufferLength: 60,
      startPosition: -1,
      manifestLoadingTimeOut: 8000,
      manifestLoadPolicy: MANIFEST_POLICY,
      playlistLoadPolicy: MANIFEST_POLICY,
      fragLoadPolicy: FRAG_POLICY
    });
    instance = hls;

    hls.on(Ctor.Events.MANIFEST_PARSED, () => {
      video.play().catch(() => {});
    });

    if (hooks.onFirstFragment) {
      const seen = () => {
        hls.off(Ctor.Events.FRAG_BUFFERED, seen);
        hooks.onFirstFragment?.();
      };
      hls.on(Ctor.Events.FRAG_BUFFERED, seen);
    }

    hls.on(Ctor.Events.ERROR, (_evt: unknown, data: { fatal: boolean; type: string }) => {
      if (!data.fatal || destroyed || instance !== hls) return;
      if (!budgetLeft()) {
        hooks.onFatal('This channel keeps dropping out. Check the aerial signal and try again.');
        return;
      }
      if (data.type === Ctor.ErrorTypes.MEDIA_ERROR) {
        hls.recoverMediaError();
      } else if (data.type === Ctor.ErrorTypes.NETWORK_ERROR) {
        hls.startLoad();
      } else {
        hooks.onFatal('The stream stopped unexpectedly.');
      }
    });

    hls.loadSource(playlistUrl);
    hls.attachMedia(video);
  }

  return {
    load,
    destroy() {
      destroyed = true;
      video.removeEventListener('timeupdate', steer);
      tearDown();
    }
  };
}
