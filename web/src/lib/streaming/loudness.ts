import type { PlaybackLoudness } from '#lib/types.js';

const TARGET_LUFS = -14;
const MAX_BOOST_DB = 6;
const PEAK_CEILING_DB = -1;

/**
 * Players fold surround down to stereo without normalizing and apply gain after it, so
 * a boost is capped by the measured peak of that fold-down. Cuts are never deepened:
 * levelling should not make clipping worse, nor invent attenuation of its own.
 */
export function levellingGain({ lufs, downmix_peak_db }: PlaybackLoudness): number {
  if (lufs == null || !isFinite(lufs)) return 1;
  const headroomDb =
    downmix_peak_db == null ? Infinity : Math.max(PEAK_CEILING_DB - downmix_peak_db, 0);
  const gainDb = Math.min(TARGET_LUFS - lufs, MAX_BOOST_DB, headroomDb);
  return Math.pow(10, gainDb / 20);
}

export function loudnessByFile(
  files: ({ id: string } & PlaybackLoudness)[]
): Record<string, PlaybackLoudness> {
  return Object.fromEntries(
    files.map((f) => [f.id, { lufs: f.lufs, downmix_peak_db: f.downmix_peak_db }])
  );
}
