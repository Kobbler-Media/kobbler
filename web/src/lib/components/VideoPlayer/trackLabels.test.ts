import { describe, expect, it } from 'vitest';
import { audioTrackLabel, subtitleTrackLabel } from './trackLabels';
import type { PlaybackAudioTrack, PlaybackSubtitleTrack } from '#lib/types.js';

const sub = (over: Partial<PlaybackSubtitleTrack>): PlaybackSubtitleTrack => ({
  track_id: 't',
  stream_index: 3,
  codec: 'subrip',
  language: null,
  language_name: null,
  title: null,
  url: null,
  format: 'vtt',
  is_default: false,
  is_forced: false,
  is_hearing_impaired: false,
  requires_transcode: false,
  ...over
});

const audio = (over: Partial<PlaybackAudioTrack>): PlaybackAudioTrack => ({
  stream_index: 1,
  codec: 'aac',
  channels: 2,
  language: null,
  language_name: null,
  title: null,
  is_default: false,
  ...over
});

describe('track labels', () => {
  it('name a track by its language, not its code', () => {
    expect(subtitleTrackLabel(sub({ language: 'eng', language_name: 'English' }))).toBe(
      'English - subrip'
    );
    expect(audioTrackLabel(audio({ language: 'jpn', language_name: 'Japanese' }))).toBe(
      'Japanese – 2ch – aac'
    );
  });

  it('fall back to the code the server could not name', () => {
    expect(subtitleTrackLabel(sub({ language: 'xyz' }))).toBe('xyz - subrip');
  });

  it('say when a subtitle is forced or for the hard of hearing', () => {
    expect(
      subtitleTrackLabel(sub({ language: 'eng', language_name: 'English', is_forced: true }))
    ).toBe('English - subrip (forced)');
    expect(
      subtitleTrackLabel(
        sub({ language: 'eng', language_name: 'English', is_hearing_impaired: true })
      )
    ).toBe('English - subrip (SDH)');
  });

  it('number a track that says nothing', () => {
    expect(subtitleTrackLabel(sub({ codec: '' }))).toBe('Track 3');
    expect(audioTrackLabel(audio({ codec: '', channels: 0 }))).toBe('Track 1');
  });
});
