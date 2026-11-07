import type { SessionFeed, SessionStatusResult } from '#lib/api/client.js';

export const FEED_IDLE_STALL_MS = 10_000;
export const FEED_NOTE_AFTER_MS = 2_500;
export const DIRECT_START_BUDGET_MS = 6_000;
export const TRANSCODE_START_BUDGET_MS = 12_000;
// The server flips a stuck session to `failed` itself; this only bounds a
// server that somehow never does.
export const START_CEILING_MS = 45_000;

// Tight at first, because a session that is gone or a transcode that died on
// its first frame is knowable from the first answer; past that it is a wait.
const START_PROBE_INTERVAL_MS = 400;
const SLOW_PROBE_AFTER_MS = 2_000;
const SLOW_PROBE_INTERVAL_MS = 1_500;

export interface StartProbe {
  elapsedMs: number;
  /** Null while the server still says it is preparing: its lead time is not the player's fault. */
  readyForMs: number | null;
  budgetMs: number;
  feed?: SessionFeed;
  sourceReadable?: boolean;
  servedGrew?: boolean;
  downloadGrew?: boolean;
}

export type StartVerdict =
  { kind: 'wait' } | { kind: 'stalled'; reason: string; sourceHealthy: boolean };

export type StartOutcome =
  | { kind: 'broken'; reason: string }
  | Extract<StartVerdict, { kind: 'stalled' }>
  | { kind: 'aborted' };

const WAIT: StartVerdict = { kind: 'wait' };

function sourceHealthy(feed: SessionFeed, downloadGrew: boolean): boolean {
  const total = feed.total_bytes ?? 0;
  const complete = total > 0 && (feed.downloaded_bytes ?? 0) >= total;
  return (feed.download_bps ?? 0) > 0 || downloadGrew || complete;
}

export function judgeStart(p: StartProbe): StartVerdict {
  const feed = p.feed;
  const healthy = !!feed && sourceHealthy(feed, p.downloadGrew ?? false);
  const stalled = (reason: string, sourceHealthy = healthy): StartVerdict => ({
    kind: 'stalled',
    reason,
    sourceHealthy
  });
  if (p.elapsedMs >= START_CEILING_MS) {
    return stalled(`Nothing played within ${START_CEILING_MS / 1000}s`);
  }
  const ready = p.readyForMs;
  if (ready === null) return WAIT;
  if (!feed) {
    return ready >= p.budgetMs
      ? stalled(
          `Nothing played within ${p.budgetMs / 1000}s of the stream being ready`,
          p.sourceReadable ?? false
        )
      : WAIT;
  }
  const idle = feed.idle_ms;
  if (idle == null) {
    return ready >= DIRECT_START_BUDGET_MS
      ? stalled('The player never asked for the stream')
      : WAIT;
  }
  if (p.servedGrew) return WAIT;
  if (idle > FEED_IDLE_STALL_MS) {
    return stalled(`The stream sent nothing for ${Math.floor(idle / 1000)}s`);
  }
  return WAIT;
}

export function formatBytes(bytes: number): string {
  if (bytes >= 1_000_000_000) return `${Math.floor(bytes / 100_000_000) / 10} GB`;
  if (bytes >= 1_000_000) return `${Math.floor(bytes / 1_000_000)} MB`;
  return `${Math.max(1, Math.floor(bytes / 1_000))} KB`;
}

export function startFeedLine(feed: SessionFeed | undefined): string | null {
  if (!feed) return null;
  if (feed.bytes_served > 0) return `Buffering · ${formatBytes(feed.bytes_served)}`;
  if ((feed.download_bps ?? 0) > 0)
    return `Fetching the source · ${formatBytes(feed.download_bps!)}/s`;
  return null;
}

export function startBudgetMs(method: string): number {
  return method === 'direct_play' ? DIRECT_START_BUDGET_MS : TRANSCODE_START_BUDGET_MS;
}

export interface SuperviseDeps {
  method: string;
  signal: AbortSignal;
  getStatus: () => Promise<SessionStatusResult>;
  onProgress: (line: string | null) => void;
  sleep?: (ms: number) => Promise<void>;
}

const sleepMs = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/**
 * The player cannot tell a slow start from a dead one; the server knows the
 * moment a session is gone and, for debrid, whether bytes still reach the
 * player. Ask it until the caller aborts (first frame) or a verdict lands.
 */
export async function superviseStart(deps: SuperviseDeps): Promise<StartOutcome> {
  const sleep = deps.sleep ?? sleepMs;
  const aborted: StartOutcome = { kind: 'aborted' };
  let waited = 0;
  let interval = START_PROBE_INTERVAL_MS;
  let budget = startBudgetMs(deps.method);
  let readyAt: number | null = 0;
  let feed: SessionFeed | undefined;
  let sourceReadable: boolean | undefined;
  try {
    for (;;) {
      await sleep(interval);
      if (deps.signal.aborted) return aborted;
      waited += interval;
      if (waited >= SLOW_PROBE_AFTER_MS) interval = SLOW_PROBE_INTERVAL_MS;

      let servedGrew = false;
      let downloadGrew = false;
      const result = await deps.getStatus();
      if (deps.signal.aborted) return aborted;
      if (result.kind === 'missing') {
        return { kind: 'broken', reason: 'The server has no record of this playback session' };
      }
      if (result.kind === 'status') {
        const status = result.status;
        if (status.state === 'failed') {
          return {
            kind: 'broken',
            reason: status.message ?? status.error ?? 'The server gave up on this stream'
          };
        }
        budget = startBudgetMs(status.method);
        sourceReadable = status.source_readable;
        readyAt = status.state === 'preparing' ? null : (readyAt ?? waited);
        if (status.feed) {
          const before = feed;
          const now = status.feed;
          servedGrew = !!before && now.bytes_served > before.bytes_served;
          downloadGrew = !!before && (now.downloaded_bytes ?? 0) > (before.downloaded_bytes ?? 0);
          feed = now;
        }
      }
      deps.onProgress(waited >= FEED_NOTE_AFTER_MS ? startFeedLine(feed) : null);
      const verdict = judgeStart({
        elapsedMs: waited,
        readyForMs: readyAt === null ? null : waited - readyAt,
        budgetMs: budget,
        feed,
        sourceReadable,
        servedGrew,
        downloadGrew
      });
      if (verdict.kind === 'stalled') return verdict;
    }
  } finally {
    deps.onProgress(null);
  }
}
