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
export const MAX_WALK_RESOLVES = 12;
export const MAX_WALK_MS = 30_000;

export interface StreamFailure {
  message: string;
  reason?: string;
  detail?: string;
  release?: string;
}

export interface WalkState {
  tried: Set<string>;
  failures: StreamFailure[];
  attempts: number;
  resolves: number;
}

export interface WalkDeps {
  resolve: (option: StreamOption) => Promise<StreamResolveResponse>;
  isAborted: () => boolean;
  now?: () => number;
  onAttemptFailed?: (failure: StreamFailure) => void;
}

export type WalkResult =
  | { kind: 'ready'; option: StreamOption; session: PlaybackSessionResponse }
  | { kind: 'caching'; option: StreamOption; response: StreamResolveCachingResponse }
  | { kind: 'exhausted'; stopped: boolean }
  | { kind: 'aborted' };

export function optionKey(o: Pick<StreamOption, 'debrid_provider_id' | 'info_hash' | 'files'>) {
  return `${o.debrid_provider_id}|${o.info_hash ?? ''}|${o.files[0]?.id ?? ''}`;
}

export function newWalkState(tried: Iterable<string> = []): WalkState {
  return { tried: new Set(tried), failures: [], attempts: 0, resolves: 0 };
}

export function refillBudget(state: WalkState) {
  state.attempts = 0;
  state.resolves = 0;
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

const isUncached = (o: StreamOption) => o.availability === 'uncached';

// The server sinks failures already, but only the ones it saw before this
// search; an uncached release goes last because resolving it starts a download.
export function fallbackCandidates(options: StreamOption[], state: WalkState): StreamOption[] {
  const rest = untriedOptions(options, state);
  const held = rest.filter((o) => !isUncached(o));
  return [
    ...held.filter((o) => o.playback !== 'failed'),
    ...held.filter((o) => o.playback === 'failed'),
    ...rest.filter(isUncached)
  ];
}

type Refusal = 'skip' | 'stop' | 'attempt';

// An instant refusal costs the viewer nothing, so it is not charged to the
// budget. A rate limit or a rejected key is the same answer for every option
// left, and asking again only makes the limit last longer.
function refusalOf(failure: StreamFailure): Refusal {
  switch (failure.reason) {
    case 'blocked':
    case 'not_video':
      return 'skip';
    case 'rate_limited':
    case 'auth_failed':
      return 'stop';
    default:
      return 'attempt';
  }
}

function walkHasBudget(state: WalkState): boolean {
  return state.attempts < MAX_ATTEMPTS && state.resolves < MAX_WALK_RESOLVES;
}

export async function walkStreams(
  options: StreamOption[],
  state: WalkState,
  deps: WalkDeps
): Promise<WalkResult> {
  const now = deps.now ?? Date.now;
  const started = now();
  let caching: Extract<WalkResult, { kind: 'caching' }> | null = null;
  let stopped = false;

  for (const option of fallbackCandidates(options, state)) {
    if (!walkHasBudget(state) || now() - started >= MAX_WALK_MS) break;
    // Each resolve of an uncached release queues a download on the debrid service.
    if (caching && isUncached(option)) continue;
    state.tried.add(optionKey(option));
    state.resolves++;
    let response: StreamResolveResponse;
    try {
      response = await deps.resolve(option);
    } catch (e) {
      if (deps.isAborted()) return { kind: 'aborted' };
      const failure = toFailure(e, option);
      state.failures.push(failure);
      const refusal = refusalOf(failure);
      if (refusal === 'stop') {
        stopped = true;
        break;
      }
      if (refusal === 'attempt') {
        state.attempts++;
        deps.onAttemptFailed?.(failure);
      }
      continue;
    }
    if (deps.isAborted()) return { kind: 'aborted' };
    if (!isStreamCaching(response)) return { kind: 'ready', option, session: response };
    caching ??= { kind: 'caching', option, response };
  }

  return caching ?? { kind: 'exhausted', stopped };
}

export async function resolveFirstReady(
  options: StreamOption[],
  resolve: WalkDeps['resolve']
): Promise<PlaybackSessionResponse | null> {
  const result = await walkStreams(options, newWalkState(), { resolve, isAborted: () => false });
  return result.kind === 'ready' ? result.session : null;
}
