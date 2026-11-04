import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import { describe, expect, it } from 'vitest';

const source = readFileSync(
  fileURLToPath(new URL('../../../static/cast-receiver/receiver.js', import.meta.url)),
  'utf8'
);

interface Media {
  contentUrl?: string;
  contentId?: string;
  contentType?: string;
  streamType?: string;
  duration?: number;
  customData?: { kobblerSession?: unknown };
}
interface LoadRequest {
  media: Media;
  currentTime?: number;
}
interface MediaStatus {
  currentTime?: number;
  media?: Media;
  liveSeekableRange?: unknown;
}
interface Message {
  type: string;
  payload: { currentTime: number; duration: number; playing: boolean };
}
interface Tracks {
  subtitles: Array<{ id: number; language: string | null }>;
  activeSubtitleId: number | null;
  audioTracks: Array<{ streamIndex: number }>;
  activeAudioIndex: number | null;
}
interface FakeElement {
  classes: Set<string>;
  textContent: string;
  style: Record<string, string>;
  dataset: Record<string, string>;
  click(): void;
}
type Handler = (data: unknown) => unknown;

function enumOf(): Record<string, string> {
  return new Proxy({}, { get: (_, key) => String(key) });
}

function receiver(
  replies: Record<string, unknown> = {},
  displays: (mime: string, codecs?: string, width?: number, height?: number) => boolean = () =>
    true,
  userAgent = 'Mozilla/5.0 (X11; Linux armv7l) CrKey/1.56.500000'
) {
  const interceptors: Record<string, Handler> = {};
  const listeners: Record<string, Handler[]> = {};
  const customListeners: Handler[] = [];
  const sent: Message[] = [];
  const fetches: string[] = [];
  const methods: Record<string, string> = {};
  const intervals: Array<() => void> = [];
  const loads: LoadRequest[] = [];
  const seeks: number[] = [];
  let stops = 0;
  const activeTextIds: number[][] = [];
  const posts: Record<string, unknown> = {};
  const keyListeners: Array<(e: { key: string; target: unknown; preventDefault(): void }) => void> =
    [];
  const contextListeners: Record<string, Handler[]> = {};
  let current: Media | null = null;
  const video = { currentTime: 0, duration: Infinity, paused: false, volume: 1 };

  const playerManager = {
    setMessageInterceptor: (type: string, fn: Handler) => (interceptors[type] = fn),
    addEventListener: (type: string, fn: Handler) => (listeners[type] ??= []).push(fn),
    getPlayerState: () => (video.paused ? 'PAUSED' : 'PLAYING'),
    getCurrentTimeSec: () => video.currentTime,
    getDurationSec: () => video.duration,
    getMediaInformation: () => current,
    getQueueManager: () => null,
    setPlaybackRate: () => {},
    setMediaElement: () => {},
    getTextTracksManager: () => ({ setActiveByIds: (ids: number[]) => activeTextIds.push(ids) }),
    pause: () => (video.paused = true),
    play: () => (video.paused = false),
    load: async (request: LoadRequest) => {
      loads.push(request);
    },
    seek: (t: number) => seeks.push(t),
    stop: () => stops++
  };
  const context = {
    addCustomMessageListener: (_ns: string, fn: Handler) => customListeners.push(fn),
    getPlayerManager: () => playerManager,
    sendCustomMessage: (_ns: string, _to: unknown, msg: Message) => sent.push(msg),
    addEventListener: (type: string, fn: Handler) => (contextListeners[type] ??= []).push(fn),
    canDisplayType: displays,
    getSenders: () => [{}],
    stop: () => {},
    start: () => {}
  };
  class Bag {}
  const clicks: Array<() => void> = [];
  const elements: Record<string, FakeElement> = {};
  const element = (id?: string) => {
    const classes = new Set<string>();
    const el = {
      classes,
      classList: {
        add: (c: string) => classes.add(c),
        remove: (c: string) => classes.delete(c),
        toggle: (c: string, on: boolean) => (on ? classes.add(c) : classes.delete(c))
      },
      setAttribute() {},
      addEventListener(type: string, fn: () => void) {
        if (id === 'skip-btn' && type === 'click') clicks.push(fn);
      },
      click() {
        clicks.forEach((fn) => fn());
      },
      focus() {},
      style: {} as Record<string, string>,
      dataset: {} as Record<string, string>,
      appendChild() {},
      querySelector: () => null,
      textContent: ''
    };
    if (id) elements[id] = el;
    return el;
  };
  const byId = (id: string) => elements[id] ?? element(id);
  const skipButton = byId('skip-btn');
  const sandbox: Record<string, unknown> = {
    console,
    navigator: { userAgent },
    setTimeout: () => 0,
    clearTimeout: () => {},
    setInterval: (fn: () => void) => intervals.push(fn),
    clearInterval: () => {},
    fetch: async (url: string, init?: { method?: string; body?: string }) => {
      fetches.push(url);
      const path = url.split('?')[0];
      methods[path] = init?.method ?? 'GET';
      if (init?.body) posts[path] = JSON.parse(init.body);
      const reply = Object.entries(replies).find(([suffix]) => path.endsWith(suffix))?.[1];
      return { ok: reply !== undefined, json: async () => reply ?? {} };
    },
    EventSource: class {
      close() {}
    },
    document: {
      getElementById: byId,
      createElement: () => element(),
      querySelector: () => null,
      addEventListener: (type: string, fn: (typeof keyListeners)[number]) => {
        if (type === 'keydown') keyListeners.push(fn);
      }
    },
    cast: {
      framework: {
        CastReceiverContext: { getInstance: () => context },
        CastReceiverOptions: Bag,
        events: { EventType: enumOf(), EndedReason: enumOf() },
        system: { EventType: enumOf(), MessageType: enumOf() },
        messages: {
          MessageType: enumOf(),
          StreamType: enumOf(),
          PlayerState: enumOf(),
          HlsSegmentFormat: enumOf(),
          TrackType: enumOf(),
          TextTrackType: enumOf(),
          MediaInformation: Bag,
          GenericMediaMetadata: Bag,
          MusicTrackMediaMetadata: Bag,
          LoadRequestData: Bag,
          EditTracksInfoRequestData: Bag,
          Image: Bag,
          Track: Bag
        }
      }
    }
  };
  vm.runInNewContext(source, sandbox);

  const session = (overrides: Record<string, unknown> = {}) => ({
    baseUrl: 'https://kob',
    token: 'tok',
    sessionId: 'sess-1',
    mediaId: 'ep-1',
    mediaType: 'episode',
    playlistUrl: 'https://kob/api/v1/admin/playback/sessions/sess-1/master.m3u8',
    directUrl: null,
    method: 'hls',
    duration: 1465,
    subtitles: [],
    audioTracks: [],
    chapters: [],
    skipSegments: [],
    metadata: { title: 'Episode' },
    ...overrides
  });

  return {
    video,
    sent,
    posts,
    activeTextIds,
    el: (id: string) => elements[id],
    press: (key: string) =>
      keyListeners.forEach((fn) => fn({ key, target: null, preventDefault() {} })),
    connect: () => contextListeners.SENDER_CONNECTED?.forEach((fn) => fn({})),
    last: <T>(type: string) => sent.findLast((m) => m.type === type)?.payload as T | undefined,
    fetches,
    methods,
    loads,
    session,
    seek: (currentTime: number) => interceptors.SEEK({ currentTime }),
    seeks,
    stops: () => stops,
    load: (kobblerSession = session()) => {
      const request = interceptors.LOAD({
        media: {
          contentUrl: 'https://kob/x',
          contentId: 'https://kob/x',
          customData: { kobblerSession }
        }
      }) as LoadRequest;
      current = request.media;
      return request;
    },
    skip: (to: number) => {
      skipButton.dataset.skipTo = String(to);
      skipButton.click();
    },
    status: (status: MediaStatus) => interceptors.MEDIA_STATUS(status) as MediaStatus,
    finish: (endedReason: string) => listeners.MEDIA_FINISHED.forEach((fn) => fn({ endedReason })),
    tick: () => intervals.forEach((fn) => fn()),
    emit: (type: string) => listeners[type]?.forEach((fn) => fn({})),
    ask: (type: string, payload?: unknown) =>
      customListeners.forEach((fn) => fn({ data: { type, payload } }))
  };
}

