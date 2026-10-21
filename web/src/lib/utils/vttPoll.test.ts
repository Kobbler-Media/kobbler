import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { pollThumbnailsVtt } from './vttPoll';

function stubFetch(responses: Array<[number, string]>) {
  let i = 0;
  const fetch = vi.fn(async () => {
    const [status, body] = responses[Math.min(i++, responses.length - 1)];
    return new Response(body, { status });
  });
  vi.stubGlobal('fetch', fetch);
  return fetch;
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe('pollThumbnailsVtt', () => {
  it.each<[string, Array<[number, string]>, number, string[]]>([
    [
      'keeps asking while the file is not generated yet',
      [
        [404, ''],
        [404, ''],
        [200, 'A']
      ],
      5,
      ['A']
    ],
    [
      'stops for good once the session is gone',
      [
        [200, 'A'],
        [410, '']
      ],
      2,
      ['A']
    ],
    [
      'stops once the file has settled',
      [
        [200, 'A'],
        [200, 'A'],
        [200, 'A']
      ],
      3,
      ['A']
    ],
    [
      'picks up a file that grows',
      [
        [200, 'A'],
        [200, 'AB'],
        [200, 'AB'],
        [200, 'AB']
      ],
      4,
      ['A', 'AB']
    ]
  ])('%s', async (_name, responses, expectedCalls, seen) => {
    const fetch = stubFetch(responses);
    const vtts: string[] = [];
    pollThumbnailsVtt('/t.vtt', (v) => vtts.push(v));
    await vi.advanceTimersByTimeAsync(10 * 15_000);
    expect(fetch).toHaveBeenCalledTimes(expectedCalls);
    expect(vtts).toEqual(seen);
  });

  it('stops when cancelled', async () => {
    const fetch = stubFetch([[404, '']]);
    const cancel = pollThumbnailsVtt('/t.vtt', () => {});
    await vi.advanceTimersByTimeAsync(0);
    cancel();
    await vi.advanceTimersByTimeAsync(5 * 15_000);
    expect(fetch).toHaveBeenCalledTimes(1);
  });
});
