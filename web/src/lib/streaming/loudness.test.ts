import { describe, expect, it } from 'vitest';
import { activeTrackLoudness, levellingGain, loudnessByFile, withTrackLoudness } from './loudness';
import type { PlaybackAudioTrack } from '#lib/types.js';

const db = (gain: number) => 20 * Math.log10(gain);

describe('levellingGain', () => {
  it.each<[string, number | null | undefined, number | null | undefined, number]>([
    ['leaves unmeasured audio alone', undefined, undefined, 0],
    ['leaves audio already at target alone', -20, -3, 0],
    ['caps a quiet surround mix at 1 dB below its downmix peak', -23.04, -1.6, 0.6],
    ['boosts a quiet mix to the target when the downmix has the headroom', -23.04, -6, 3.04],
    ['still never boosts past the ceiling', -30, -20, 6],
    ['falls back to the plain ceiling until the peak is measured', -30, null, 6],
    ['never boosts a downmix that already clips', -26, 0.5, 0],
    ['cuts audio mixed louder than the target', -16, 0.5, -4],
    ['still cuts loud audio', -10, -0.1, -10]
  ])('%s', (_, lufs, downmix_peak_db, expectedDb) => {
    expect(db(levellingGain({ lufs, downmix_peak_db }))).toBeCloseTo(expectedDb, 3);
  });
});

describe('loudnessByFile', () => {
  it("keys each audiobook file's figures by its id so a track change picks up its own", () => {
    expect(
      loudnessByFile([
        { id: 'a', lufs: -19, downmix_peak_db: -2 },
        { id: 'b', lufs: -24 },
        { id: 'c' }
      ])
    ).toEqual({
      a: { lufs: -19, downmix_peak_db: -2 },
      b: { lufs: -24, downmix_peak_db: undefined },
      c: { lufs: undefined, downmix_peak_db: undefined }
    });
  });
});

const track = (
  stream_index: number,
  extra: Partial<PlaybackAudioTrack> = {}
): PlaybackAudioTrack => ({
  stream_index,
  codec: 'eac3',
  channels: 6,
  language: null,
  language_name: null,
  title: null,
  is_default: false,
  ...extra
});

describe('activeTrackLoudness', () => {
  const dub = track(1, { is_default: true, lufs: -26.8, downmix_peak_db: -5.3 });
  const original = track(2, { lufs: -24.7, downmix_peak_db: -4.0 });

  it('levels by the track that is playing, not the default-flagged dub', () => {
    expect(activeTrackLoudness([dub, original], 2)).toEqual({ lufs: -24.7, downmix_peak_db: -4.0 });
  });

  it('falls back to the default track before one is chosen', () => {
    expect(activeTrackLoudness([dub, original], null)).toEqual({
      lufs: -26.8,
      downmix_peak_db: -5.3
    });
  });

  it('leaves an unmeasured track unlevelled', () => {
    expect(activeTrackLoudness([dub, track(2)], 2)).toEqual({});
  });
});

describe('withTrackLoudness', () => {
  it('gives each track its own measurement and leaves the rest as they were', () => {
    const tracks = withTrackLoudness(
      [track(1), track(2, { lufs: -30 }), track(3)],
      [
        { stream_index: 2, lufs: -24.7, downmix_peak_db: -4 },
        { stream_index: 1, lufs: -26.8, downmix_peak_db: -5.3 }
      ]
    );
    expect(tracks.map((t) => [t.stream_index, t.lufs, t.downmix_peak_db])).toEqual([
      [1, -26.8, -5.3],
      [2, -24.7, -4],
      [3, undefined, undefined]
    ]);
  });
});
