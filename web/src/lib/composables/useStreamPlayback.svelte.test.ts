import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  StreamOption,
  StreamResolveParams,
  StreamResolveResponse,
  StreamResolveStatusResponse,
  StreamSearchResponse
} from '#lib/api/client.js';
import type { PlaybackSessionResponse } from '#lib/types.js';

const api = vi.hoisted(() => ({
  streamSearch: vi.fn(),
  streamLocal: vi.fn(),
  streamResolve: vi.fn(),
  streamResolveStatus: vi.fn(),
  streamPause: vi.fn(),
  reportBadRelease: vi.fn(),
  deleteStreamSession: vi.fn(),
  reportStreamOutcome: vi.fn(),
  streamPrewarm: vi.fn(),
  claimStreamPrewarm: vi.fn(),
  touchStreamPrewarm: vi.fn(),
  releaseStreamPrewarm: vi.fn()
}));

vi.mock('#lib/api/client.js', async (importOriginal) => ({
  ...(await importOriginal<typeof import('#lib/api/client.js')>()),
  ...api
}));
vi.mock('#lib/services/eventstream.svelte.js', () => ({ on: vi.fn(), off: vi.fn() }));
vi.mock('#lib/utils/media.js', () => ({ detectCapabilities: () => ({}) }));

const { ApiError } = await import('#lib/api/client.js');
const { useStreamPlayback } = await import('./useStreamPlayback.svelte');

