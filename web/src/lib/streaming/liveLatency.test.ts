import { describe, expect, it } from 'vitest';
import {
  LIVE_CATCH_UP_RATE,
  LIVE_REBUILD_RATE,
  liveSyncRate,
  liveTargetLatency,
  tunerFollowLimit
} from './liveLatency';

const TARGET = 20;

describe('liveSyncRate', () => {
  it.each<[string, number | null, number, number, number]>([
    ['holds 1x on target', 20, 1, 1, 1],
    ['holds 1x inside the dead band', 19.2, 1, 1, 1],
    ['slows down once the holdback has drained', 18, 1, 1, LIVE_REBUILD_RATE],
    [
      'keeps slowing until latency is back near target',
      19.2,
      1,
      LIVE_REBUILD_RATE,
      LIVE_REBUILD_RATE
    ],
    ['returns to 1x once rebuilt', 19.9, 1, LIVE_REBUILD_RATE, 1],
    ['speeds up when stalls pushed playback back', 22, 1, 1, LIVE_CATCH_UP_RATE],
    ['keeps catching up until near target', 20.6, 1, LIVE_CATCH_UP_RATE, LIVE_CATCH_UP_RATE],
    ['returns to 1x once caught up', 20.1, 1, LIVE_CATCH_UP_RATE, 1],
    ['rides out the per-segment sawtooth of 4s segments', 18, 4, 1, 1],
    ['still rebuilds a real drain with 4s segments', 16.5, 4, 1, LIVE_REBUILD_RATE],
    ['leaves the rate alone without a latency reading', null, 1, 1, 1]
  ])('%s', (_name, latency, targetDurationS, currentRate, expected) => {
    expect(liveSyncRate({ latency, targetLatency: TARGET, targetDurationS, currentRate })).toBe(
      expected
    );
  });

  it('does not act without a target latency', () => {
    expect(
      liveSyncRate({ latency: 5, targetLatency: null, targetDurationS: 4, currentRate: 1 })
    ).toBe(1);
  });
});

// The tuner holds back one 1s segment and keeps its whole timeshift buffer, so a viewer
// far behind the edge is one who scrubbed there, not one a stall left behind.
describe('liveSyncRate on a timeshifted tuner', () => {
  const FOLLOW = tunerFollowLimit(1, 1);
  it.each<[string, number, number, number]>([
    ['rebuilds a cushion the tuner ate', 0, 1, LIVE_REBUILD_RATE],
    ['catches up after stalls left it a few seconds behind', 3, 1, LIVE_CATCH_UP_RATE],
    ['leaves a viewer scrubbed back into the buffer where they are', 600, 1, 1],
    ['stops catching up once the viewer scrubs back', 600, LIVE_CATCH_UP_RATE, 1],
    ['holds 1x at the holdback', 1, 1, 1]
  ])('%s', (_name, latency, currentRate, expected) => {
    expect(
      liveSyncRate({
        latency,
        targetLatency: 1,
        targetDurationS: 1,
        currentRate,
        followLimitS: FOLLOW
      })
    ).toBe(expected);
  });
});

describe('liveTargetLatency', () => {
  it.each<[string, number, number, number, number]>([
    ['keeps the configured holdback when the window has room', 20, 60, 10, 20],
    ['stays two segments clear of the window start', 20, 24, 4, 16],
    ['never asks for less than one segment', 20, 6, 4, 4]
  ])('%s', (_name, configured, windowS, targetDurationS, expected) => {
    expect(liveTargetLatency(configured, windowS, targetDurationS)).toBe(expected);
  });
});
