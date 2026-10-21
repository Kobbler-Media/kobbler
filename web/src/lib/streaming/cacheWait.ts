import { ApiError, isStreamCaching, streamResolveStatus } from '$lib/api/client';
import type { StreamResolveCachingResponse, StreamResolveResponse } from '$lib/api/client';
import type { PlaybackSessionResponse } from '$lib/types';
import { CACHE_FAILURE_REASON } from './failure';

const POLL_MS = 3000;
export const CACHING_QUEUED_AFTER_MS = 20_000;

export interface CachingState {
  progress: number;
  note: string;
}

export class StreamAbortedError extends Error {
  constructor() {
    super('Stream cancelled');
    this.name = 'StreamAbortedError';
  }
}

export function cachingNote(progress: number, elapsedMs: number, provider?: string): string {
  const who = provider?.trim() || 'your debrid service';
  if (progress > 0) return `Downloading on ${who}`;
  if (elapsedMs >= CACHING_QUEUED_AFTER_MS) {
    return `Queued on ${who} — it hasn't started downloading yet`;
  }
  return `Asking ${who} to fetch it…`;
}

export function waitForCache(
  caching: StreamResolveCachingResponse,
  resolveReady: () => Promise<StreamResolveResponse>,
  onProgress: (state: CachingState) => void,
  signal: AbortSignal,
  provider?: string
): Promise<PlaybackSessionResponse> {
  const started = Date.now();
  const report = (progress: number) =>
    onProgress({ progress, note: cachingNote(progress, Date.now() - started, provider) });
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
          report(1);
          const session = await resolveReady();
          if (signal.aborted) return;
          if (!isStreamCaching(session)) return settle(() => resolve(session));
        } else {
          report(status.progress ?? 0);
        }
        timer = setTimeout(tick, POLL_MS);
      } catch (e) {
        if (!signal.aborted) settle(() => reject(e));
      }
    };
    report(0);
    timer = setTimeout(tick, POLL_MS);
  });
}
