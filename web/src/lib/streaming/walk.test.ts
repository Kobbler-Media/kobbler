import { describe, expect, it } from 'vitest';
import { ApiError } from '$lib/api/client';
import type {
  StreamAvailability,
  StreamOption,
  StreamResolveCachingResponse,
  StreamResolveResponse,
  StreamSearchResponse
} from '$lib/api/client';
import type { PlaybackSessionResponse } from '$lib/types';
import {
  MAX_ATTEMPTS,
  MAX_WALK_RESOLVES,
  fallbackCandidates,
  newWalkState,
  optionKey,
  rankOptions,
  walkStreams
} from './walk';
import type { WalkDeps } from './walk';

function option(
  hash: string,
  extra: Partial<StreamOption> & { fileId?: number; provider?: string } = {}
): StreamOption {
  const { fileId = 1, provider = 'rd', ...rest } = extra;
  return {
    id: `${provider}-${hash}-${fileId}`,
    release_title: `Release ${hash}`,
    size: 1,
    indexer: 'x',
    info_hash: hash,
    total_score: 1,
    match_score: 1,
    quality_score: 1,
    debrid_provider: provider,
    debrid_provider_id: provider,
    cached: false,
    source_type: 'torrent',
    files: [{ id: fileId, path: 'a.mkv', size: 1 }],
    ...rest
  };
}

const avail = (hash: string, availability: StreamAvailability) => option(hash, { availability });

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

function caching(hash: string): StreamResolveCachingResponse {
  return { session_id: null, status: 'caching', debrid_provider_id: 'rd', torrent_id: `t-${hash}` };
}

type Behaviour = 'ready' | 'caching' | ApiError | Error;

const refusal = (reason: string) =>
  new ApiError(502, `refused: ${reason}`, reason, `raw ${reason}`);
const blocked = () => refusal('blocked');

function harness(behaviour: Record<string, Behaviour>, msPerResolve = 0) {
  const resolved: string[] = [];
  let clock = 0;
  let aborted = false;
  let attemptFailures = 0;
  const deps: WalkDeps = {
    resolve: async (o): Promise<StreamResolveResponse> => {
      const hash = o.info_hash!;
      resolved.push(hash);
      clock += msPerResolve;
      const b = behaviour[hash] ?? 'ready';
      if (b === 'ready') return session(`s-${hash}`);
      if (b === 'caching') return caching(hash);
      throw b;
    },
    isAborted: () => aborted,
    now: () => clock,
    onAttemptFailed: () => attemptFailures++
  };
  return {
    deps,
    resolved,
    attemptFailures: () => attemptFailures,
    abort: () => {
      aborted = true;
    }
  };
}

const letters = (s: string) => s.split('');
const hashesOf = (opts: StreamOption[]) => opts.map((o) => o.info_hash);

describe('rankOptions', () => {
  const opts = [option('a'), option('b'), option('c', { fileId: 1 }), option('c', { fileId: 2 })];
  const response = (extra: Partial<StreamSearchResponse>): StreamSearchResponse => ({
    options: opts,
    errors: [],
    ...extra
  });

  it.each([
    ['keeps server order without a preference', {}, ['a', 'b', 'c|1', 'c|2']],
    ['moves the preferred hash first', { preferred_info_hash: 'b' }, ['b', 'a', 'c|1', 'c|2']],
    [
      'matches the preferred file inside a pack',
      { preferred_info_hash: 'c', preferred_file_id: 2 },
      ['c|2', 'a', 'b', 'c|1']
    ],
    [
      'ignores a preference missing from the list',
      { preferred_info_hash: 'z' },
      ['a', 'b', 'c|1', 'c|2']
    ]
  ])('%s', (_name, extra, expected) => {
    const ranked = rankOptions(response(extra)).map((o) =>
      o.info_hash === 'c' ? `c|${o.files[0].id}` : o.info_hash
    );
    expect(ranked).toEqual(expected);
  });
});

describe('fallbackCandidates', () => {
  it('keeps server order but sinks failed releases, then uncached ones last', () => {
    const opts = [
      avail('u', 'uncached'),
      option('f', { playback: 'failed' }),
      avail('a', 'ready'),
      avail('b', 'unknown')
    ];
    expect(hashesOf(fallbackCandidates(opts, newWalkState()))).toEqual(['a', 'b', 'f', 'u']);
  });

  it('leaves out what this run already tried', () => {
    const opts = [option('a'), option('b')];
    const state = newWalkState([optionKey(opts[0])]);
    expect(hashesOf(fallbackCandidates(opts, state))).toEqual(['b']);
  });
});

