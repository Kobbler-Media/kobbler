import { describe, expect, it } from 'vitest';
import type { PlaybackAudioTrack, PlaybackMethod } from '$lib/types';
import { startingAudio } from './audioPreference';

const track = (stream_index: number, language: string, is_default = false): PlaybackAudioTrack => ({
  stream_index,
  codec: 'ac3',
  channels: 6,
  language,
  title: null,
  is_default
});

const itaDefault = [track(1, 'ita', true), track(2, 'eng')];

describe('startingAudio', () => {
  it.each<
    [
      string,
      PlaybackMethod,
      PlaybackAudioTrack[],
      number | undefined,
      ReturnType<typeof startingAudio>
    ]
  >([
    [
      'switches a direct-played release off its default dub onto the preferred track',
      'direct_play',
      itaDefault,
      2,
      { active: 1, switchTo: 2 }
    ],
    [
      'leaves a direct play alone when the preferred track is the one the browser plays',
      'direct_play',
      itaDefault,
      1,
      { active: 1, switchTo: null }
    ],
    [
      'trusts the server mapping for HLS and shows the preferred track as active',
      'remux',
      itaDefault,
      2,
      { active: 2, switchTo: null }
    ],
    [
      'falls back to the default flag when the server names no preference',
      'video_transcode',
      itaDefault,
      undefined,
      { active: 1, switchTo: null }
    ],
    [
      'ignores a preferred index the session does not carry',
      'direct_play',
      itaDefault,
      7,
      { active: 1, switchTo: null }
    ],
    [
      'reads the first track as what the browser plays when none is flagged default',
      'direct_play',
      [track(3, 'ita'), track(4, 'eng')],
      4,
      { active: 3, switchTo: 4 }
    ],
    [
      'has nothing to pick before the probe lists tracks',
      'direct_play',
      [],
      2,
      { active: null, switchTo: null }
    ]
  ])('%s', (_, method, tracks, preferred, expected) => {
    expect(startingAudio(method, tracks, preferred)).toEqual(expected);
  });
});
