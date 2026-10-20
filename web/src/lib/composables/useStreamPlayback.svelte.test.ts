import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  StreamOption,
  StreamResolveParams,
  StreamResolveResponse,
  StreamResolveStatusResponse,
  StreamSearchResponse
} from '$lib/api/client';
import type { PlaybackSessionResponse } from '$lib/types';

const api = vi.hoisted(() => ({
  streamSearch: vi.fn(),
  streamResolve: vi.fn(),
  streamResolveStatus: vi.fn(),
  deleteStreamSession: vi.fn()
}));

vi.mock('$lib/api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/api/client')>()),
  ...api
}));
vi.mock('$lib/services/eventstream.svelte', () => ({ on: vi.fn(), off: vi.fn() }));
vi.mock('$lib/utils/media', () => ({ detectCapabilities: () => ({}) }));

const { ApiError } = await import('$lib/api/client');
const { useStreamPlayback } = await import('./useStreamPlayback.svelte');
const { optionKey } = await import('$lib/streaming/walk');

function option(hash: string): StreamOption {
  return {
    id: hash,
    release_title: `Release ${hash}`,
    size: 1,
    indexer: 'x',
    info_hash: hash,
    total_score: 1,
    match_score: 1,
    quality_score: 1,
    debrid_provider: 'rd',
    debrid_provider_id: 'rd',
    cached: false,
    source_type: 'torrent',
    files: [{ id: 1, path: 'a.mkv', size: 1 }]
  };
}

function session(id: string): PlaybackSessionResponse {
  return {
    session_id: id,
    method: 'direct_play',
    playlist_url: null,
    direct_url: `http://x/${id}`,
    duration_seconds: 10,
    subtitles: [],
    audio_tracks: []
  };
}

type Plan = 'ready' | 'caching' | 'blocked';

let resolveCount: Record<string, number>;

function serve(
  hashes: string[],
  plan: Record<string, Plan> = {},
  extra: Partial<StreamSearchResponse> = {}
) {
  api.streamSearch.mockResolvedValue({ options: hashes.map(option), errors: [], ...extra });
  api.streamResolve.mockImplementation(
    async (p: StreamResolveParams): Promise<StreamResolveResponse> => {
      const hash = p.info_hash!;
      resolveCount[hash] = (resolveCount[hash] ?? 0) + 1;
      const step = plan[hash] ?? 'ready';
      if (step === 'blocked') {
        throw new ApiError(
          502,
          'The debrid provider refused this release.',
          'blocked',
          `raw ${hash}`
        );
      }
      if (step === 'caching') {
        return { session_id: null, status: 'caching', debrid_provider_id: 'rd', torrent_id: hash };
      }
      return session(`${hash}-${resolveCount[hash]}`);
    }
  );
}

function setup(withWant = true) {
  const playback = {
    openPending: vi.fn(),
    playSession: vi.fn(),
    close: vi.fn()
  };
  const want = vi.fn(async () => {});
  const stream = useStreamPlayback({
    mediaType: 'movie',
    getMediaId: () => 'm1',
    getSearchParams: () => ({ movie_id: 'm1' }),
    getResumePosition: () => undefined,
    getEstimatedDuration: () => undefined,
    playback: playback as never,
    onMessage: vi.fn(),
    want: withWant ? want : undefined
  });
  const played = () => playback.playSession.mock.calls.map(([s]) => s.session_id);
  const lastSession = () => played()[played().length - 1];
  return { stream, playback, want, played, lastSession };
}

const resolved = () => api.streamResolve.mock.calls.map(([p]) => p.info_hash);

async function settle() {
  for (let i = 0; i < 10; i++) await Promise.resolve();
}

