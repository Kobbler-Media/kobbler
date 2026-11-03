import type { LivetvChannel, LivetvSession } from '#lib/types.js';
import { createLivetvSession, destroyLivetvSession } from '#lib/api/client.js';
import { detectCapabilities } from '#lib/utils/media.js';
import { preloadHls } from '#lib/components/livetv/player/liveHls.js';

let channel = $state<LivetvChannel | null>(null);

let warmChannelId: string | null = null;
let warmPromise: Promise<LivetvSession> | null = null;
let warmExpiry: ReturnType<typeof setTimeout> | null = null;

// A warm session holds one of four tuner leases. If the gesture that started it
// never turns into a watch, hand the tuner back rather than waiting for the
// server-side lease to lapse.
const WARM_TTL_MS = 20_000;

export function getLivetvPlayerState() {
  return {
    get channel() {
      return channel;
    },
    get isOpen() {
      return channel !== null;
    }
  };
}

function discardWarm() {
  const pending = warmPromise;
  warmChannelId = null;
  warmPromise = null;
  if (warmExpiry) {
    clearTimeout(warmExpiry);
    warmExpiry = null;
  }
  if (!pending) return;
  pending
    .then((session) => destroyLivetvSession(session.session_id).catch(() => {}))
    .catch(() => {});
}

/**
 * Decision 12: the tuner is on the LAN, so the only latency we control is our
 * own. Called on pointerdown — ahead of the click that opens the player — this
 * fetches the hls.js chunk and gets the tune request in flight before the
 * overlay has even mounted. A warm session holds a tuner lease, so it is only
 * ever started from an explicit watch gesture and is released if the user ends
 * up somewhere else.
 */
export function preWarmLivetvChannel(target: LivetvChannel) {
  preloadHls();
  if (warmChannelId === target.id) return;
  discardWarm();
  warmChannelId = target.id;
  const pending = createLivetvSession({
    channel_id: target.id,
    capabilities: detectCapabilities()
  });
  warmPromise = pending;
  pending.catch(() => {});
  warmExpiry = setTimeout(() => {
    if (warmChannelId === target.id) discardWarm();
  }, WARM_TTL_MS);
}

export function claimLivetvSession(channelId: string): Promise<LivetvSession> {
  if (warmChannelId === channelId && warmPromise) {
    const pending = warmPromise;
    warmChannelId = null;
    warmPromise = null;
    if (warmExpiry) {
      clearTimeout(warmExpiry);
      warmExpiry = null;
    }
    return pending;
  }
  discardWarm();
  return createLivetvSession({ channel_id: channelId, capabilities: detectCapabilities() });
}

export function openLivetvChannel(target: LivetvChannel) {
  channel = target;
}

export function setLivetvPlayerChannel(target: LivetvChannel) {
  channel = target;
}

export function closeLivetvPlayer() {
  discardWarm();
  channel = null;
}
