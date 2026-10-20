import { describe, expect, it } from 'vitest';
import { failureDetails, summarizeFailures, summarizeNoStreams } from './failure';
import type { StreamFailure } from './walk';

const f = (reason: string | undefined, message = `server says ${reason}`): StreamFailure => ({
  message,
  reason,
  detail: reason ? `raw ${reason}` : undefined,
  release: `Release ${reason}`
});

describe('summarizeFailures', () => {
  it.each([
    [
      'names the count and the dominant reason',
      [f('blocked'), f('timeout'), f('blocked')],
      3,
      'None of the 3 streams we tried would play. Your debrid service refused the release.'
    ],
    [
      'uses the singular for one stream',
      [f('not_video')],
      1,
      "The stream we tried wouldn't play. The release doesn't contain a playable video."
    ],
    [
      'breaks ties by the first failure seen',
      [f('rate_limited'), f('auth_failed')],
      2,
      'None of the 2 streams we tried would play. Your debrid service is limiting requests right now.'
    ],
    [
      'falls back to the server sentence for an unknown reason',
      [f('mystery', 'Something odd happened.')],
      1,
      "The stream we tried wouldn't play. Something odd happened."
    ],
    [
      'falls back to the message when there is no reason',
      [f(undefined, 'Network down')],
      2,
      'None of the 2 streams we tried would play. Network down'
    ],
    [
      'names the player when the browser could not play them',
      [f('wont_play'), f('wont_play')],
      2,
      "None of the 2 streams we tried would play. Your browser couldn't play them."
    ],
    [
      'says only the count when nothing failed loudly',
      [],
      2,
      'None of the 2 streams we tried would play.'
    ]
  ])('%s', (_name, failures, tried, expected) => {
    expect(summarizeFailures(failures, tried)).toBe(expected);
  });
});

describe('summarizeNoStreams', () => {
  it('passes on a search error', () => {
    expect(summarizeNoStreams(['Kobload is offline'])).toBe('Kobload is offline');
  });

  it('explains an empty result', () => {
    expect(summarizeNoStreams([])).toBe("We couldn't find a stream for this title.");
  });
});

describe('failureDetails', () => {
  it('lists each failure with its release and raw detail', () => {
    expect(failureDetails([f('blocked'), f(undefined, 'Network down')])).toEqual([
      'Release blocked: raw blocked',
      'Release undefined: Network down'
    ]);
  });
});
