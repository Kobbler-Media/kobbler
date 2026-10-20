import { ApiError, isStreamCaching } from '$lib/api/client';
import type {
  StreamOption,
  StreamResolveCachingResponse,
  StreamResolveResponse,
  StreamSearchResponse
} from '$lib/api/client';
import type { PlaybackSessionResponse } from '$lib/types';

// kobload prewarms the top 8 in rank order; past ~6 the next option is
// rarely warm, so walking further only trades one cold wait for another.
export const MAX_ATTEMPTS = 6;

export interface StreamFailure {
  message: string;
  reason?: string;
  detail?: string;
  release?: string;
}

interface CachingCandidate {
  option: StreamOption;
  response: StreamResolveCachingResponse;
}

export interface WalkState {
  tried: Set<string>;
  failures: StreamFailure[];
  caching: CachingCandidate[];
  budget: number;
}

export interface WalkDeps {
  resolve: (option: StreamOption) => Promise<StreamResolveResponse>;
  waitForCache?: (
    option: StreamOption,
    response: StreamResolveCachingResponse
  ) => Promise<PlaybackSessionResponse>;
  isAborted: () => boolean;
  onAttempt?: (attempt: number, option: StreamOption) => void;
}

export type WalkResult =
  | { kind: 'ready'; option: StreamOption; session: PlaybackSessionResponse }
  | { kind: 'exhausted' }
  | { kind: 'aborted' };

export function optionKey(o: Pick<StreamOption, 'debrid_provider_id' | 'info_hash' | 'files'>) {
  return `${o.debrid_provider_id}|${o.info_hash ?? ''}|${o.files[0]?.id ?? ''}`;
}

export function newWalkState(tried: Iterable<string> = []): WalkState {
  return { tried: new Set(tried), failures: [], caching: [], budget: MAX_ATTEMPTS };
}

export function rankOptions(result: StreamSearchResponse): StreamOption[] {
  const ranked = [...result.options];
  if (!result.preferred_info_hash) return ranked;
  const idx = ranked.findIndex(
    (o) =>
      o.info_hash === result.preferred_info_hash &&
      (result.preferred_file_id == null || o.files[0]?.id === result.preferred_file_id)
  );
  if (idx > 0) ranked.unshift(...ranked.splice(idx, 1));
  return ranked;
}

export function toFailure(error: unknown, option?: StreamOption): StreamFailure {
  const release = option?.release_title;
  if (error instanceof ApiError) {
    return { message: error.message, reason: error.reason, detail: error.detail, release };
  }
  return { message: error instanceof Error ? error.message : String(error), release };
}

export function untriedOptions(options: StreamOption[], state: WalkState): StreamOption[] {
  return options.filter((o) => !state.tried.has(optionKey(o)));
}

const ABORTED: WalkResult = { kind: 'aborted' };

export async function walkStreams(
  options: StreamOption[],
  state: WalkState,
  deps: WalkDeps
): Promise<WalkResult> {
  for (;;) {
    for (const option of untriedOptions(options, state)) {
      if (state.budget <= 0) break;
      state.tried.add(optionKey(option));
      state.budget--;
      deps.onAttempt?.(state.tried.size, option);
      let response: StreamResolveResponse;
      try {
        response = await deps.resolve(option);
      } catch (e) {
        if (deps.isAborted()) return ABORTED;
        state.failures.push(toFailure(e, option));
        continue;
      }
      if (deps.isAborted()) return ABORTED;
      if (isStreamCaching(response)) {
        state.caching.push({ option, response });
        continue;
      }
      return { kind: 'ready', option, session: response };
    }

    const wait = deps.waitForCache;
    const next = wait && state.caching.shift();
    if (!wait || !next) return { kind: 'exhausted' };
    try {
      const session = await wait(next.option, next.response);
      if (deps.isAborted()) return ABORTED;
      return { kind: 'ready', option: next.option, session };
    } catch (e) {
      if (deps.isAborted()) return ABORTED;
      state.failures.push(toFailure(e, next.option));
      state.budget = MAX_ATTEMPTS;
    }
  }
}

export async function resolveFirstReady(
  options: StreamOption[],
  resolve: WalkDeps['resolve']
): Promise<PlaybackSessionResponse | null> {
  const result = await walkStreams(options, newWalkState(), { resolve, isAborted: () => false });
  return result.kind === 'ready' ? result.session : null;
}