describe('cast receiver', () => {
  it('treats a replaced title as interrupted, not finished', async () => {
    const r = receiver();
    r.load();
    r.finish('INTERRUPTED');
    await Promise.resolve();

    expect(r.sent.filter((m) => m.type === 'MEDIA_FINISHED')).toEqual([]);
    expect(r.fetches.filter((u) => u.includes('/watch-state') || u.includes('/next'))).toEqual([]);
  });

  it('moves on to the next episode when the stream really ends', async () => {
    const r = receiver();
    r.load();
    r.finish('END_OF_STREAM');
    await new Promise((resolve) => setImmediate(resolve));

    expect(r.fetches.some((u) => u.includes('/tv/episodes/ep-1/next'))).toBe(true);
  });

  it('describes a title that is still transcoding as seekable, at its full length', () => {
    const r = receiver();
    const request = r.load();

    expect(request.media.streamType).toBe('BUFFERED');
    expect(request.media.duration).toBe(1465);

    const status = r.status({
      media: { streamType: 'LIVE', duration: -1 },
      liveSeekableRange: { start: 0, end: 120, isMovingWindow: false, isLiveDone: false }
    });
    expect(status.media?.streamType).toBe('BUFFERED');
    expect(status.media?.duration).toBe(1465);
    expect(status.liveSeekableRange).toBeUndefined();
  });

  it('reports the full length to the sender while the playlist is still growing', () => {
    const r = receiver();
    r.load();
    r.video.currentTime = 300;
    r.video.duration = Infinity;
    r.tick();

    const update = r.sent.find((m) => m.type === 'POSITION_UPDATE');
    expect(update?.payload.duration).toBe(1465);
  });

  it('reports where it is although the media element sits in the player shadow DOM', () => {
    const r = receiver();
    r.load();
    r.video.currentTime = 915;
    r.tick();

    const update = r.sent.find((m) => m.type === 'POSITION_UPDATE');
    expect(update?.payload).toMatchObject({ currentTime: 915, playing: true });
    expect(r.methods['https://kob/api/v1/admin/tv/episodes/ep-1/watch-state/position']).toBe(
      'PATCH'
    );
  });

  it('tells the sender at once when it is paused, and saves nothing while it is', () => {
    const r = receiver();
    r.load();
    r.video.currentTime = 1000;
    r.video.paused = true;
    r.emit('PAUSE');

    expect(r.sent.filter((m) => m.type === 'POSITION_UPDATE').map((m) => m.payload)).toEqual([
      { currentTime: 1000, duration: 1465, playing: false }
    ]);
    r.tick();
    expect(r.fetches.filter((u) => u.includes('/watch-state/position'))).toEqual([]);
  });

  it('speaks title time for a transcode that began part way in', () => {
    const r = receiver();
    r.load(r.session({ startOffset: 1380 }));
    r.video.currentTime = 10;
    r.tick();

    expect(r.sent.find((m) => m.type === 'POSITION_UPDATE')?.payload.currentTime).toBe(1390);
    expect(r.fetches.some((u) => u.includes('/watch-state/position'))).toBe(true);
  });

  it('follows a seek onto the playlist the server rebuilt from there', async () => {
    const r = receiver({ '/seek': { seek_generation: 2, start_time: 594, sub_segment_offset: 6 } });
    r.load(r.session({ startOffset: 1380 }));

    expect(await r.seek(600)).toBeNull();
    expect(r.loads.at(-1)?.currentTime).toBe(6);
    expect(r.loads.at(-1)?.media.contentUrl).toContain('g=2');
    expect(r.loads.at(-1)?.media.customData?.kobblerSession).toBeUndefined();

    r.video.currentTime = 6;
    r.tick();
    expect(r.sent.findLast((m) => m.type === 'POSITION_UPDATE')?.payload.currentTime).toBe(600);
  });

  it('skips on a transcode through the server, in title time', async () => {
    const r = receiver({ '/seek': { seek_generation: 1, start_time: 90, sub_segment_offset: 0 } });
    r.load(r.session({ startOffset: 0 }));
    r.skip(90);
    await new Promise((resolve) => setImmediate(resolve));

    expect(r.seeks).toEqual([]);
    expect(r.loads.at(-1)?.media.contentUrl).toContain('g=1');
  });

  it('reports title time in the media status every sender reads', () => {
    const r = receiver();
    r.load(r.session({ startOffset: 1380 }));

    expect(r.status({ currentTime: 10 }).currentTime).toBe(1390);
  });

  it('describes the episode it moved on to when a sender joins later', async () => {
    const r = receiver({
      '/next': {
        episode_id: 'ep-2',
        title: 'Volcano',
        season_identifier: '1',
        episode_identifier: '2',
        show_title: 'South Park',
        file_id: 'file-2'
      },
      '/playback/sessions': {
        session_id: 'sess-2',
        direct_url: '/direct/2',
        duration_seconds: 1400
      }
    });
    r.load();
    r.finish('END_OF_STREAM');
    await new Promise((resolve) => setTimeout(resolve, 0));
    r.ask('STATUS_REQUEST');

    const status = r.sent.findLast((m) => m.type === 'STATUS_RESPONSE') as unknown as {
      payload: { mediaId: string; metadata: { title: string; subtitle: string } };
    };
    expect(status.payload.mediaId).toBe('ep-2');
    expect(status.payload.metadata).toMatchObject({
      title: 'South Park',
      subtitle: 'S1E2 - Volcano'
    });
  });

  it('tells a sender what the device itself says it decodes, and asks for that next', async () => {
    const r = receiver(
      {
        '/next': { episode_id: 'ep-2', show_title: 'South Park', file_id: 'file-2' },
        '/playback/sessions': { session_id: 'sess-2', direct_url: '/direct/2' }
      },
      (_mime, codecs = '', _w, height = 0) =>
        height <= 1080 && !codecs.startsWith('vp09') && codecs !== 'ec-3'
    );
    r.connect();

    const caps = {
      video_codecs: ['h264', 'hevc'],
      audio_codecs: ['aac', 'ac3'],
      containers: ['mp4'],
      max_height: 1080
    };
    expect(r.last('CAPABILITIES')).toEqual(caps);

    r.load();
    r.finish('END_OF_STREAM');
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(
      (r.posts['https://kob/api/v1/admin/playback/sessions'] as { capabilities: unknown })
        .capabilities
    ).toEqual(caps);
  });

  const withTracks = (r: ReturnType<typeof receiver>) =>
    r.session({
      startOffset: 1380,
      subtitles: [
        { url: '/api/s/en.vtt', format: 'vtt', language: 'eng', is_default: false },
        { url: '/api/s/signs.ass', format: 'ass', language: 'eng', is_default: false },
        { url: '/api/s/fr.vtt', format: 'vtt', language: 'fre', is_default: true }
      ],
      audioTracks: [
        { stream_index: 1, language: 'jpn', is_default: true },
        { stream_index: 2, language: 'eng', is_default: false }
      ]
    });

  it('offers the text tracks it can show, timed to where the transcode began', () => {
    const r = receiver();
    const request = r.load(withTracks(r)) as LoadRequest & {
      media: { tracks: Array<{ trackContentId: string }> };
      activeTrackIds: number[];
    };

    expect(request.media.tracks.map((t) => t.trackContentId)).toEqual([
      'https://kob/api/s/en.vtt?offset=1380&token=tok',
      'https://kob/api/s/fr.vtt?offset=1380&token=tok'
    ]);
    expect(request.activeTrackIds).toEqual([2]);

    r.ask('TRACKS_REQUEST');
    expect(r.last<Tracks>('TRACKS')).toMatchObject({
      subtitles: [
        { id: 1, language: 'eng' },
        { id: 2, language: 'fre' }
      ],
      activeSubtitleId: 2,
      audioTracks: [{ streamIndex: 1 }, { streamIndex: 2 }],
      activeAudioIndex: 1
    });

    r.ask('SUBTITLE_CHANGE', { trackId: null });
    expect(r.activeTextIds).toEqual([[]]);
    expect(r.last<Tracks>('TRACKS')?.activeSubtitleId).toBeNull();
  });

  it('switches the audio from where it is and keeps speaking title time', async () => {
    const r = receiver({
      '/switch-audio': {
        method: 'video_transcode',
        playlist_url: '/api/v1/admin/playback/sessions/sess-1/master.m3u8',
        start_time: 1400
      }
    });
    r.load(withTracks(r));
    r.video.currentTime = 20;
    r.ask('AUDIO_SWITCH', { streamIndex: 2 });
    await new Promise((resolve) => setImmediate(resolve));

    expect(r.posts['https://kob/api/v1/admin/playback/sessions/sess-1/switch-audio']).toEqual({
      audio_stream_index: 2,
      resume_time: 1400
    });
    expect(r.loads.at(-1)?.currentTime).toBe(0);
    expect(r.loads.at(-1)?.media.contentUrl).toContain('/sess-1/master.m3u8?g=1');
    expect(r.last<Tracks>('TRACKS')?.activeAudioIndex).toBe(2);

    r.video.currentTime = 5;
    r.tick();
    expect(r.last<Message['payload']>('POSITION_UPDATE')?.currentTime).toBe(1405);
  });

  it('tells senders once when a skippable segment starts and once when it ends', () => {
    const r = receiver();
    r.load(
      r.session({ skipSegments: [{ segment_type: 'intro', start_seconds: 60, end_seconds: 90 }] })
    );
    const skips = () => r.sent.filter((m) => m.type === 'SKIP_SEGMENT').map((m) => m.payload);

    r.video.currentTime = 70;
    r.tick();
    r.tick();
    r.video.currentTime = 95;
    r.tick();

    expect(skips()).toEqual([{ segmentType: 'intro', endSeconds: 90 }, null]);
  });

  it('shows title time on screen for a transcode that began part way in', () => {
    const r = receiver();
    r.load(r.session({ startOffset: 1380, metadata: { title: 'South Park', subtitle: 'S1E1' } }));
    r.video.currentTime = 10;
    r.video.paused = true;
    r.tick();

    expect(r.el('hud').classes.has('visible')).toBe(true);
    expect(r.el('hud-title').textContent).toBe('South Park');
    expect(r.el('hud-elapsed').textContent).toBe('23:10');
    expect(r.el('hud-total').textContent).toBe('24:25');
    expect(parseFloat(r.el('hud-fill').style.width)).toBeCloseTo((1390 / 1465) * 100, 3);
    expect(r.el('idle').classes.has('visible')).toBe(false);
  });

  it('moves through a transcode from the remote in title time', async () => {
    const r = receiver({
      '/seek': { seek_generation: 1, start_time: 1410, sub_segment_offset: 0 }
    });
    r.load(r.session({ startOffset: 1380 }));
    r.video.currentTime = 10;
    r.press('ArrowRight');
    await new Promise((resolve) => setImmediate(resolve));

    expect(r.posts['https://kob/api/v1/admin/playback/sessions/sess-1/seek']).toEqual({
      time_seconds: 1420
    });
  });

  const book = {
    baseUrl: 'https://kob',
    token: 'tok',
    bookId: 'book-1',
    fileIds: ['f1', 'f2'],
    fileDurations: [600, 900],
    currentFileIndex: 0,
    currentTime: 120,
    duration: 1500,
    chapters: [
      { fileIndex: 0, title: 'One', startSeconds: 0, endSeconds: 600 },
      { fileIndex: 1, title: 'Two', startSeconds: 0, endSeconds: 900 }
    ],
    metadata: { title: 'Treehouse', author: 'Andy' }
  };

  it('plays a book cast after a film as a book, not as the film', async () => {
    const r = receiver();
    r.load(r.session({ startOffset: 1380 }));
    r.ask('INIT_AUDIOBOOK', book);

    const first = r.loads.at(-1);
    expect(first?.media.contentUrl).toBe(
      'https://kob/api/v1/admin/books/book-1/audio/f1?container=mp4&token=tok'
    );
    expect(first?.currentTime).toBe(120);

    expect(await r.seek(300)).toEqual({ currentTime: 300 });
    expect(r.fetches.some((u) => u.includes('/seek'))).toBe(false);
  });

  it('moves a cast book to another file, and shows the whole book on screen', () => {
    const r = receiver();
    r.ask('INIT_AUDIOBOOK', book);
    r.ask('AUDIOBOOK_SEEK', { fileIndex: 1, time: 30 });

    expect(r.loads.at(-1)?.media.contentUrl).toContain('/audio/f2?container=mp4');
    expect(r.loads.at(-1)?.currentTime).toBe(30);

    r.video.currentTime = 30;
    r.tick();
    expect(r.el('hud-elapsed').textContent).toBe('10:30');
    expect(r.el('hud-total').textContent).toBe('25:00');
    expect(r.el('hud-subtitle').textContent).toBe('Andy · Two');
  });

  it('leaves the transport to a TV that draws its own', async () => {
    const r = receiver(
      { '/seek': { seek_generation: 1, start_time: 1410, sub_segment_offset: 0 } },
      () => true,
      'Mozilla/5.0 (Linux; Android 14.0) CrKey/1.56.500000 DeviceType/AndroidTV'
    );
    r.load(r.session({ startOffset: 1380 }));
    r.video.currentTime = 10;
    r.video.paused = true;
    r.tick();
    r.press('ArrowRight');
    await new Promise((resolve) => setImmediate(resolve));

    expect(r.el('hud').classes.has('visible')).toBe(false);
    expect(r.fetches.some((u) => u.includes('/seek'))).toBe(false);
  });

  const read = {
    url: 'https://kob/api/v1/admin/tts/cast/r1/audio?token=tok',
    title: 'Treehouse',
    subtitle: 'Chapter 3',
    coverUrl: 'https://kob/covers/b1.jpg?token=tok'
  };

  it('reads a book aloud as a live stream, with the book on screen', () => {
    const r = receiver();
    r.ask('INIT_TTS', read);

    const load = r.loads.at(-1);
    expect(load?.media.contentUrl).toBe(read.url);
    expect(load?.media.contentType).toBe('audio/aac');
    expect(load?.media.streamType).toBe('LIVE');
    expect(r.el('art').style.backgroundImage).toBe(`url("${read.coverUrl}")`);

    r.video.currentTime = 12;
    r.tick();
    expect(r.last<{ currentTime: number }>('POSITION_UPDATE')?.currentTime).toBe(12);
    expect(r.el('art').classes.has('visible')).toBe(true);
    expect(r.el('hud-title').textContent).toBe('Treehouse');
    expect(r.el('hud-subtitle').textContent).toBe('Chapter 3');
  });

  it('starts a read-aloud where the sender says, and counts from there', () => {
    const r = receiver();
    r.ask('INIT_TTS', { ...read, startMs: 5000 });

    expect(r.loads.at(-1)?.media.contentUrl).toBe(`${read.url}&from_ms=5000`);
    r.video.currentTime = 1;
    r.tick();
    expect(r.last<{ currentTime: number }>('POSITION_UPDATE')?.currentTime).toBe(6);
  });

  it('reopens a dropped read-aloud where it was and keeps its clock', () => {
    const r = receiver();
    r.ask('INIT_TTS', read);
    r.video.currentTime = 30;
    r.emit('ERROR');

    expect(r.loads.at(-1)?.media.contentUrl).toBe(`${read.url}&from_ms=30000`);
    r.video.currentTime = 2;
    r.tick();
    expect(r.last<{ currentTime: number }>('POSITION_UPDATE')?.currentTime).toBe(32);
    expect(r.status({ currentTime: 2 }).currentTime).toBe(32);
  });

  it('gives up reopening a read-aloud that keeps failing', () => {
    const r = receiver();
    r.ask('INIT_TTS', read);
    for (let i = 0; i < 10; i++) r.emit('ERROR');

    expect(r.loads.length).toBeLessThan(6);
  });

  it('tells the sender a read-aloud reached its end, not that the media finished', () => {
    const r = receiver();
    r.ask('INIT_TTS', read);
    r.finish('INTERRUPTED');
    expect(r.sent.filter((m) => m.type === 'TTS_FINISHED')).toEqual([]);

    r.finish('END_OF_STREAM');
    expect(r.sent.filter((m) => m.type === 'TTS_FINISHED')).toHaveLength(1);
    expect(r.sent.filter((m) => m.type === 'MEDIA_FINISHED')).toEqual([]);
  });

  it('stops a read-aloud when the sender does', () => {
    const r = receiver();
    r.ask('INIT_TTS', read);
    r.ask('TTS_STOP');

    expect(r.stops()).toBe(1);
    r.finish('END_OF_STREAM');
    expect(r.sent.filter((m) => m.type === 'TTS_FINISHED')).toEqual([]);
  });
});
