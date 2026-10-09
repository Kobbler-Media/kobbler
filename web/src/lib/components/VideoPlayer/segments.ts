import type { SkipSegment } from '$lib/types';

export const SEGMENT_COLORS: Record<string, string> = {
  intro: 'rgba(159, 229, 176, 0.55)',
  credits: 'rgba(168, 200, 240, 0.55)',
  recap: 'rgba(240, 225, 154, 0.55)',
  post_credits_scene: 'rgba(240, 176, 200, 0.55)',
  ad_break: 'rgba(239, 68, 68, 0.55)'
};

export const SEGMENT_LABELS: Record<string, string> = {
  intro: 'Intro',
  credits: 'Credits',
  recap: 'Recap',
  post_credits_scene: 'Post-Credits Scene',
  ad_break: 'Ad Break'
};

/** True when a skip segment should be shown as its own region/entry (not just a chapter duplicate). */
export function isVisibleSkipSegment(seg: SkipSegment): boolean {
  return seg.source !== 'chapter' && !!SEGMENT_LABELS[seg.segment_type];
}

/** Build a label like "Intro (Detected · 0.87)" or "Credits (Manual)". */
export function segmentTitle(seg: SkipSegment): string {
  const base = SEGMENT_LABELS[seg.segment_type] ?? seg.segment_type;
  const tag = seg.source === 'manual' ? 'Manual' : 'Detected';
  const conf =
    seg.confidence !== undefined && seg.confidence !== null && seg.source !== 'manual'
      ? ` · ${seg.confidence.toFixed(2)}`
      : '';
  return `${base} (${tag}${conf})`;
}
