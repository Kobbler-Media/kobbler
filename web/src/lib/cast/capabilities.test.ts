import { afterEach, describe, expect, it, vi } from 'vitest';
import { castCapabilities, receiverCapabilities, receiverReported } from './capabilities.js';

describe('castCapabilities', () => {
  // A receiver plays a transcode as fMP4 through MSE, which rejects MP3, FLAC and Opus
  // inside it, so audio it cannot take there has to arrive as AAC.
  it('asks for audio a receiver can play out of an fMP4 playlist', () => {
    expect(castCapabilities('Projector TV').audio_codecs).toEqual(['aac']);
    expect(castCapabilities('Lounge Google TV').audio_codecs).toEqual(['aac', 'ac3', 'eac3']);
  });
});

describe('receiverCapabilities', () => {
  const probed = {
    video_codecs: ['h264', 'hevc'],
    audio_codecs: ['aac', 'ac3'],
    containers: ['mp4'],
    max_height: 1080
  };

  afterEach(() => {
    receiverReported(null);
    vi.useRealTimers();
  });

  it('takes what the receiver says it decodes over what its name suggests', async () => {
    receiverReported(probed);
    expect(await receiverCapabilities('Projector TV')).toEqual(probed);
  });

  it('waits for a receiver that has not answered yet', async () => {
    const pending = receiverCapabilities('Projector TV');
    receiverReported(probed);
    expect(await pending).toEqual(probed);
  });

  it('falls back to the name when the receiver stays quiet', async () => {
    vi.useFakeTimers();
    const pending = receiverCapabilities('Projector TV');
    vi.advanceTimersByTime(5000);
    expect(await pending).toEqual(castCapabilities('Projector TV'));
  });
});
