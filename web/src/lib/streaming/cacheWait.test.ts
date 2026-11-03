import { afterEach, describe, expect, it, vi } from 'vitest';

const api = vi.hoisted(() => ({ streamResolveStatus: vi.fn() }));
vi.mock('#lib/api/client.js', async (importOriginal) => ({
  ...(await importOriginal<typeof import('#lib/api/client.js')>()),
  ...api
}));

const { cachingNote, waitForCache } = await import('./cacheWait');
type CachingState = import('./cacheWait').CachingState;

afterEach(() => {
  vi.useRealTimers();
  vi.clearAllMocks();
});

describe('cachingNote', () => {
  it.each<[number, number, string | undefined, string]>([
    [0, 0, 'Real-Debrid', 'Asking Real-Debrid to fetch it…'],
    [0, 20_000, 'Real-Debrid', "Queued on Real-Debrid — it hasn't started downloading yet"],
    [0.3, 25_000, 'Real-Debrid', 'Downloading on Real-Debrid'],
    [0, 20_000, undefined, "Queued on your debrid service — it hasn't started downloading yet"],
    [0, 0, ' ', 'Asking your debrid service to fetch it…']
  ])('%d at %dms on %s', (progress, elapsed, provider, note) => {
    expect(cachingNote(progress, elapsed, provider)).toBe(note);
  });
});

describe('waitForCache', () => {
  it('says the download is queued once it has sat at 0% for 20s', async () => {
    vi.useFakeTimers();
    api.streamResolveStatus.mockResolvedValue({ status: 'downloading', progress: 0 });
    const states: CachingState[] = [];
    const controller = new AbortController();
    const done = waitForCache(
      { session_id: null, status: 'caching', debrid_provider_id: 'rd', torrent_id: 't' },
      async () => {
        throw new Error('not ready');
      },
      (s) => states.push(s),
      controller.signal,
      'Real-Debrid'
    ).catch(() => {});
    await vi.advanceTimersByTimeAsync(21_000);
    controller.abort();
    await done;
    expect(states[0]).toEqual({ progress: 0, note: 'Asking Real-Debrid to fetch it…' });
    expect(states[states.length - 1]).toEqual({
      progress: 0,
      note: "Queued on Real-Debrid — it hasn't started downloading yet"
    });
  });
});
