import { describe, expect, it } from 'vitest';
import { ApiError } from '$lib/api/client';
import type {
  StreamOption,
  StreamResolveCachingResponse,
  StreamResolveResponse,
  StreamSearchResponse
} from '$lib/api/client';
import type { PlaybackSessionResponse } from '$lib/types';
import { MAX_ATTEMPTS, newWalkState, optionKey, rankOptions, walkStreams } from './walk';
import type { WalkDeps } from './walk';

function option(hash: string, fileId = 1, provider = 'rd'): StreamOption {
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
    files: [{ id: fileId, path: 'a.mkv', size: 1 }]
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

function caching(hash: string): StreamResolveCachingResponse {
  return { session_id: null, status: 'caching', debrid_provider_id: 'rd', torrent_id: `t-${hash}` };
}

type Behaviour = 'ready' | 'caching' | ApiError | Error;

function harness(
  behaviour: Record<string, Behaviour>,
  cacheResult: Record<string, Behaviour> = {}
) {
  const resolved: string[] = [];
  const waited: string[] = [];
  let aborted = false;
  const deps: WalkDeps = {
    resolve: async (o): Promise<StreamResolveResponse> => {
      const hash = o.info_hash!;
      resolved.push(hash);
      const b = behaviour[hash] ?? 'ready';
      if (b === 'ready') return session(`s-${hash}`);
      if (b === 'caching') return caching(hash);
      throw b;
    },
    waitForCache: async (o) => {
      const hash = o.info_hash!;
      waited.push(hash);
      const b = cacheResult[hash] ?? 'ready';
      if (b === 'ready') return session(`c-${hash}`);
      throw b;
    },
    isAborted: () => aborted
  };
  return {
    deps,
    resolved,
    waited,
    abort: () => {
      aborted = true;
    }
  };
}

const blocked = () =>
  new ApiError(502, 'The debrid provider refused this release.', 'blocked', 'infringing file');

describe('rankOptions', () => {
  const opts = [option('a'), option('b'), option('c', 1), option('c', 2)];
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

describe('walkStreams', () => {
  it('plays the first option that resolves, trying strictly in order', async () => {
    const h = harness({ a: blocked(), b: 'ready', c: 'ready' });
    const state = newWalkState();
    const result = await walkStreams([option('a'), option('b'), option('c')], state, h.deps);
    expect(result).toMatchObject({ kind: 'ready', session: { session_id: 's-b' } });
    expect(h.resolved).toEqual(['a', 'b']);
    expect(state.failures).toEqual([
      expect.objectContaining({
        reason: 'blocked',
        detail: 'infringing file',
        release: 'Release a'
      })
    ]);
  });

  it('skips caching options and plays a later ready one without waiting', async () => {
    const h = harness({ a: 'caching', b: 'ready' });
    const result = await walkStreams([option('a'), option('b')], newWalkState(), h.deps);
    expect(result).toMatchObject({ kind: 'ready', session: { session_id: 's-b' } });
    expect(h.waited).toEqual([]);
  });

  it(`stops after ${MAX_ATTEMPTS} attempts`, async () => {
    const options = 'abcdefghij'.split('').map((x) => option(x));
    const h = harness(Object.fromEntries(options.map((o) => [o.info_hash!, blocked()])));
    const state = newWalkState();
    const result = await walkStreams(options, state, h.deps);
    expect(result.kind).toBe('exhausted');
    expect(h.resolved).toEqual('abcdef'.split(''));
    expect(state.tried.size).toBe(MAX_ATTEMPTS);
  });

  it('waits on the best-ranked caching option when nothing is ready', async () => {
    const h = harness({ a: blocked(), b: 'caching', c: 'caching' });
    const result = await walkStreams(
      [option('a'), option('b'), option('c')],
      newWalkState(),
      h.deps
    );
    expect(result).toMatchObject({ kind: 'ready', session: { session_id: 'c-b' } });
    expect(h.waited).toEqual(['b']);
  });

  it('continues with the remaining options when the caching wait fails', async () => {
    const options = 'abcdefgh'.split('').map((x) => option(x));
    const h = harness(
      { a: 'caching', b: blocked(), c: blocked(), d: blocked(), e: blocked(), f: blocked() },
      { a: new Error('Download failed') }
    );
    const state = newWalkState();
    const result = await walkStreams(options, state, h.deps);
    expect(result).toMatchObject({ kind: 'ready', session: { session_id: 's-g' } });
    expect(h.waited).toEqual(['a']);
    expect(h.resolved).toEqual('abcdefg'.split(''));
    expect(state.failures.map((f) => f.message)).toContain('Download failed');
  });

  it('falls through to the next caching option after a failed wait', async () => {
    const h = harness({ a: 'caching', b: 'caching' }, { a: new Error('nope') });
    const result = await walkStreams([option('a'), option('b')], newWalkState(), h.deps);
    expect(result).toMatchObject({ kind: 'ready', session: { session_id: 'c-b' } });
    expect(h.waited).toEqual(['a', 'b']);
  });

  it('never retries an option already tried in the same run', async () => {
    const h = harness({ a: 'ready', b: 'ready' });
    const state = newWalkState([optionKey(option('a'))]);
    const result = await walkStreams([option('a'), option('b')], state, h.deps);
    expect(result).toMatchObject({ kind: 'ready', session: { session_id: 's-b' } });
    expect(h.resolved).toEqual(['b']);
  });

  it('treats sibling files of one pack and the same hash on another provider as distinct', () => {
    const keys = new Set([option('a', 1), option('a', 2), option('a', 1, 'ad')].map(optionKey));
    expect(keys.size).toBe(3);
  });

  it('continues a run with untried options after a later failure', async () => {
    const h = harness({ a: 'ready', b: 'ready' });
    const state = newWalkState();
    const options = [option('a'), option('b')];
    expect(await walkStreams(options, state, h.deps)).toMatchObject({ kind: 'ready' });
    expect(await walkStreams(options, state, h.deps)).toMatchObject({
      kind: 'ready',
      session: { session_id: 's-b' }
    });
    expect(await walkStreams(options, state, h.deps)).toEqual({ kind: 'exhausted' });
  });

  it('waits on a caching option skipped by an earlier walk of the run', async () => {
    const h = harness({ a: 'caching', b: 'ready' });
    const state = newWalkState();
    const options = [option('a'), option('b')];
    expect(await walkStreams(options, state, h.deps)).toMatchObject({
      session: { session_id: 's-b' }
    });
    expect(await walkStreams(options, state, h.deps)).toMatchObject({
      session: { session_id: 'c-a' }
    });
  });

  it('shares the attempt budget across walks until it is refilled', async () => {
    const options = 'abcdefgh'.split('').map((x) => option(x));
    const h = harness(Object.fromEntries(options.map((o) => [o.info_hash!, blocked()])));
    const state = newWalkState();
    await walkStreams(options.slice(0, 4), state, h.deps);
    await walkStreams(options, state, h.deps);
    expect(h.resolved).toEqual('abcdef'.split(''));
    state.budget = MAX_ATTEMPTS;
    await walkStreams(options, state, h.deps);
    expect(h.resolved).toEqual('abcdefgh'.split(''));
  });

  it('ends without waiting when the caller cannot wait on caching options', async () => {
    const h = harness({ a: 'caching', b: blocked() });
    delete h.deps.waitForCache;
    const state = newWalkState();
    const result = await walkStreams([option('a'), option('b')], state, h.deps);
    expect(result).toEqual({ kind: 'exhausted' });
    expect(state.failures).toEqual([expect.objectContaining({ reason: 'blocked' })]);
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
