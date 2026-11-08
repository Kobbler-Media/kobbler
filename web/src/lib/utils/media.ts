/**
 * Client capability detection for media playback.
 * Probes browser codec support via MediaSource.isTypeSupported().
 */

export type ClientCapabilities = {
  video_codecs: string[];
  audio_codecs: string[];
  containers: string[];
  max_height?: number;
  hdr_formats?: HdrFormat[];
};

export type HdrFormat = 'hdr10' | 'hlg' | 'dolby_vision';

const TEN_BIT_PROBES = [
  'video/mp4; codecs="hvc1.2.4.L153.B0"',
  'video/mp4; codecs="vp09.02.10.10"'
];
const DOLBY_VISION_PROBE = 'video/mp4; codecs="dvh1.05.06"';

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

  return { video_codecs, audio_codecs, containers, hdr_formats: detectHdrFormats() };
}

// Chromium and Safari tone-map HDR video onto an SDR display themselves; Firefox
// shows it washed out.
function detectHdrFormats(): HdrFormat[] {
  const hdrDisplay =
    typeof matchMedia !== 'undefined' && matchMedia('(dynamic-range: high)').matches;
  const tonemaps = hdrDisplay || !/firefox/i.test(navigator.userAgent);
  const formats: HdrFormat[] = [];
  if (tonemaps && TEN_BIT_PROBES.some(isSupported)) formats.push('hdr10', 'hlg');
  if (hdrDisplay && isSupported(DOLBY_VISION_PROBE)) formats.push('dolby_vision');
  return formats;
}
