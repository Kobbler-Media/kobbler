import { describe, expect, it } from 'vitest';
import { levellingGain, loudnessByFile } from './loudness';

const db = (gain: number) => 20 * Math.log10(gain);

describe('levellingGain', () => {
  it.each<[string, number | null | undefined, number | null | undefined, number]>([
    ['leaves unmeasured audio alone', undefined, undefined, 0],
    ['leaves audio already at target alone', -14, -3, 0],
    ['caps a quiet surround mix at 1 dB below its downmix peak', -23.04, -1.6, 0.6],
    ['boosts as far as the downmix headroom allows', -23.04, -6, 5],
    ['still never boosts past the ceiling', -30, -20, 6],
    ['falls back to the plain ceiling until the peak is measured', -23.04, null, 6],
    ['never boosts a downmix that already clips', -16, 0.5, 0],
    ['still cuts loud audio', -10, -0.1, -4]
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
