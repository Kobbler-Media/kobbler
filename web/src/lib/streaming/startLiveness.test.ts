import { describe, expect, it } from 'vitest';
import type { SessionFeed, SessionStatusResult } from '$lib/api/client';
import { formatBytes, judgeStart, startFeedLine, superviseStart } from './startLiveness';
import type { StartProbe } from './startLiveness';

function feed(extra: Partial<SessionFeed> = {}): SessionFeed {
  return { requests_open: 1, bytes_served: 0, ...extra };
}

function ok(state: 'ready' | 'preparing' | 'failed', f?: SessionFeed, message?: string) {
  return {
    kind: 'status',
    status: { session_id: 's', state, method: 'direct_play', is_live: false, feed: f, message }
  } satisfies SessionStatusResult;
}

const probe = (p: Partial<StartProbe>): StartProbe => ({
  elapsedMs: 1000,
  readyForMs: 1000,
  budgetMs: 6000,
  ...p
});

describe('judgeStart', () => {
  it.each<[string, Partial<StartProbe>, ReturnType<typeof judgeStart>]>([
    [
      'waits while the server is still preparing',
      { readyForMs: null, elapsedMs: 30_000 },
      { kind: 'wait' }
    ],
    [
      'calls a start without a feed lost once the budget is spent',
      { readyForMs: 6000 },
      {
        kind: 'stalled',
        reason: 'Nothing played within 6s of the stream being ready',
        sourceHealthy: false
      }
    ],
    [
      'calls a feed the player never asked for lost after 6s',
      { readyForMs: 6000, budgetMs: 12_000, feed: feed({ idle_ms: undefined, download_bps: 10 }) },
      { kind: 'stalled', reason: 'The player never asked for the stream', sourceHealthy: true }
    ],
    [
      'waits while bytes are still going out',
      { feed: feed({ idle_ms: 20_000 }), servedGrew: true },
      { kind: 'wait' }
    ],
    ['waits on a short idle', { feed: feed({ idle_ms: 9000 }) }, { kind: 'wait' }],
    [
      'calls a feed idle past 10s lost',
      { feed: feed({ idle_ms: 12_000 }) },
      { kind: 'stalled', reason: 'The stream sent nothing for 12s', sourceHealthy: false }
    ],
    [
      'counts a finished download as a healthy source',
      { feed: feed({ idle_ms: 12_000, downloaded_bytes: 10, total_bytes: 10 }) },
      { kind: 'stalled', reason: 'The stream sent nothing for 12s', sourceHealthy: true }
    ],
    [
      'stops at the 45s ceiling whatever the server says',
      { elapsedMs: 45_000, readyForMs: null },
      { kind: 'stalled', reason: 'Nothing played within 45s', sourceHealthy: false }
    ]
  ])('%s', (_name, p, verdict) => {
    expect(judgeStart(probe(p))).toEqual(verdict);
  });
});

describe('startFeedLine', () => {
  it.each<[SessionFeed | undefined, string | null]>([
    [undefined, null],
    [feed(), null],
    [feed({ bytes_served: 24_000_000 }), 'Buffering · 24 MB'],
    [feed({ download_bps: 3_000_000 }), 'Fetching the source · 3 MB/s'],
    [feed({ bytes_served: 1_500_000_000 }), 'Buffering · 1.5 GB']
  ])('%j → %s', (f, line) => {
    expect(startFeedLine(f)).toBe(line);
  });

  it('never reports zero kilobytes', () => {
    expect(formatBytes(10)).toBe('1 KB');
  });
});

function run(statuses: SessionStatusResult[], abortAfter?: number) {
  const controller = new AbortController();
  const lines: (string | null)[] = [];
  let calls = 0;
  const outcome = superviseStart({
    method: 'direct_play',
    signal: controller.signal,
    sleep: async () => {
      if (abortAfter !== undefined && calls >= abortAfter) controller.abort();
    },
    getStatus: async () => statuses[Math.min(calls++, statuses.length - 1)],
    onProgress: (line) => lines.push(line)
  });
  return { outcome, lines, calls: () => calls };
}

describe('superviseStart', () => {
  it('reports a session the server no longer has as broken', async () => {
    const r = run([{ kind: 'missing' }]);
    expect(await r.outcome).toEqual({
      kind: 'broken',
      reason: 'The server has no record of this playback session'
    });
  });

  it('passes on what the server says about a failed session', async () => {
    const r = run([ok('failed', undefined, 'probe failed')]);
    expect(await r.outcome).toEqual({ kind: 'broken', reason: 'probe failed' });
  });

  it('keeps waiting while bytes keep reaching the player', async () => {
    const statuses = Array.from({ length: 60 }, (_, i) =>
      ok('ready', feed({ bytes_served: i * 1_000_000, idle_ms: i === 0 ? 100 : 11_000 }))
    );
    const r = run(statuses, 25);
    expect(await r.outcome).toEqual({ kind: 'aborted' });
  });

  it('calls a feed that went idle lost, naming a healthy download', async () => {
    const r = run([ok('ready', feed({ bytes_served: 5, idle_ms: 11_000, download_bps: 100 }))]);
    expect(await r.outcome).toEqual({
      kind: 'stalled',
      reason: 'The stream sent nothing for 11s',
      sourceHealthy: true
    });
  });

  it('does not start the clock while the server is preparing', async () => {
    const preparing = Array.from({ length: 20 }, () => ok('preparing'));
    const r = run([...preparing, ok('ready')]);
    expect(await r.outcome).toEqual({
      kind: 'stalled',
      reason: 'Nothing played within 6s of the stream being ready',
      sourceHealthy: false
    });
    expect(r.calls()).toBeGreaterThan(20);
  });

  it('says how the feed is doing once the start is slow', async () => {
    const statuses = Array.from({ length: 10 }, () =>
      ok('ready', feed({ bytes_served: 24_000_000, idle_ms: 100 }))
    );
    const r = run(statuses, 8);
    await r.outcome;
    expect(r.lines).toContain('Buffering · 24 MB');
    expect(r.lines[0]).toBeNull();
    expect(r.lines[r.lines.length - 1]).toBeNull();
  });

  it('treats an unanswered status request as no news', async () => {
    const r = run([{ kind: 'unknown' }, { kind: 'unknown' }, { kind: 'missing' }]);
    expect(await r.outcome).toMatchObject({ kind: 'broken' });
    expect(r.calls()).toBe(3);
  });
});
