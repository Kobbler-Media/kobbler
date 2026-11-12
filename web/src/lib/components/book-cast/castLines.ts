import type { BookCastLineContext } from '#lib/types.js';

export function firstPassName(
  line: { speaker: string | null },
  character: { name: string } | undefined
): string | null {
  if (!character || !line.speaker) return null;
  return line.speaker.toLowerCase() === character.name.toLowerCase() ? null : line.speaker;
}

type Placed = { segment_index: number; context: BookCastLineContext | null; text?: string | null };

export interface Piece<L> {
  text: string;
  line?: L;
}

export interface Paragraph<L> {
  segment_index: number;
  pieces: Piece<L>[];
}

export function paragraphs<L extends Placed>(lines: L[]): Paragraph<L>[] {
  const out: Paragraph<L>[] = [];
  for (const l of lines) {
    let p = out.at(-1);
    if (!p || p.segment_index !== l.segment_index) {
      p = { segment_index: l.segment_index, pieces: [] };
      out.push(p);
    }
    if (!l.context) {
      p.pieces.push({ text: `“${l.text ?? ''}”`, line: l });
      continue;
    }
    const { pre, quote, post } = l.context;
    const full = pre + quote + post;
    // The previous line's trailing narration runs on to the end of the
    // paragraph, through this quote; it is cut again at this quote's start.
    if (p.pieces.at(-1)?.line === undefined) p.pieces.pop();
    const done = p.pieces.reduce((n, x) => n + x.text.length, 0);
    const lead = full.slice(done, pre.length);
    if (lead) p.pieces.push({ text: lead });
    p.pieces.push({ text: quote, line: l });
    if (post) p.pieces.push({ text: post });
  }
  return out;
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** A paragraph cut around its quotes, whose spans count UTF-8 bytes as the server does. */
export function cutQuotes<L extends { span_start: number; span_end: number }>(
  text: string,
  quotes: L[]
): Piece<L>[] {
  const bytes = encoder.encode(text);
  const out: Piece<L>[] = [];
  let at = 0;
  for (const q of quotes.toSorted((a, b) => a.span_start - b.span_start)) {
    if (q.span_start < at || q.span_end > bytes.length) continue;
    if (q.span_start > at) out.push({ text: decoder.decode(bytes.subarray(at, q.span_start)) });
    out.push({ text: decoder.decode(bytes.subarray(q.span_start, q.span_end)), line: q });
    at = q.span_end;
  }
  if (at < bytes.length) out.push({ text: decoder.decode(bytes.subarray(at)) });
  return out;
}