describe('walkStreams', () => {
  it('plays the first option that resolves, trying strictly in order', async () => {
    const h = harness({ a: blocked(), b: 'ready', c: 'ready' });
    const state = newWalkState();
    const result = await walkStreams([option('a'), option('b'), option('c')], state, h.deps);
    expect(result).toMatchObject({ kind: 'ready', session: { session_id: 's-b' } });
    expect(h.resolved).toEqual(['a', 'b']);
    expect(state.failures).toEqual([
      expect.objectContaining({ reason: 'blocked', detail: 'raw blocked', release: 'Release a' })
    ]);
  });

  it.each(['blocked', 'not_video'])(
    'does not charge %s refusals to the attempt budget',
    async (reason) => {
      const opts = letters('abcdefghij').map((x) => option(x));
      const behaviour = Object.fromEntries(
        opts.slice(0, 9).map((o) => [o.info_hash!, refusal(reason)])
      );
      const h = harness(behaviour);
      const result = await walkStreams(opts, newWalkState(), h.deps);
      expect(result).toMatchObject({ kind: 'ready', session: { session_id: 's-j' } });
      expect(h.attemptFailures()).toBe(0);
    }
  );

  it(`caps one walk at ${MAX_WALK_RESOLVES} resolves, refusals included`, async () => {
    const opts = letters('abcdefghijklmnop').map((x) => option(x));
    const h = harness(Object.fromEntries(opts.map((o) => [o.info_hash!, blocked()])));
    const result = await walkStreams(opts, newWalkState(), h.deps);
    expect(result).toEqual({ kind: 'exhausted', stopped: false });
    expect(h.resolved).toHaveLength(MAX_WALK_RESOLVES);
  });

  it(`stops after ${MAX_ATTEMPTS} real failed attempts`, async () => {
    const opts = letters('abcdefghij').map((x) => option(x));
    const h = harness(Object.fromEntries(opts.map((o) => [o.info_hash!, refusal('timeout')])));
    const state = newWalkState();
    const result = await walkStreams(opts, state, h.deps);
    expect(result.kind).toBe('exhausted');
    expect(h.resolved).toEqual(letters('abcdef'));
    expect(h.attemptFailures()).toBe(MAX_ATTEMPTS);
  });

  it.each(['rate_limited', 'auth_failed'])('stops the whole walk on %s', async (reason) => {
    const h = harness({ a: refusal(reason), b: 'ready' });
    const result = await walkStreams([option('a'), option('b')], newWalkState(), h.deps);
    expect(result).toEqual({ kind: 'exhausted', stopped: true });
    expect(h.resolved).toEqual(['a']);
  });

  it('gives up once a walk has run for 30 seconds', async () => {
    const opts = letters('abcdef').map((x) => option(x));
    const h = harness(
      Object.fromEntries(opts.map((o) => [o.info_hash!, refusal('unavailable')])),
      10_000
    );
    await walkStreams(opts, newWalkState(), h.deps);
    expect(h.resolved).toEqual(['a', 'b', 'c']);
  });

  it('passes over a caching option and plays a later ready one', async () => {
    const h = harness({ a: 'caching', b: 'ready' });
    const result = await walkStreams([option('a'), option('b')], newWalkState(), h.deps);
    expect(result).toMatchObject({ kind: 'ready', session: { session_id: 's-b' } });
  });

  it('answers with the best caching option only when nothing was ready', async () => {
    const h = harness({ a: blocked(), b: 'caching', c: 'caching' });
    const result = await walkStreams(
      [option('a'), option('b'), option('c')],
      newWalkState(),
      h.deps
    );
    expect(result).toMatchObject({ kind: 'caching', option: { info_hash: 'b' } });
    expect(h.resolved).toEqual(['a', 'b', 'c']);
  });

  it('resolves no further uncached options once one is caching', async () => {
    const h = harness({ a: blocked(), u1: 'caching', u2: 'caching' });
    const result = await walkStreams(
      [avail('u1', 'uncached'), avail('u2', 'uncached'), avail('a', 'unknown')],
      newWalkState(),
      h.deps
    );
    expect(result).toMatchObject({ kind: 'caching', option: { info_hash: 'u1' } });
    expect(h.resolved).toEqual(['a', 'u1']);
  });

  it('never retries an option already tried in the same run', async () => {
    const h = harness({ a: 'ready', b: 'ready' });
    const state = newWalkState([optionKey(option('a'))]);
    const result = await walkStreams([option('a'), option('b')], state, h.deps);
    expect(result).toMatchObject({ kind: 'ready', session: { session_id: 's-b' } });
    expect(h.resolved).toEqual(['b']);
  });

  it('treats sibling files of one pack and the same hash on another provider as distinct', () => {
    const keys = new Set(
      [option('a'), option('a', { fileId: 2 }), option('a', { provider: 'ad' })].map(optionKey)
    );
    expect(keys.size).toBe(3);
  });

  it('shares the budget across walks of one run', async () => {
    const opts = letters('abcdefgh').map((x) => option(x));
    const h = harness(Object.fromEntries(opts.map((o) => [o.info_hash!, refusal('timeout')])));
    const state = newWalkState();
    await walkStreams(opts.slice(0, 4), state, h.deps);
    await walkStreams(opts, state, h.deps);
    expect(h.resolved).toEqual(letters('abcdef'));
  });

  it('stops quietly when aborted mid-walk', async () => {
    const h = harness({ a: blocked(), b: 'ready' });
    const resolve = h.deps.resolve;
    h.deps.resolve = async (o) => {
      h.abort();
      return resolve(o);
    };
    const result = await walkStreams([option('a'), option('b')], newWalkState(), h.deps);
    expect(result).toEqual({ kind: 'aborted' });
    expect(h.resolved).toEqual(['a']);
  });
});