function option(hash: string): StreamOption {
  return {
    id: hash,
    release_title: `Release ${hash}`,
    size: 1,
    indexer: 'x',
    release_key: hash,
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

type Plan = 'ready' | 'caching' | 'blocked' | 'timeout' | 'rate_limited' | 'hang';

let resolveCount: Record<string, number>;

function serve(
  hashes: string[],
  plan: Record<string, Plan> = {},
  extra: Partial<StreamSearchResponse> = {}
) {
  api.streamSearch.mockResolvedValue({ options: hashes.map(option), errors: [], ...extra });
  api.streamResolve.mockImplementation(
    async (p: StreamResolveParams): Promise<StreamResolveResponse> => {
      const hash = p.release_key!;
      resolveCount[hash] = (resolveCount[hash] ?? 0) + 1;
      const step = plan[hash] ?? 'ready';
      if (step === 'hang') return new Promise(() => {});
      if (step === 'blocked' || step === 'timeout' || step === 'rate_limited') {
        throw new ApiError(502, `refused ${step}`, step, `raw ${hash}`);
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
  const onMessage = vi.fn();
  const stream = useStreamPlayback({
    mediaType: 'movie',
    getMediaId: () => 'm1',
    getSearchParams: () => ({ movie_id: 'm1' }),
    getResumePosition: () => undefined,
    getEstimatedDuration: () => undefined,
    playback: playback as never,
    onMessage,
    want: withWant ? want : undefined
  });
  const played = () => playback.playSession.mock.calls.map(([s]) => s.session_id);
  const lastSession = () => played()[played().length - 1];
  return { stream, playback, want, onMessage, played, lastSession };
}

const resolved = () => api.streamResolve.mock.calls.map(([p]) => p.release_key);
const startTimes = () => api.streamResolve.mock.calls.map(([p]) => p.start_time);

async function settle() {
  for (let i = 0; i < 20; i++) await Promise.resolve();
}

function failStart(
  stream: ReturnType<typeof setup>['stream'],
  sessionId: string,
  started = false,
  position = 0
) {
  return stream.handlePlayerFailure({
    sessionId,
    started,
    position,
    reason: 'media_error_4',
    reported: Promise.resolve()
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.useRealTimers();
  resolveCount = {};
  api.deleteStreamSession.mockResolvedValue(undefined);
  api.reportStreamOutcome.mockResolvedValue(undefined);
  api.touchStreamPrewarm.mockResolvedValue(undefined);
  api.releaseStreamPrewarm.mockResolvedValue(undefined);
  api.streamLocal.mockResolvedValue(null);
  api.streamPause.mockResolvedValue(null);
  api.reportBadRelease.mockResolvedValue(undefined);
});

describe('useStreamPlayback', () => {
  it('plays the release already on disk without searching', async () => {
    serve(['a']);
    api.streamLocal.mockResolvedValue(option('disk'));
    const { stream, played } = setup();
    await stream.autoPlay();
    expect(api.streamLocal).toHaveBeenCalledWith('movie', 'm1');
    expect(resolved()).toEqual(['disk']);
    expect(played()).toEqual(['disk-1']);
    expect(api.streamSearch).not.toHaveBeenCalled();
  });

  it('resolves an NZB kobload has not imported by its indexer listing', async () => {
    const nzb_ref = { indexer_id: '7', guid: 'g1' };
    api.streamSearch.mockResolvedValue({
      options: [{ ...option('x'), release_key: undefined, nzb_ref, source_type: 'usenet' }],
      errors: []
    });
    api.streamResolve.mockResolvedValue(session('nzb'));
    const { stream, played } = setup();
    await stream.autoPlay();
    expect(api.streamResolve.mock.calls.map(([p]) => [p.release_key, p.nzb_ref])).toEqual([
      [undefined, nzb_ref]
    ]);
    expect(played()).toEqual(['nzb']);
  });

  it('searches when the release on disk will not open', async () => {
    serve(['a'], { disk: 'blocked' });
    api.streamLocal.mockResolvedValue(option('disk'));
    const { stream, played } = setup();
    await stream.autoPlay();
    expect(resolved()).toEqual(['disk', 'a']);
    expect(played()).toEqual(['a-1']);
  });

  it('a title with a release on disk is not searched when its page opens', async () => {
    serve(['a']);
    api.streamPrewarm.mockResolvedValue(null);
    api.streamLocal.mockResolvedValue(option('disk'));
    const { stream } = setup();
    await stream.preWarmStream();
    expect(api.streamSearch).not.toHaveBeenCalled();
    expect(stream.knownOptions?.map((o) => o.release_key)).toEqual(['disk']);
  });

  it('plays the preferred option first without showing the list', async () => {
    serve(['a', 'b', 'c'], {}, { preferred_release_key: 'c' });
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
      stream.handlePlayerFailure({
        sessionId: 'a-1',
        started: false,
        position: 0,
        reason: 'x',
        reported
      })
    ).toBe(true);
    expect(stream.playerProps.pendingStatus).toBe("That stream wouldn't play — trying another…");
    await reported;
    await settle();
    expect(api.deleteStreamSession).toHaveBeenCalledWith('a-1');
    expect(order).toEqual(['outcome', 'delete']);
    expect(played()).toEqual(['a-1', 'b-1']);
    expect(playback.openPending).toHaveBeenCalledTimes(2);
  });

  it('reopens a running stream where it broke, quietly at first', async () => {
    vi.useFakeTimers();
    serve(['a', 'b'], {});
    const { stream, lastSession, onMessage } = setup();
    await stream.autoPlay();
    api.streamResolve.mockImplementationOnce(() => new Promise(() => {}));
    failStart(stream, 'a-1', true, 600);
    expect(stream.playerProps.pendingStatus).toBe('');
    await vi.advanceTimersByTimeAsync(1_799);
    expect(stream.playerProps.pendingStatus).toBe('');
    await vi.advanceTimersByTimeAsync(1);
    expect(stream.playerProps.pendingStatus).toBe('Lost the stream — reconnecting…');
    expect(startTimes()).toEqual([undefined, 600]);
    expect(stream.playerProps.resumePosition).toBe(600);
    expect(api.reportStreamOutcome).not.toHaveBeenCalled();
    expect(lastSession()).toBe('a-1');
    expect(onMessage).not.toHaveBeenCalled();
  });

  it('switches to the next release when the reopened stream breaks too', async () => {
    serve(['a', 'b']);
    const { stream, lastSession, onMessage } = setup();
    await stream.autoPlay();
    failStart(stream, 'a-1', true, 600);
    await settle();
    expect(lastSession()).toBe('a-2');
    failStart(stream, 'a-2', true, 630);
    await settle();
    expect(stream.failure).toBeNull();
    expect(lastSession()).toBe('b-1');
    expect(resolved()).toEqual(['a', 'a', 'b']);
    expect(startTimes()[2]).toBe(625);
    expect(stream.playerProps.resumePosition).toBe(625);
    expect(api.reportStreamOutcome).toHaveBeenCalledWith('a-2', {
      ok: false,
      reason: 'media_error_4'
    });
    expect(onMessage).toHaveBeenCalledWith('Switched to another source');
  });

  it('switches when the reopen itself is refused', async () => {
    serve(['a', 'b']);
    const { stream, lastSession, onMessage } = setup();
    await stream.autoPlay();
    api.streamResolve.mockRejectedValueOnce(new ApiError(502, 'gone', 'timeout'));
    failStart(stream, 'a-1', true, 100);
    await settle();
    expect(lastSession()).toBe('b-1');
    expect(startTimes()[2]).toBe(95);
    expect(onMessage).toHaveBeenCalledWith('Switched to another source');
  });

  it('shows the dialog only once the switch has nothing left to try', async () => {
    serve(['a']);
    const { stream, onMessage } = setup();
    await stream.autoPlay();
    failStart(stream, 'a-1', true, 600);
    await settle();
    failStart(stream, 'a-2', true, 610);
    await settle();
    expect(stream.failure).toMatchObject({ title: 'Playback stopped', canTryAnother: false });
    expect(onMessage).not.toHaveBeenCalled();
  });

  it('starts over from the page position after a fresh press', async () => {
    serve(['a', 'b']);
    const { stream } = setup();
    await stream.autoPlay();
    failStart(stream, 'a-1', true, 600);
    await settle();
    await stream.autoPlay();
    expect(stream.playerProps.resumePosition).toBeUndefined();
    expect(startTimes().at(-1)).toBeUndefined();
  });

  it('shows a friendly summary when nothing plays, and Try another continues', async () => {
    const hashes = 'abcdefgh'.split('');
    serve(hashes, {
      ...Object.fromEntries(hashes.slice(0, 6).map((h) => [h, 'timeout' as Plan])),
      g: 'blocked'
    });
    const { stream, playback, lastSession } = setup();
    await stream.autoPlay();
    expect(resolved()).toEqual(hashes.slice(0, 6));
    expect(playback.close).toHaveBeenCalled();
    expect(stream.failure).toMatchObject({
      title: "Couldn't play this",
      summary:
        'None of the 6 streams we tried would play. Your debrid service took too long to respond.',
      canTryAnother: true,
      canChoose: true,
      canWant: true
    });
    expect(stream.failure!.details[0]).toBe('Release a: raw a');
    expect(stream.showStreamModal).toBe(false);
    await stream.tryAnother();
    expect(resolved()).toEqual(hashes);
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
    expect(stream.caching).toEqual({ progress: 0.5, note: 'Downloading on rd' });
    expect(stream.playerProps.pendingCaching).toEqual(stream.caching);
    api.streamResolve.mockResolvedValueOnce(session('b-ready'));
    await vi.advanceTimersByTimeAsync(3000);
    await done;
    expect(played()).toEqual(['b-ready']);
    expect(stream.caching).toBeNull();
  });

  it('stops at the panel when a picked release will not resolve', async () => {
    serve(['a', 'b', 'c'], { b: 'timeout' });
    const { stream, played } = setup();
    await stream.playOption(option('b'));
    expect(resolved()).toEqual(['b']);
    expect(played()).toEqual([]);
    expect(api.streamSearch).toHaveBeenCalledTimes(1);
    expect(stream.failure).toMatchObject({
      title: "Couldn't play this",
      summary: "The stream we tried wouldn't play. Your debrid service took too long to respond.",
      details: ['Release b: raw b'],
      canTryAnother: true,
      canChoose: true
    });
  });

  it('stops at the panel when the player cannot start a picked release', async () => {
    serve(['a', 'b']);
    const { stream, played } = setup();
    await stream.autoPlay();
    await stream.playOption(option('b'));
    expect(failStart(stream, 'b-1')).toBe(true);
    await settle();
    expect(resolved()).toEqual(['a', 'b']);
    expect(played()).toEqual(['a-1', 'b-1']);
    expect(stream.failure).toMatchObject({
      title: "Couldn't play this",
      summary: "The stream we tried wouldn't play. Your browser couldn't play them.",
      canChoose: true
    });
  });

  it('reopens a picked release once, then stops at the panel when it drops again', async () => {
    serve(['a', 'b']);
    const { stream, lastSession, onMessage } = setup();
    await stream.playOption(option('a'));
    failStart(stream, 'a-1', true, 600);
    await settle();
    expect(lastSession()).toBe('a-2');
    failStart(stream, 'a-2', true, 630);
    await settle();
    expect(resolved()).toEqual(['a', 'a']);
    expect(stream.failure).toMatchObject({ title: 'Playback stopped', canChoose: true });
    expect(api.streamSearch).toHaveBeenCalledTimes(1);
    expect(onMessage).not.toHaveBeenCalled();
  });

  it('stops at the panel when reopening a picked release is refused', async () => {
    serve(['a', 'b']);
    const { stream, played } = setup();
    await stream.playOption(option('a'));
    api.streamResolve.mockRejectedValueOnce(new ApiError(502, 'gone', 'timeout'));
    failStart(stream, 'a-1', true, 100);
    await settle();
    expect(resolved()).toEqual(['a', 'a']);
    expect(played()).toEqual(['a-1']);
    expect(stream.failure).toMatchObject({ title: 'Playback stopped' });
  });

  it('walks on from a failed pick once Try another is pressed', async () => {
    serve(['a', 'b', 'c'], { b: 'timeout' });
    const { stream, played } = setup();
    await stream.playOption(option('b'));
    await stream.tryAnother();
    expect(resolved()).toEqual(['b', 'a']);
    failStart(stream, 'a-1');
    await settle();
    expect(played()).toEqual(['a-1', 'c-1']);
    expect(stream.failure).toBeNull();
  });

  it('forgets the pick on the next automatic play', async () => {
    serve(['a', 'b'], { a: 'timeout' });
    const { stream, played } = setup();
    await stream.playOption(option('a'));
    await stream.autoPlay();
    expect(played()).toEqual(['b-1']);
  });

  it('says it is finding a stream from the moment Stream is pressed', async () => {
    serve(['a'], { a: 'hang' });
    const { stream, playback } = setup();
    void stream.autoPlay();
    expect(playback.openPending).toHaveBeenCalled();
    expect(stream.playerProps.pendingStatus).toBe('Finding a stream…');
    await settle();
    expect(stream.playerProps.pendingStatus).toBe('Finding a stream…');
  });

  it('stays quiet about instant refusals and speaks up after a real failure', async () => {
    serve(['a', 'b', 'c'], { a: 'blocked', b: 'timeout', c: 'hang' });
    const { stream } = setup();
    void stream.autoPlay();
    await settle();
    expect(resolved()).toEqual(['a', 'b', 'c']);
    expect(stream.playerProps.pendingStatus).toBe("That stream wouldn't play — trying another…");

    serve(['a', 'b'], { a: 'blocked', b: 'hang' });
    const quiet = setup();
    void quiet.stream.autoPlay();
    await settle();
    expect(quiet.stream.playerProps.pendingStatus).toBe('Finding a stream…');
  });

  it('says why a resolve is being held while the debrid account is paused', async () => {
    vi.useFakeTimers();
    serve(['a'], { a: 'hang' });
    const { stream } = setup();
    void stream.autoPlay();
    await vi.advanceTimersByTimeAsync(1_500);
    expect(api.streamPause).toHaveBeenCalledWith('rd');
    expect(stream.playerProps.pendingStatus).toBe('Finding a stream…');
    api.streamPause.mockResolvedValue({ reason: 'adds_paused', seconds: 27 });
    await vi.advanceTimersByTimeAsync(3_000);
    expect(stream.playerProps.pendingStatus).toBe(
      'Your debrid service is pausing new requests on your account. Trying again in about 27s…'
    );
    stream.cancel();
    await vi.advanceTimersByTimeAsync(30_000);
    expect(api.streamPause).toHaveBeenCalledTimes(2);
    expect(stream.playerProps.pendingStatus).toBeUndefined();
  });

  it('marks a reported release in place and undoes it without searching again', async () => {
    serve(['a', 'b']);
    const { stream, played } = setup();
    stream.openModal();
    await settle();
    const worked = { ...stream.streamOptions[0], playback: 'worked' as const };
    api.streamSearch.mockResolvedValue({ options: [worked, option('b')], errors: [] });
    stream.reset();
    stream.openModal();
    await settle();
    api.streamSearch.mockClear();

    await stream.reportBad(stream.streamOptions[0], true);
    expect(api.reportBadRelease).toHaveBeenCalledWith(worked, true);
    expect(stream.streamOptions[0]).toMatchObject({ reported_bad: true, playback: 'failed' });
    expect(stream.streamOptions[1].reported_bad).toBeUndefined();

    await stream.autoPlay();
    expect(played()).toEqual(['b-1']);

    await stream.reportBad(stream.streamOptions[0], false);
    expect(api.reportBadRelease).toHaveBeenLastCalledWith(expect.anything(), false);
    expect(stream.streamOptions[0]).toEqual(worked);
    expect(api.streamSearch).not.toHaveBeenCalled();
  });

  it('clears the failed mark of a release that arrived already reported', async () => {
    serve([]);
    const reported = { ...option('a'), reported_bad: true, playback: 'failed' as const };
    api.streamSearch.mockResolvedValue({ options: [reported], errors: [] });
    const { stream } = setup();
    stream.openModal();
    await settle();
    await stream.reportBad(stream.streamOptions[0], false);
    expect(stream.streamOptions[0]).toEqual(option('a'));
  });

  it('still plays a reported release the viewer picks', async () => {
    serve(['a']);
    const { stream, played } = setup();
    await stream.playOption({ ...option('a'), reported_bad: true });
    expect(played()).toEqual(['a-1']);
  });

  it('leaves the release alone and says so when the report is refused', async () => {
    serve(['a']);
    api.reportBadRelease.mockRejectedValue(new ApiError(500, 'Report failed'));
    const { stream, onMessage } = setup();
    stream.openModal();
    await settle();
    await stream.reportBad(stream.streamOptions[0], true);
    expect(stream.streamOptions[0].reported_bad).toBeUndefined();
    expect(onMessage).toHaveBeenCalledWith('Report failed');
  });

  it('stops walking when the debrid service is rate limiting', async () => {
    serve(['a', 'b'], { a: 'rate_limited' });
    const { stream, played } = setup();
    await stream.autoPlay();
    expect(resolved()).toEqual(['a']);
    expect(played()).toEqual([]);
    expect(stream.failure?.summary).toBe(
      "The stream we tried wouldn't play. Your debrid service is limiting requests right now."
    );
  });

  it('shows caching inside the player only when nothing else will play', async () => {
    vi.useFakeTimers();
    serve(['a', 'b'], { a: 'caching', b: 'timeout' });
    api.streamResolveStatus.mockResolvedValue({ status: 'downloading', progress: 0 });
    const { stream, playback } = setup();
    void stream.autoPlay();
    await vi.advanceTimersByTimeAsync(0);
    expect(resolved()).toEqual(['a', 'b']);
    expect(playback.close).not.toHaveBeenCalled();
    expect(stream.playerProps.pendingCaching).toEqual({
      progress: 0,
      note: 'Asking rd to fetch it…'
    });
    expect(stream.playerProps.pendingStatus).toBeUndefined();
    await vi.advanceTimersByTimeAsync(21_000);
    expect(stream.caching?.note).toBe("Queued on rd — it hasn't started downloading yet");
    stream.cancel();
    expect(stream.caching).toBeNull();
  });

  it('leaves sessions it did not start to the player', async () => {
    serve(['a']);
    const { stream } = setup();
    await stream.autoPlay();
    expect(failStart(stream, 'someone-else')).toBe(false);
  });
});

describe('instant stream', () => {
  const prewarmed = (option_ = option('w')) => ({
    state: 'ready',
    prewarm_id: 'pw1',
    option: option_
  });

  it('warms a second after the page asks and keeps it alive while visible', async () => {
    vi.useFakeTimers();
    serve(['a']);
    api.streamPrewarm.mockResolvedValue(prewarmed());
    const { stream } = setup();
    void stream.preWarmStream();
    await vi.advanceTimersByTimeAsync(999);
    expect(api.streamPrewarm).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(api.streamPrewarm).toHaveBeenCalledWith({
      media_type: 'movie',
      media_id: 'm1',
      capabilities: {},
      start_time: undefined
    });
    await vi.advanceTimersByTimeAsync(90_000);
    expect(api.touchStreamPrewarm).toHaveBeenCalledWith('pw1');
    void stream.preWarmStream();
    await vi.advanceTimersByTimeAsync(2_000);
    expect(api.streamPrewarm).toHaveBeenCalledTimes(1);
  });

  it('plays the claimed session on Stream without walking', async () => {
    vi.useFakeTimers();
    serve(['a']);
    api.streamPrewarm.mockResolvedValue(prewarmed());
    api.claimStreamPrewarm.mockResolvedValue(session('claimed'));
    const { stream, played } = setup();
    void stream.preWarmStream();
    await vi.advanceTimersByTimeAsync(1_000);
    await stream.autoPlay();
    expect(api.claimStreamPrewarm).toHaveBeenCalledWith('pw1');
    expect(played()).toEqual(['claimed']);
    expect(resolved()).toEqual([]);
    expect(api.releaseStreamPrewarm).not.toHaveBeenCalled();
  });

  it('reopens the claimed release when it breaks mid-play', async () => {
    vi.useFakeTimers();
    serve(['a', 'w']);
    api.streamPrewarm.mockResolvedValue(prewarmed());
    api.claimStreamPrewarm.mockResolvedValue(session('claimed'));
    const { stream, lastSession } = setup();
    void stream.preWarmStream();
    await vi.advanceTimersByTimeAsync(1_000);
    await stream.autoPlay();
    failStart(stream, 'claimed', true, 50);
    await vi.advanceTimersByTimeAsync(0);
    expect(resolved()).toEqual(['w']);
    expect(lastSession()).toBe('w-1');
  });

  it.each([404, 409, 410])('walks as usual when the claim answers %i', async (status) => {
    vi.useFakeTimers();
    serve(['a']);
    api.streamPrewarm.mockResolvedValue(prewarmed());
    api.claimStreamPrewarm.mockRejectedValue(new ApiError(status, 'gone'));
    const { stream, played } = setup();
    void stream.preWarmStream();
    await vi.advanceTimersByTimeAsync(1_000);
    await stream.autoPlay();
    expect(played()).toEqual(['a-1']);
  });

  it('walks as usual when nothing was warmed', async () => {
    vi.useFakeTimers();
    serve(['a']);
    api.streamPrewarm.mockResolvedValue(undefined);
    const { stream, played } = setup();
    void stream.preWarmStream();
    await vi.advanceTimersByTimeAsync(1_000);
    await stream.autoPlay();
    expect(api.claimStreamPrewarm).not.toHaveBeenCalled();
    expect(played()).toEqual(['a-1']);
  });

  it('releases the prewarm when the page lets go of it', async () => {
    vi.useFakeTimers();
    serve(['a']);
    api.streamPrewarm.mockResolvedValue(prewarmed());
    const { stream } = setup();
    void stream.preWarmStream();
    await vi.advanceTimersByTimeAsync(1_000);
    stream.reset();
    expect(api.releaseStreamPrewarm).toHaveBeenCalledWith('pw1', false);
    await vi.advanceTimersByTimeAsync(90_000);
    expect(api.touchStreamPrewarm).not.toHaveBeenCalled();
  });

  it('releases a prewarm that answers after the page let go', async () => {
    vi.useFakeTimers();
    serve(['a']);
    let answer: (v: unknown) => void = () => {};
    api.streamPrewarm.mockReturnValue(new Promise((r) => (answer = r)));
    const { stream } = setup();
    void stream.preWarmStream();
    await vi.advanceTimersByTimeAsync(1_000);
    stream.reset();
    answer(prewarmed());
    await vi.advanceTimersByTimeAsync(0);
    expect(api.releaseStreamPrewarm.mock.calls.map(([id]) => id)).toEqual(['pw1']);
  });
});
