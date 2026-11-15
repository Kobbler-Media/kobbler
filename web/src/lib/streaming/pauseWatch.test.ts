import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { StreamPause } from '#lib/api/client.js';
import { watchPause } from './pauseWatch';

function harness(answers: Array<StreamPause | null>) {
  let finish: (value: string) => void = () => {};
  const pending = new Promise<string>((r) => (finish = r));
  const check = vi.fn(async () => answers.shift() ?? null);
  const notes: Array<string | null> = [];
  const controller = new AbortController();
  const watched = watchPause(pending, check, (note) => notes.push(note), controller.signal);
  return { finish, check, notes, watched, controller };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe('watchPause', () => {
  it('says nothing about a resolve that is only slow', async () => {
    const h = harness([]);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(h.check).toHaveBeenCalledTimes(20);
    expect(h.notes).toEqual([]);
  });

  it('does not ask about a resolve that answers quickly', async () => {
    const h = harness([]);
    await vi.advanceTimersByTimeAsync(1_499);
    h.finish('done');
    expect(await h.watched).toBe('done');
    await vi.advanceTimersByTimeAsync(10_000);
    expect(h.check).not.toHaveBeenCalled();
  });

  it('asks after 1.5s, then every 3s, and keeps the countdown current', async () => {
    const h = harness([
      { reason: 'adds_paused', seconds: 27 },
      { reason: 'adds_paused', seconds: 24 }
    ]);
    await vi.advanceTimersByTimeAsync(1_499);
    expect(h.check).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(h.notes).toEqual([
      'Your debrid service is pausing new requests on your account. Trying again in about 27s…'
    ]);
    await vi.advanceTimersByTimeAsync(2_999);
    expect(h.check).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(h.notes.at(-1)).toBe(
      'Your debrid service is pausing new requests on your account. Trying again in about 24s…'
    );
  });

  it.each([
    ['rate_limited', 'Your debrid service is limiting requests right now.'],
    ['something_new', 'Your debrid service is making requests wait.']
  ])('words a %s pause', async (reason, lead) => {
    const h = harness([{ reason, seconds: 9 }]);
    await vi.advanceTimersByTimeAsync(1_500);
    expect(h.notes).toEqual([`${lead} Trying again in about 9s…`]);
  });

  it('takes the note down when the pause lifts or the resolve answers', async () => {
    const lifted = harness([{ reason: 'rate_limited', seconds: 3 }, null]);
    await vi.advanceTimersByTimeAsync(4_500);
    expect(lifted.notes).toEqual([expect.any(String), null]);

    const answered = harness([{ reason: 'rate_limited', seconds: 3 }]);
    await vi.advanceTimersByTimeAsync(1_500);
    answered.finish('done');
    await answered.watched;
    expect(answered.notes).toEqual([expect.any(String), null]);
  });

  it('stops asking once the resolve answers or the attempt is abandoned', async () => {
    const answered = harness([]);
    await vi.advanceTimersByTimeAsync(1_500);
    answered.finish('done');
    await answered.watched;
    const abandoned = harness([]);
    await vi.advanceTimersByTimeAsync(1_500);
    abandoned.controller.abort();
    await vi.advanceTimersByTimeAsync(30_000);
    expect(answered.check).toHaveBeenCalledTimes(1);
    expect(abandoned.check).toHaveBeenCalledTimes(1);
    expect(abandoned.notes).toEqual([]);
  });

  it('passes a refusal through', async () => {
    const check = vi.fn(async () => null);
    const watched = watchPause(
      Promise.reject(new Error('no')),
      check,
      () => {},
      new AbortController().signal
    );
    await expect(watched).rejects.toThrow('no');
  });
});
