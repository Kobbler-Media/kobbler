import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError, reportStreamOutcome, streamResolve } from './client';

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
