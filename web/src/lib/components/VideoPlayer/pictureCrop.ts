export interface PictureArea {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface FrameRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

export type AspectMode = 'auto' | 'fit' | 'fill';
export type FitMode = 'contain' | 'cover';

export interface CuePlacement {
  line: number;
  position: number;
  size: number;
}

const WHOLE_FRAME_TOLERANCE = 0.001;
const AUTO_FILL_TOLERANCE = 0.15;
const CUE_BOTTOM_MARGIN = 0.03;

// ASS and burned-in subtitles are laid out against the encoded frame, so their lines sit in
// the bars as often as not; trimming the bars would trim the subtitles with them.
const FRAME_ANCHORED_SUBTITLES = new Set(['ass', 'burn_in']);

export function cropArea(
  picture: PictureArea | null | undefined,
  subtitleFormat: string | null | undefined
): PictureArea | null {
  if (!picture || (subtitleFormat && FRAME_ANCHORED_SUBTITLES.has(subtitleFormat))) return null;
  const { x, y, width, height } = picture;
  if (![x, y, width, height].every(Number.isFinite)) return null;
  if (x < 0 || y < 0 || width <= 0 || height <= 0) return null;
  if (x + width > 1 + WHOLE_FRAME_TOLERANCE || y + height > 1 + WHOLE_FRAME_TOLERANCE) return null;
  if (width >= 1 - WHOLE_FRAME_TOLERANCE && height >= 1 - WHOLE_FRAME_TOLERANCE) return null;
  return picture;
}

export function sessionPicture(
  session: { session_id: string; picture?: PictureArea },
  late: { sessionId: string; picture: PictureArea } | null
): PictureArea | undefined {
  return late && late.sessionId === session.session_id ? late.picture : session.picture;
}

export function resolveFit(
  mode: AspectMode,
  contentWidth: number,
  contentHeight: number,
  containerWidth: number,
  containerHeight: number
): FitMode {
  if (mode === 'fit') return 'contain';
  if (mode === 'fill') return 'cover';
  if (contentWidth <= 0 || contentHeight <= 0) return 'contain';
  if (containerWidth <= 0 || containerHeight <= 0) return 'contain';
  const contentAR = contentWidth / contentHeight;
  const screenAR = containerWidth / containerHeight;
  return Math.abs(contentAR - screenAR) / screenAR <= AUTO_FILL_TOLERANCE ? 'cover' : 'contain';
}

export function videoFrameRect(
  fit: FitMode,
  videoWidth: number,
  videoHeight: number,
  containerWidth: number,
  containerHeight: number,
  area: PictureArea
): FrameRect | null {
  if (videoWidth <= 0 || videoHeight <= 0 || containerWidth <= 0 || containerHeight <= 0) {
    return null;
  }
  const sx = containerWidth / (videoWidth * area.width);
  const sy = containerHeight / (videoHeight * area.height);
  const scale = fit === 'cover' ? Math.max(sx, sy) : Math.min(sx, sy);
  const width = videoWidth * scale;
  const height = videoHeight * scale;
  return {
    left: (containerWidth - width * area.width) / 2 - width * area.x,
    top: (containerHeight - height * area.height) / 2 - height * area.y,
    width,
    height
  };
}

export function frameStyle(rect: FrameRect): string {
  return `left: ${rect.left}px; top: ${rect.top}px; width: ${rect.width}px; height: ${rect.height}px;`;
}

export function cuePlacement(
  rect: FrameRect,
  containerWidth: number,
  containerHeight: number
): CuePlacement {
  const left = Math.max(0, -rect.left);
  const right = Math.min(rect.width, containerWidth - rect.left);
  const top = Math.max(0, -rect.top);
  const bottom = Math.min(rect.height, containerHeight - rect.top);
  return {
    line: ((bottom - (bottom - top) * CUE_BOTTOM_MARGIN) / rect.height) * 100,
    position: ((left + right) / 2 / rect.width) * 100,
    size: ((right - left) / rect.width) * 100
  };
}

export function placeCues(track: TextTrack, placement: CuePlacement | null) {
  for (const cue of Array.from(track.cues ?? []) as VTTCue[]) {
    cue.snapToLines = placement === null;
    cue.line = placement?.line ?? 'auto';
    cue.lineAlign = placement ? 'end' : 'start';
    cue.position = placement?.position ?? 'auto';
    cue.size = placement?.size ?? 100;
  }
}
