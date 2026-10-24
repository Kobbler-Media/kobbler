// hls.js only steers latency in lowLatencyMode, and even then only downwards.
// An IPTV source that delivers a hair slower than realtime, or hiccups without
// bursting back, eats the holdback a little at a time; once it is gone every
// segment lands just in time and playback stalls once per segment. Nudging the
// rate both ways keeps the holdback where liveSyncDuration put it.
export const LIVE_REBUILD_RATE = 0.95;
export const LIVE_CATCH_UP_RATE = 1.05;

// hls.js measures latency against the playlist edge, which jumps a whole
// segment at each refresh, so the reading is a sawtooth of +/- half a segment.
// A band narrower than that would flip the rate every segment.
// A player that keeps its whole buffer for scrubbing back sets followLimitS: past it,
// the viewer chose to be behind and is left there.
export function liveSyncRate({
  latency,
  targetLatency,
  targetDurationS,
  currentRate,
  followLimitS = Infinity
}: {
  latency: number | null;
  targetLatency: number | null;
  targetDurationS: number;
  currentRate: number;
  followLimitS?: number;
}): number {
  if (latency === null || targetLatency === null || latency > followLimitS) return 1;
  // The floor shrinks with the target, or a 1s tuner holdback could drain to nothing
  // without ever leaving the band.
  const enter = Math.max(Math.min(1, targetLatency / 2), targetDurationS * 0.75);
  const exit = enter / 4;
  const drift = latency - targetLatency;
  if (currentRate === LIVE_REBUILD_RATE && drift < -exit) return LIVE_REBUILD_RATE;
  if (currentRate === LIVE_CATCH_UP_RATE && drift > exit) return LIVE_CATCH_UP_RATE;
  if (drift < -enter) return LIVE_REBUILD_RATE;
  if (drift > enter) return LIVE_CATCH_UP_RATE;
  return 1;
}

// The server's sliding window is only a few segments long, and hls.js grows its
// own target on every stall, so steering to its target would park the playhead
// at the window start where the next segment is about to be deleted.
export function liveTargetLatency(
  configuredS: number,
  windowS: number,
  targetDurationS: number
): number {
  return Math.max(targetDurationS, Math.min(configuredS, windowS - 2 * targetDurationS));
}

// How far behind the edge a stall can plausibly leave a timeshift viewer; anything
// further is a scrub-back.
export function tunerFollowLimit(targetLatencyS: number, targetDurationS: number): number {
  return targetLatencyS + 4 * targetDurationS;
}
