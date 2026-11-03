import { beforeEach, describe, expect, it, vi } from 'vitest';

const api = vi.hoisted(() => ({
  streamSearch: vi.fn(),
  streamResolve: vi.fn(),
  streamPrewarm: vi.fn(),
  claimStreamPrewarm: vi.fn(),
  touchStreamPrewarm: vi.fn(),
  releaseStreamPrewarm: vi.fn(),
  createPlaybackSession: vi.fn()
}));

vi.mock('#lib/api/client.js', async (importOriginal) => ({
  ...(await importOriginal<typeof import('#lib/api/client.js')>()),
  ...api
}));
vi.mock('#lib/utils/media.js', () => ({ detectCapabilities: () => ({}) }));

const { ApiError } = await import('#lib/api/client.js');
const { createNextEpisodeWarmer } = await import('./episodeStream');

const session = (id: string) => ({
  session_id: id,
  method: 'direct_play',
  playlist_url: null,
  direct_url: null,
  duration_seconds: 1,
  subtitles: [],
  audio_tracks: []
});

const option = {
  id: 'o',
  release_title: 'R',
  size: 1,
  indexer: 'x',
  info_hash: 'h',
  total_score: 1,
  match_score: 1,
  quality_score: 1,
  debrid_provider: 'rd',
  debrid_provider_id: 'rd',
  cached: true,
  source_type: 'torrent',
  files: [{ id: 1, path: 'a.mkv', size: 1 }]
};

const caps = { video_codecs: [], audio_codecs: [], containers: [] };

const next = (file_id: string | null = null) => ({ episode_id: 'e2', file_id });

beforeEach(() => {
  vi.clearAllMocks();
  vi.useFakeTimers();
  api.streamPrewarm.mockResolvedValue({ state: 'warming', prewarm_id: 'pw', option });
  api.releaseStreamPrewarm.mockResolvedValue(undefined);
  api.touchStreamPrewarm.mockResolvedValue(undefined);
  api.streamSearch.mockResolvedValue({ options: [option], errors: [] });
  api.streamResolve.mockResolvedValue(session('resolved'));
  api.createPlaybackSession.mockResolvedValue(session('local'));
});

describe('next episode warmer', () => {
  it('warms the next episode once playback passes 70%', async () => {
    const warmer = createNextEpisodeWarmer();
    warmer.consider(next(), 0.69);
    await vi.advanceTimersByTimeAsync(0);
    expect(api.streamPrewarm).not.toHaveBeenCalled();
    warmer.consider(next(), 0.7);
    warmer.consider(next(), 0.71);
    await vi.advanceTimersByTimeAsync(0);
    expect(api.streamPrewarm).toHaveBeenCalledTimes(1);
    expect(api.streamPrewarm).toHaveBeenCalledWith({
      media_type: 'episode',
      media_id: 'e2',
      capabilities: {}
    });
  });

  it('never warms an episode with a local file, even from a debrid session', async () => {
    const warmer = createNextEpisodeWarmer();
    warmer.consider(next('f2'), 0.9);
    await vi.advanceTimersByTimeAsync(0);
    expect(api.streamPrewarm).not.toHaveBeenCalled();
  });

  it('claims the warmed episode on autoplay', async () => {
    api.claimStreamPrewarm.mockResolvedValue(session('claimed'));
    const warmer = createNextEpisodeWarmer();
    warmer.consider(next(), 0.8);
    await vi.advanceTimersByTimeAsync(0);
    expect((await warmer.open(next(), caps))?.session.session_id).toBe('claimed');
    expect(api.streamResolve).not.toHaveBeenCalled();
  });

  it('resolves as before when the claim has gone stale', async () => {
    api.claimStreamPrewarm.mockRejectedValue(new ApiError(410, 'gone'));
    const warmer = createNextEpisodeWarmer();
    warmer.consider(next(), 0.8);
    await vi.advanceTimersByTimeAsync(0);
    expect((await warmer.open(next(), caps))?.session.session_id).toBe('resolved');
  });

  it('releases a warm the viewer never used, and a different episode resolves cold', async () => {
    const warmer = createNextEpisodeWarmer();
    warmer.consider(next(), 0.8);
    await vi.advanceTimersByTimeAsync(0);
    expect((await warmer.open({ episode_id: 'e9', file_id: null }, caps))?.session.session_id).toBe(
      'resolved'
    );
    expect(api.releaseStreamPrewarm).toHaveBeenCalledWith('pw', false);
    expect(api.claimStreamPrewarm).not.toHaveBeenCalled();
  });

  it('releases on close', async () => {
    const warmer = createNextEpisodeWarmer();
    warmer.consider(next(), 0.8);
    await vi.advanceTimersByTimeAsync(0);
    warmer.release(true);
    expect(api.releaseStreamPrewarm).toHaveBeenCalledWith('pw', true);
  });

  it('opens an episode with a local file through library playback, not debrid', async () => {
    const warmer = createNextEpisodeWarmer();
    warmer.consider(next('f2'), 0.9);
    await vi.advanceTimersByTimeAsync(0);
    const opened = await warmer.open(next('f2'), caps);
    expect(opened).toEqual({ session: session('local'), fileId: 'f2' });
    expect(api.createPlaybackSession).toHaveBeenCalledWith({
      media_type: 'episode',
      file_id: 'f2',
      capabilities: caps
    });
    expect(api.streamPrewarm).not.toHaveBeenCalled();
    expect(api.streamSearch).not.toHaveBeenCalled();
    expect(api.streamResolve).not.toHaveBeenCalled();
    expect(api.claimStreamPrewarm).not.toHaveBeenCalled();
  });

  it('streams an episode without a file by debrid, claiming the warm', async () => {
    api.claimStreamPrewarm.mockResolvedValue(session('claimed'));
    const warmer = createNextEpisodeWarmer();
    warmer.consider(next(), 0.8);
    await vi.advanceTimersByTimeAsync(0);
    const opened = await warmer.open(next(), caps);
    expect(opened).toEqual({ session: session('claimed'), fileId: 'debrid-claimed' });
    expect(api.createPlaybackSession).not.toHaveBeenCalled();
  });

  it('has nothing to open when no debrid release is ready', async () => {
    api.streamSearch.mockResolvedValue({ options: [], errors: [] });
    const warmer = createNextEpisodeWarmer();
    expect(await warmer.open(next(), caps)).toBeNull();
  });
});
