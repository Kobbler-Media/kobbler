import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  dismissForYou,
  getWatchHistory,
  removeWatchHistory,
  setWatchHistoryExcluded,
  undismissForYou
} from './client';

function stubFetch(status: number, body?: unknown) {
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

const title = { media_type: 'tv' as const, tmdb_id: 1399 };

describe('recommendation edits', () => {
  it.each([
    [
      'dismiss',
      () => dismissForYou(title),
      '/api/v1/admin/discover/for-you/dismiss',
      'POST',
      title
    ],
    [
      'undismiss',
      () => undismissForYou(title),
      '/api/v1/admin/discover/for-you/dismiss',
      'DELETE',
      title
    ],
    [
      'hide',
      () => setWatchHistoryExcluded([title], true),
      '/api/v1/admin/watch-history/exclude',
      'POST',
      { items: [title], excluded: true }
    ],
    [
      'remove',
      () => removeWatchHistory([title]),
      '/api/v1/admin/watch-history/remove',
      'POST',
      { items: [title] }
    ]
  ])('%s sends its body', async (_, call, path, method, body) => {
    const fetch = stubFetch(204);
    await call();
    expect(fetch).toHaveBeenCalledWith(
      path,
      expect.objectContaining({ method, body: JSON.stringify(body) })
    );
  });

  it('filters the history by source and pages it', async () => {
    const fetch = stubFetch(200, { items: [], total: 0 });
    await getWatchHistory({ source: 'trakt', limit: 100, offset: 200 });
    expect(fetch).toHaveBeenCalledWith(
      '/api/v1/admin/watch-history?source=trakt&limit=100&offset=200',
      expect.anything()
    );
  });
});
