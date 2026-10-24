import { describe, expect, it } from 'vitest';
import type Hls from 'hls.js';
import { createLiveHls } from './liveHls';
import { LIVE_CATCH_UP_RATE, LIVE_REBUILD_RATE } from '$lib/streaming/liveLatency';

class FakeHls {
  static Events = { MANIFEST_PARSED: 'mp', FRAG_BUFFERED: 'fb', ERROR: 'err' };
  static ErrorTypes = { MEDIA_ERROR: 'media', NETWORK_ERROR: 'net' };
  static last: FakeHls;
  latency = 1;
  latestLevelDetails = { live: true, targetduration: 1, totalduration: 1800 };
  constructor() {
    FakeHls.last = this;
  }
  on() {}
  off() {}
  loadSource() {}
  attachMedia() {}
  destroy() {}
}

function fakeVideo() {
  const video = new EventTarget() as EventTarget & { paused: boolean; playbackRate: number };
  video.paused = false;
  video.playbackRate = 1;
  Object.assign(video, { play: () => Promise.resolve() });
  return video;
}

describe('createLiveHls rate steering', () => {
  it.each<[string, number, boolean, number]>([
    ['slows to rebuild a drained cushion', 0, false, LIVE_REBUILD_RATE],
    ['catches up after a stall', 3, false, LIVE_CATCH_UP_RATE],
    ['leaves a scrubbed-back viewer alone', 900, false, 1],
    ['does nothing while paused', 0, true, 1]
  ])('%s', (_name, latency, paused, expected) => {
    const video = fakeVideo();
    video.paused = paused;
    const handle = createLiveHls(
      FakeHls as unknown as typeof Hls,
      video as unknown as HTMLVideoElement,
      { onFatal: () => {} }
    );
    handle.load('/live.m3u8');
    FakeHls.last.latency = latency;
    video.dispatchEvent(new Event('timeupdate'));
    expect(video.playbackRate).toBe(expected);
    handle.destroy();
  });

  it('stops steering once destroyed', () => {
    const video = fakeVideo();
    const handle = createLiveHls(
      FakeHls as unknown as typeof Hls,
      video as unknown as HTMLVideoElement,
      { onFatal: () => {} }
    );
    handle.load('/live.m3u8');
    handle.destroy();
    FakeHls.last.latency = 0;
    video.dispatchEvent(new Event('timeupdate'));
    expect(video.playbackRate).toBe(1);
  });
});