function failStart(stream: ReturnType<typeof setup>['stream'], sessionId: string, started = false) {
  return stream.handlePlayerFailure({
    sessionId,
    started,
    reason: 'media_error_4',
    reported: Promise.resolve()
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.useRealTimers();
  resolveCount = {};
  api.deleteStreamSession.mockResolvedValue(undefined);
});

describe('useStreamPlayback', () => {
  it('plays the preferred option first without showing the list', async () => {
    serve(['a', 'b', 'c'], {}, { preferred_info_hash: 'c' });
    const { stream, played } = setup();
    await stream.autoPlay();
    expect(resolved()).toEqual(['c']);
    expect(played()).toEqual(['c-1']);
    expect(stream.showStreamModal).toBe(false);
  });

  it('skips caching and blocked options in rank order', async () => {
    serve(['a', 'b', 'c'], { a: 'blocked', b: 'caching' });
    const { stream, played } = setup();
    await stream.autoPlay();
    expect(resolved()).toEqual(['a', 'b', 'c']);
    expect(played()).toEqual(['c-1']);
  });

  it('moves to the next untried option when the player cannot start one', async () => {
    serve(['a', 'b']);
    const { stream, played, playback } = setup();
    await stream.autoPlay();
    const order: string[] = [];
    const reported = new Promise<void>((r) => setTimeout(r, 0)).then(() => {
      order.push('outcome');
    });
    api.deleteStreamSession.mockImplementation(async () => {
      order.push('delete');
    });
    expect(
      stream.handlePlayerFailure({ sessionId: 'a-1', started: false, reason: 'x', reported })
    ).toBe(true);
    expect(stream.playerProps.pendingStatus).toBe("That stream wouldn't play — trying another…");
    await reported;
    await settle();
    expect(api.deleteStreamSession).toHaveBeenCalledWith('a-1');
    expect(order).toEqual(['outcome', 'delete']);
    expect(played()).toEqual(['a-1', 'b-1']);
    expect(playback.openPending).toHaveBeenCalledTimes(2);
  });

  it('retries a running stream once, then shows the panel', async () => {
    serve(['a', 'b']);
    const { stream, lastSession } = setup();
    await stream.autoPlay();
    failStart(stream, 'a-1', true);
    await settle();
    expect(lastSession()).toBe('a-2');
    failStart(stream, 'a-2', true);
    expect(stream.failure).toMatchObject({ title: 'Playback stopped', canTryAnother: true });
    await stream.tryAnother();
    expect(lastSession()).toBe('b-1');
  });

  it('shows a friendly summary when nothing plays, and Try another continues', async () => {
    const hashes = 'abcdefgh'.split('');
    serve(hashes, Object.fromEntries(hashes.slice(0, 7).map((h) => [h, 'blocked' as Plan])));
    const { stream, playback, lastSession } = setup();
    await stream.autoPlay();
    expect(resolved()).toEqual(hashes.slice(0, 6));
    expect(playback.close).toHaveBeenCalled();
    expect(stream.failure).toMatchObject({
      title: "Couldn't play this",
      summary:
        'None of the 6 streams we tried would play. Your debrid service refused the release.',
      canTryAnother: true,
      canChoose: true,
      canWant: true
    });
    expect(stream.failure!.details[0]).toBe('Release a: raw a');
    expect(stream.showStreamModal).toBe(false);
    await stream.tryAnother();
    expect(lastSession()).toBe('h-1');
    expect(stream.failure).toBeNull();
  });

  it('offers Want and no list when the search finds nothing', async () => {
    serve([]);
    const { stream, want } = setup();
    await stream.autoPlay();
    expect(stream.failure).toMatchObject({
      title: 'No streams found',
      canChoose: false,
      canTryAnother: false,
      canWant: true
    });
    await stream.want();
    expect(want).toHaveBeenCalled();
    expect(stream.failure).toBeNull();
  });

  it('hides Want when the host cannot want this title', async () => {
    serve([]);
    const { stream } = setup(false);
    await stream.autoPlay();
    expect(stream.failure?.canWant).toBe(false);
  });

  it('opens the release list only when asked', async () => {
    serve(['a'], { a: 'blocked' });
    const { stream } = setup();
    await stream.autoPlay();
    stream.chooseRelease();
    expect(stream.failure).toBeNull();
    expect(stream.showStreamModal).toBe(true);
  });

  it('waits with progress on a picked release that is still caching', async () => {
    vi.useFakeTimers();
    serve(['a', 'b'], { b: 'caching' });
    api.streamResolveStatus.mockResolvedValueOnce({
      status: 'downloading',
      progress: 0.5
    } satisfies StreamResolveStatusResponse);
    api.streamResolveStatus.mockResolvedValueOnce({ status: 'ready' });
    const { stream, played } = setup();
    const done = stream.playOption(option('b'));
    await vi.advanceTimersByTimeAsync(3000);
    expect(stream.cachingProgress).toBe(0.5);
    api.streamResolve.mockResolvedValueOnce(session('b-ready'));
    await vi.advanceTimersByTimeAsync(3000);
    await done;
    expect(played()).toEqual(['b-ready']);
    expect(stream.cachingProgress).toBeNull();
  });

  it('continues a discover handoff without retrying the release it already tried', async () => {
    serve(['a', 'b']);
    const { stream, played } = setup();
    stream.adoptSession({
      session: session('handoff'),
      option: option('a'),
      tried: [optionKey(option('a'))]
    });
    failStart(stream, 'handoff');
    await settle();
    expect(resolved()).toEqual(['b']);
    expect(played()).toEqual(['handoff', 'b-1']);
  });

  it('leaves sessions it did not start to the player', async () => {
    serve(['a']);
    const { stream } = setup();
    await stream.autoPlay();
    expect(failStart(stream, 'someone-else')).toBe(false);
  });
});
