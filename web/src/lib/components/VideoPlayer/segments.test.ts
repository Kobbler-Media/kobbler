import { describe, expect, it } from 'vitest';
import type { SkipSegment } from '#lib/types.js';
import { creditsSkipTarget } from './segments';

const seg = (
  segment_type: SkipSegment['segment_type'],
  start: number,
  end: number
): SkipSegment => ({
  segment_type,
  start_seconds: start,
  end_seconds: end,
  source: 'audio+visual'
});

const nameCards = seg('credits', 2922, 3006);
const crawl = seg('credits', 3135, 3311);
const postCredits = seg('post_credits_scene', 3250, 3311);

describe('creditsSkipTarget', () => {
  it.each([
    [
      'name cards ahead of a mid-credits scene land on the scene',
      nameCards,
      [nameCards, crawl],
      3006
    ],
    [
      'name cards land on the scene even when a post-credits scene follows the crawl',
      nameCards,
      [nameCards, crawl, postCredits],
      3006
    ],
    [
      'the crawl lands on its post-credits scene',
      seg('credits', 3135, 3250),
      [nameCards, seg('credits', 3135, 3250), postCredits],
      3250
    ],
    [
      'credits that stop short of the end land on what follows',
      seg('credits', 3000, 3200),
      [seg('credits', 3000, 3200)],
      3200
    ],
    ['credits that run to the end finish the title', crawl, [nameCards, crawl], 'end']
  ] as const)('%s', (_, active, segments, expected) => {
    expect(creditsSkipTarget(active, [...segments], 3311)).toBe(expected);
  });
});
