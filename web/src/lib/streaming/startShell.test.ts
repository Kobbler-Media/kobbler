import { describe, expect, it } from 'vitest';
import { showsStartShell } from './startShell';
import type { StartView } from './startShell';

const starting = (v: Partial<StartView>): StartView => ({
  debrid: false,
  live: false,
  sessionReady: true,
  streamStarted: false,
  startHeld: false,
  failed: false,
  casting: false,
  ...v
});

describe('showsStartShell', () => {
  it.each([
    ['waiting for a session', { sessionReady: false }],
    ['waiting for the first frame', {}]
  ])('never shows a library file the stream shell while %s', (_, v) => {
    expect(showsStartShell(starting(v))).toBe(false);
  });

  it.each([
    ['waiting for a session', { sessionReady: false }],
    ['waiting for the first frame', {}]
  ])('shows a debrid stream the stream shell while %s', (_, v) => {
    expect(showsStartShell(starting({ debrid: true, ...v }))).toBe(true);
  });

  it.each([
    ['it has started', { streamStarted: true }],
    ['the viewer is holding it', { startHeld: true }],
    ['it failed', { failed: true }],
    ['it is casting', { casting: true }],
    ['it is live', { live: true }]
  ])('drops the debrid stream shell once %s', (_, v) => {
    expect(showsStartShell(starting({ debrid: true, ...v }))).toBe(false);
  });
});
