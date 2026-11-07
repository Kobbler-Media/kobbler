import { describe, expect, it } from 'vitest';
import { streamPosition } from './streamPosition';

describe('streamPosition', () => {
  it.each([
    ['from the start', 0, 12.5, 12.5],
    ['after a resume that started the playlist at 31s', 31, 0, 31],
    ['after a seek that restarted the playlist at 610.7s', 610.7, 3.2, 613.9]
  ])('places the playhead in the episode %s', (_, seekOffset, mediaTime, expected) => {
    expect(streamPosition(seekOffset, mediaTime)).toBeCloseTo(expected, 6);
  });
});
