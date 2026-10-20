import { ApiError, isStreamCaching, streamResolveStatus } from '$lib/api/client';
import type { StreamResolveCachingResponse, StreamResolveResponse } from '$lib/api/client';
import type { PlaybackSessionResponse } from '$lib/types';
import { CACHE_FAILURE_REASON } from './failure';

const POLL_MS = 3000;

export class StreamAbortedError extends Error {
  constructor() {
    super('Stream cancelled');
    this.name = 'StreamAbortedError';
  }
}

export function waitForCache(
  caching: StreamResolveCachingResponse,
  resolveReady: () => Promise<StreamResolveResponse>,
  onProgress: (progress: number) => void,
  signal: AbortSignal
): Promise<PlaybackSessionResponse> {
  return new Promise((resolve, reject) => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const onAbort = () => {
      clearTimeout(timer);
      reject(new StreamAbortedError());
    };
    if (signal.aborted) return onAbort();
    signal.addEventListener('abort', onAbort, { once: true });
    const settle = (fn: () => void) => {
      signal.removeEventListener('abort', onAbort);
      fn();
    };
    const tick = async () => {
      try {
        const status = await streamResolveStatus(caching.debrid_provider_id, caching.torrent_id);
        if (signal.aborted) return;
        if (status.status === 'error') {
          const message = status.message ?? 'The download failed on your debrid service.';
          return settle(() =>
            reject(new ApiError(502, message, CACHE_FAILURE_REASON, status.message))
          );
        }
        if (status.status === 'ready') {
          onProgress(1);
          const session = await resolveReady();
          if (signal.aborted) return;
          if (!isStreamCaching(session)) return settle(() => resolve(session));
        } else {
          onProgress(status.progress ?? 0);
        }
        timer = setTimeout(tick, POLL_MS);
      } catch (e) {
        if (!signal.aborted) settle(() => reject(e));
      }
    };
    onProgress(0);
    timer = setTimeout(tick, POLL_MS);
  });
}
