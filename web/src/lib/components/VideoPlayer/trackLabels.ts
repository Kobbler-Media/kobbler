import type { PlaybackAudioTrack, PlaybackSubtitleTrack } from '#lib/types.js';

export function subtitleTrackLabel(s: PlaybackSubtitleTrack): string {
  const base =
    [s.title, s.language_name ?? s.language, s.codec].filter(Boolean).join(' - ') ||
    `Track ${s.stream_index}`;
  const marks = [
    s.is_forced && 'forced',
    s.is_hearing_impaired && 'SDH',
    s.requires_transcode && 'transcode'
  ].filter(Boolean);
  return marks.length ? `${base} (${marks.join(', ')})` : base;
}

export function audioTrackLabel(a: PlaybackAudioTrack): string {
  return (
    [a.title, a.language_name ?? a.language, a.channels > 0 ? `${a.channels}ch` : null, a.codec]
      .filter(Boolean)
      .join(' – ') || `Track ${a.stream_index}`
  );
}
