/**
 * Client capability detection for media playback.
 * Probes browser codec support via MediaSource.isTypeSupported().
 */

export type ClientCapabilities = {
  video_codecs: string[];
  audio_codecs: string[];
  containers: string[];
};

const VIDEO_PROBES: [string, string][] = [
  ['h264', 'video/mp4; codecs="avc1.42E01E"'],
  ['hevc', 'video/mp4; codecs="hvc1.1.6.L93.B0"']
];

const AUDIO_PROBES: [string, string][] = [
  ['aac', 'audio/mp4; codecs="mp4a.40.2"'],
  ['ac3', 'audio/mp4; codecs="ac-3"'],
  ['opus', 'audio/webm; codecs="opus"'],
  ['flac', 'audio/flac']
];

function isSupported(mime: string): boolean {
  if (typeof MediaSource !== 'undefined' && MediaSource.isTypeSupported(mime)) {
    return true;
  }
  return false;
}

export function detectCapabilities(): ClientCapabilities {
  const video_codecs: string[] = [];
  const audio_codecs: string[] = [];

  for (const [codec, mime] of VIDEO_PROBES) {
    if (isSupported(mime)) {
      video_codecs.push(codec);
    }
  }

  for (const [codec, mime] of AUDIO_PROBES) {
    if (isSupported(mime)) {
      audio_codecs.push(codec);
    }
  }

  // Always include h264 and aac as baseline (every modern browser handles them)
  if (!video_codecs.includes('h264')) {
    video_codecs.unshift('h264');
  }
  if (!audio_codecs.includes('aac')) {
    audio_codecs.unshift('aac');
  }

  // We deliver everything as fMP4 via HLS
  const containers = ['mp4'];

  return { video_codecs, audio_codecs, containers };
}
