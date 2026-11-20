import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CastMessage } from '#lib/cast/messages.js';

const sdk = vi.hoisted(() => ({
  state: null as ((connected: boolean, deviceName: string | null, resumed: boolean) => void) | null,
  message: null as ((msg: CastMessage) => void) | null,
  sendMessage: vi.fn()
}));
const api = vi.hoisted(() => ({
  createPlaybackToken: vi.fn(),
  revokePlaybackToken: vi.fn()
}));
const player = vi.hoisted(() => ({
  state: { playing: true },
  pause: vi.fn(),
  togglePlayPause: vi.fn()
}));
const ui = vi.hoisted(() => ({ showToast: vi.fn() }));

vi.mock('#lib/cast/sender.js', () => ({
  initCastSdk: vi.fn().mockResolvedValue(true),
  requestSession: vi.fn(),
  endSession: vi.fn(),
  onStateChange: (listener: typeof sdk.state) => {
    sdk.state = listener;
  },
  onMessage: (listener: typeof sdk.message) => {
    sdk.message = listener;
  },
  sendMessage: sdk.sendMessage,
  loadMedia: vi.fn(),
  isSdkReady: () => true,
  getDeviceName: () => null
}));
vi.mock('#lib/api/client.js', () => api);
vi.mock('#lib/stores/audioplayer.svelte.js', () => ({
  getAudioPlayerState: () => ({
    isActive: true,
    playing: player.state.playing,
    currentBook: { id: 'book', title: 'Book', authors: [], cover_url: null },
    currentFileId: 'file',
    unifiedChapters: [],
    currentTrackIndex: 0,
    currentTime: 12,
    duration: 100
  }),
  getFileIds: () => ['file'],
  pause: player.pause,
  togglePlayPause: player.togglePlayPause
}));
vi.mock('#lib/stores/ui.svelte.js', () => ui);
vi.stubGlobal('window', { location: { origin: 'http://kobbler.test' } });

const { initCast } = await import('./cast.svelte');
const { isAudiobookCasting } = await import('#lib/cast/audiobook.js');

async function castAudiobookTo(deviceName: string, playing: boolean) {
  player.state.playing = playing;
  sdk.state!(true, deviceName, false);
  await vi.waitFor(() =>
    expect(sdk.sendMessage).toHaveBeenCalledWith(
      expect.objectContaining({ type: 'INIT_AUDIOBOOK' })
    )
  );
  expect(player.pause).toHaveBeenCalled();
  expect(isAudiobookCasting()).toBe(true);
}

beforeAll(async () => {
  await initCast('app');
});

beforeEach(() => {
  vi.clearAllMocks();
  api.createPlaybackToken.mockResolvedValue({ token: 'cast-token' });
  api.revokePlaybackToken.mockResolvedValue(undefined);
  sdk.state!(false, null, false);
});

describe('a receiver that cannot play what it was sent', () => {
  it.each([
    { playing: true, resumes: 1 },
    { playing: false, resumes: 0 }
  ])('keeps an audiobook local (playing: $playing)', async ({ playing, resumes }) => {
    await castAudiobookTo('Lounge TV', playing);

    sdk.message!({ type: 'UNSUPPORTED', payload: { type: 'INIT_AUDIOBOOK' } });

    expect(ui.showToast).toHaveBeenCalledWith("Lounge TV can't play audiobooks");
    expect(isAudiobookCasting()).toBe(false);
    expect(api.revokePlaybackToken).toHaveBeenCalledWith('cast-token');
    expect(player.togglePlayPause).toHaveBeenCalledTimes(resumes);
  });

  it('says so for read-aloud', () => {
    sdk.state!(true, 'Bedroom TV', true);

    sdk.message!({ type: 'UNSUPPORTED', payload: { type: 'INIT_TTS' } });

    expect(ui.showToast).toHaveBeenCalledWith("Bedroom TV can't play read-aloud");
    expect(player.togglePlayPause).not.toHaveBeenCalled();
  });
});
