import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  ApiError,
  getPlaybackSessionStatus,
  reportBadRelease,
  reportStreamOutcome,
  streamPause,
  streamResolve
} from './client';

function stubFetch(status: number, body: unknown) {
  const fetch = vi.fn(
    async () =>
      new Response(body === undefined ? null : JSON.stringify(body), {
        status,
        headers: body === undefined ? {} : { 'content-type': 'application/json' }
      })
  );
  vi.stubGlobal('fetch', fetch);
  return fetch;
}

afterEach(() => vi.unstubAllGlobals());

describe('stream API errors', () => {
  it('carries the server sentence, reason and raw detail', async () => {
    stubFetch(502, {
      error: 'The debrid provider refused this release. Try a different source.',
      reason: 'blocked',
      detail: 'infringing_file'
    });
    const err = await streamResolve({ debrid_provider_id: 'rd' }).catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err).toMatchObject({
      status: 502,
      message: 'The debrid provider refused this release. Try a different source.',
      reason: 'blocked',
      detail: 'infringing_file'
    });
  });
});

describe('reportStreamOutcome', () => {
  it.each([[{ ok: true as const }], [{ ok: false as const, reason: 'media_error' }]])(
    'posts %j to the session outcome',
    async (outcome) => {
      const fetch = stubFetch(204, undefined);
      await reportStreamOutcome('abc', outcome);
      expect(fetch).toHaveBeenCalledWith(
        '/api/v1/admin/stream/abc/outcome',
        expect.objectContaining({ method: 'POST', body: JSON.stringify(outcome) })
      );
    }
  );
});

describe('streamPause', () => {
  it('asks about one provider and returns its pause', async () => {
    const pause = { reason: 'adds_paused', seconds: 27 };
    const fetch = stubFetch(200, { pause });
    expect(await streamPause('p1')).toEqual(pause);
    expect(fetch).toHaveBeenCalledWith(
      '/api/v1/admin/stream/pause?provider_id=p1',
      expect.anything()
    );
  });

  it('reads no pause as null', async () => {
    stubFetch(200, { pause: null });
    expect(await streamPause('p')).toBeNull();
  });

  it.each([404, 500])('reads a %i as no pause', async (status) => {
    stubFetch(status, { error: 'nope' });
    expect(await streamPause('p')).toBeNull();
  });

  it('reads an unanswered request as no pause', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => Promise.reject(new TypeError('offline')))
    );
    expect(await streamPause('p')).toBeNull();
  });
});

describe('reportBadRelease', () => {
  const release = {
    debrid_provider_id: 'p',
    info_hash: 'h',
    files: [{ id: 3, path: 'a', size: 1 }]
  };

  it.each([true, false])('posts the release file with bad=%s', async (bad) => {
    const fetch = stubFetch(204, undefined);
    await reportBadRelease(release, bad);
    expect(fetch).toHaveBeenCalledWith(
      '/api/v1/admin/stream/bad',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({ debrid_provider_id: 'p', info_hash: 'h', file_id: 3, bad })
      })
    );
  });

  it('sends a null file id for a release without files', async () => {
    const fetch = stubFetch(204, undefined);
    await reportBadRelease({ ...release, files: [] }, true);
    expect(fetch).toHaveBeenCalledWith(
      '/api/v1/admin/stream/bad',
      expect.objectContaining({
        body: JSON.stringify({ debrid_provider_id: 'p', info_hash: 'h', file_id: null, bad: true })
      })
    );
  });
});

describe('getPlaybackSessionStatus', () => {
  it('returns the status with its feed', async () => {
    const body = {
      session_id: 's',
      state: 'ready',
      method: 'direct_play',
      is_live: false,
      feed: { requests_open: 1, bytes_served: 10, idle_ms: 830, source: 'proxy' }
    };
    stubFetch(200, body);
    expect(await getPlaybackSessionStatus('s')).toEqual({ kind: 'status', status: body });
  });

  it.each([404, 410])('reads %i as a session that is gone', async (status) => {
    stubFetch(status, { error: 'gone' });
    expect(await getPlaybackSessionStatus('s')).toEqual({ kind: 'missing' });
  });

  it('reads a request that never got an answer as no news', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => Promise.reject(new TypeError('offline')))
    );
    expect(await getPlaybackSessionStatus('s')).toEqual({ kind: 'unknown' });
  });
});
