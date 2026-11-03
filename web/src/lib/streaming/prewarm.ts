import {
  claimStreamPrewarm,
  isStreamCaching,
  releaseStreamPrewarm,
  streamPrewarm,
  touchStreamPrewarm
} from '#lib/api/client.js';
import type { StreamOption, StreamPrewarmParams } from '#lib/api/client.js';
import type { PlaybackSessionResponse } from '#lib/types.js';

const PREWARM_DELAY_MS = 1_000;
// The server lets an untouched prewarm go after about five minutes.
const PREWARM_TOUCH_MS = 90_000;

export interface ClaimedPrewarm {
  session: PlaybackSessionResponse;
  option: StreamOption;
}

const pageVisible = () => typeof document === 'undefined' || document.visibilityState === 'visible';

function keyOf(params: StreamPrewarmParams) {
  return JSON.stringify({ ...params, capabilities: undefined });
}

export function createPrewarm(isVisible: () => boolean = pageVisible) {
  let key: string | null = null;
  let id: string | null = null;
  let option: StreamOption | null = null;
  let delay: ReturnType<typeof setTimeout> | null = null;
  let touch: ReturnType<typeof setInterval> | null = null;
  let warming: Promise<void> | null = null;
  let generation = 0;

  function forget() {
    generation++;
    if (delay) clearTimeout(delay);
    if (touch) clearInterval(touch);
    delay = touch = warming = null;
    key = id = option = null;
  }

  function release(keepalive = false) {
    const held = id;
    forget();
    if (held) releaseStreamPrewarm(held, keepalive).catch(() => {});
  }

  async function warm(gen: number, params: StreamPrewarmParams) {
    try {
      const res = await streamPrewarm(params);
      if (!res) return;
      if (gen !== generation) {
        releaseStreamPrewarm(res.prewarm_id).catch(() => {});
        return;
      }
      id = res.prewarm_id;
      option = res.option ?? null;
      touch = setInterval(() => {
        if (id && isVisible()) touchStreamPrewarm(id).catch(() => {});
      }, PREWARM_TOUCH_MS);
    } catch {
      // Best effort: a press without a prewarm walks the releases as before.
    }
  }

  function start(params: StreamPrewarmParams, delayMs = PREWARM_DELAY_MS) {
    const next = keyOf(params);
    if (next === key) return;
    release();
    key = next;
    const gen = generation;
    delay = setTimeout(() => {
      delay = null;
      warming = warm(gen, params);
    }, delayMs);
  }

  async function claim(params: StreamPrewarmParams): Promise<ClaimedPrewarm | null> {
    if (key !== keyOf(params) || delay) {
      release();
      return null;
    }
    const gen = generation;
    await warming;
    if (gen !== generation || !id || !option) {
      if (gen === generation) release();
      return null;
    }
    const claimed = { id, option };
    forget();
    try {
      const res = await claimStreamPrewarm(claimed.id);
      return isStreamCaching(res) ? null : { session: res, option: claimed.option };
    } catch {
      return null;
    }
  }

  return { start, claim, release };
}
