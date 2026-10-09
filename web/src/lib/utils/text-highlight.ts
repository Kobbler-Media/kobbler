/**
 * Utilities for highlighting text in iframe documents (EPUB reader).
 * Uses TreeWalker to find text nodes and wraps matches in highlight spans.
 */

const BLOCK_TAGS = new Set([
  'P',
  'DIV',
  'H1',
  'H2',
  'H3',
  'H4',
  'H5',
  'H6',
  'LI',
  'BLOCKQUOTE',
  'BR',
  'HR',
  'SECTION',
  'ARTICLE',
  'HEADER',
  'FOOTER',
  'TR',
  'DT',
  'DD',
  'FIGCAPTION'
]);

interface TextNodeEntry {
  node: Text;
  start: number;
  end: number;
}

/**
 * Normalize text for fuzzy matching: collapse whitespace, replace smart quotes,
 * normalize unicode, and trim.
 */
function normalize(text: string): string {
  return text
    .normalize('NFC')
    .replace(/[\u2018\u2019\u201A\u201B]/g, "'")
    .replace(/[\u201C\u201D\u201E\u201F]/g, '"')
    .replace(/[\u2013\u2014]/g, '-')
    .replace(/\u00A0/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

/**
 * Collect the document's text nodes together with the string they form.
 *
 * Block boundaries emit a separator that belongs to no node. Without it the
 * concatenation runs the end of one block into the start of the next
 * ("...runs off.\u201cI expected more..."), which both hides sentences that the
 * search would otherwise find and — because the aligner counts whitespace
 * tokens — shifts every word index after the join by one. The text the engine
 * was given has these boundaries in it, so the text searched here must too.
 */
function collectTextNodes(doc: Document): { entries: TextNodeEntry[]; text: string } {
  const body = doc.body;
  if (!body) return { entries: [], text: '' };

  const entries: TextNodeEntry[] = [];
  const parts: string[] = [];
  let offset = 0;

  const walker = doc.createTreeWalker(body, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT);
  let node: Node | null;
  while ((node = walker.nextNode())) {
    if (node.nodeType === Node.ELEMENT_NODE) {
      if (BLOCK_TAGS.has((node as Element).tagName)) {
        parts.push('\n\n');
        offset += 2;
      }
      continue;
    }
    const text = node as Text;
    const len = text.nodeValue?.length ?? 0;
    if (len > 0) {
      entries.push({ node: text, start: offset, end: offset + len });
      parts.push(text.nodeValue ?? '');
      offset += len;
    }
  }

  return { entries, text: parts.join('') };
}

/**
 * Build a mapping from normalized string positions back to original string positions.
 * Returns an array where normalizedMap[normalizedIndex] = originalIndex.
 */
function buildNormalizedMap(original: string): { normalized: string; map: number[] } {
  const map: number[] = [];
  let normalized = '';
  let inWhitespace = false;
  // Skip leading whitespace
  let started = false;

  for (let i = 0; i < original.length; i++) {
    let ch = original[i];

    // Apply same normalization transforms
    if ('\u2018\u2019\u201A\u201B'.includes(ch)) ch = "'";
    else if ('\u201C\u201D\u201E\u201F'.includes(ch)) ch = '"';
    else if ('\u2013\u2014'.includes(ch)) ch = '-';
    else if (ch === '\u00A0') ch = ' ';

    if (/\s/.test(ch)) {
      if (!started) continue;
      if (!inWhitespace) {
        normalized += ' ';
        map.push(i);
        inWhitespace = true;
      }
    } else {
      started = true;
      inWhitespace = false;
      normalized += ch;
      map.push(i);
    }
  }

  // Trim trailing space
  if (normalized.endsWith(' ')) {
    normalized = normalized.slice(0, -1);
    map.pop();
  }

  return { normalized, map };
}

/**
 * Find a sentence in the document and wrap the matching text nodes in
 * highlight spans. Whitespace and quote characters are normalised on both
 * sides, so the sentence does not have to match the markup byte for byte.
 *
 * Searching starts at `fromIndex`, a position previously returned by this
 * function. Read-along moves forward through a chapter, and short sentences
 * repeat inside one — "Yes." can occur a dozen times — so a search that always
 * started at the top would highlight the first of them every time and, now
 * that the page follows the highlight, page the reader backwards to it.
 * Wrapping never changes the document's text, so an index stays valid for as
 * long as the document does.
 *
 * Returns the match position, or -1 when the sentence isn't in the document.
 */
export function highlightSentence(
  doc: Document,
  sentence: string,
  className: string = 'readalong-hl',
  fromIndex: number = 0
): number {
  clearHighlights(doc, className);

  const normalizedSentence = normalize(sentence);
  if (!normalizedSentence) return -1;

  const { entries, text } = collectTextNodes(doc);
  if (entries.length === 0) return -1;

  const { normalized: normalizedText, map } = buildNormalizedMap(text);

  let matchIndex = normalizedText.indexOf(normalizedSentence, fromIndex);
  if (matchIndex === -1) {
    matchIndex = normalizedText.toLowerCase().indexOf(normalizedSentence.toLowerCase(), fromIndex);
  }
  if (matchIndex === -1) return -1;

  const wrapped = wrapRange(
    doc,
    entries,
    map,
    matchIndex,
    matchIndex + normalizedSentence.length,
    className
  );
  return wrapped ? matchIndex : -1;
}

/**
 * Wrap text in the original document corresponding to normalized positions
 * [normStart, normEnd) with highlight spans.
 */
function wrapRange(
  doc: Document,
  entries: TextNodeEntry[],
  map: number[],
  normStart: number,
  normEnd: number,
  className: string
): boolean {
  // Map normalized positions back to original text positions
  const origStart = map[normStart];
  const origEnd = normEnd < map.length ? map[normEnd - 1] + 1 : map[map.length - 1] + 1;

  // Find which text nodes are affected
  const affectedRanges: { entry: TextNodeEntry; localStart: number; localEnd: number }[] = [];

  for (const entry of entries) {
    if (entry.end <= origStart || entry.start >= origEnd) continue;

    const localStart = Math.max(0, origStart - entry.start);
    const localEnd = Math.min(entry.node.nodeValue!.length, origEnd - entry.start);
    affectedRanges.push({ entry, localStart, localEnd });
  }

  if (affectedRanges.length === 0) return false;

  // Wrap each affected text node range. Process in reverse to avoid
  // invalidating positions in earlier nodes.
  for (let i = affectedRanges.length - 1; i >= 0; i--) {
    const { entry, localStart, localEnd } = affectedRanges[i];
    const textNode = entry.node;
    const nodeText = textNode.nodeValue ?? '';

    // Split: [before][highlight][after]
    const before = nodeText.slice(0, localStart);
    const highlight = nodeText.slice(localStart, localEnd);
    const after = nodeText.slice(localEnd);

    const parent = textNode.parentNode;
    if (!parent) continue;

    const frag = doc.createDocumentFragment();

    if (before) {
      frag.appendChild(doc.createTextNode(before));
    }

    const span = doc.createElement('span');
    span.className = className;
    span.textContent = highlight;
    frag.appendChild(span);

    if (after) {
      frag.appendChild(doc.createTextNode(after));
    }

    parent.replaceChild(frag, textNode);
  }

  return true;
}

/** Nearest block-level ancestor, or the body. */
function blockAncestor(node: Node): Node | null {
  let el: Element | null =
    node.nodeType === Node.ELEMENT_NODE ? (node as Element) : node.parentElement;
  while (el && !BLOCK_TAGS.has(el.tagName)) el = el.parentElement;
  return el ?? node.ownerDocument?.body ?? null;
}

/**
 * Collect the text nodes of the sentence the sentence-level highlight already
 * wrapped, in document order, as one string.
 *
 * Working from the existing spans rather than searching for the sentence text
 * again is what makes repeated sentences land right: "Yes." occurs dozens of
 * times in a chapter and a text search always finds the first one, so the word
 * highlight would sit on a different page from the voice.
 *
 * Slices that were adjacent in the DOM stay adjacent here; a gap that held
 * whitespace, or a block boundary, contributes one space — the same separator
 * the synthesised text carries, so token counts agree.
 */
function collectHighlightedNodes(
  doc: Document,
  sentenceClassName: string
): { entries: TextNodeEntry[]; text: string } {
  const spans = Array.from(doc.querySelectorAll(`.${sentenceClassName}`));
  const entries: TextNodeEntry[] = [];
  let text = '';
  let prev: Element | null = null;

  for (const span of spans) {
    if (prev) {
      const gap = doc.createRange();
      gap.setStartAfter(prev);
      gap.setEndBefore(span);
      const gapText = gap.toString();
      if (gapText.length > 0 || blockAncestor(prev) !== blockAncestor(span)) {
        text += ' ';
      }
    }
    const walker = doc.createTreeWalker(span, NodeFilter.SHOW_TEXT);
    let node: Text | null;
    while ((node = walker.nextNode() as Text | null)) {
      const len = node.nodeValue?.length ?? 0;
      if (len === 0) continue;
      entries.push({ node, start: text.length, end: text.length + len });
      text += node.nodeValue ?? '';
    }
    prev = span;
  }

  return { entries, text };
}

/**
 * Highlight the `localWordIndex`-th word of the sentence currently being read.
 *
 * Counts words rather than matching text: the forced aligner reports indices
 * into a whitespace split of the exact string that was synthesised, so the
 * index is authoritative and no fuzzy search is needed to use it.
 *
 * Word highlights live in a different className from sentence highlights so
 * the two can coexist (word nests visually inside sentence). Each call clears
 * any prior word span first, so only one word is ever highlighted at a time.
 * Returns `false` silently when the sentence isn't on the page or the index is
 * out of range — playback keeps going, the highlight just skips that tick
 * rather than landing on the wrong word.
 */
export function highlightWordByIndex(
  doc: Document,
  sentenceText: string,
  localWordIndex: number,
  className: string = 'readalong-word',
  sentenceClassName: string = 'readalong-hl'
): boolean {
  clearHighlights(doc, className);
  if (localWordIndex < 0) return false;

  // The sentence's own spans first; fall back to locating it in the page when
  // the sentence highlight found nothing to wrap.
  const anchored = collectHighlightedNodes(doc, sentenceClassName);
  const useAnchor = anchored.entries.length > 0;
  const { entries, text } = useAnchor ? anchored : collectTextNodes(doc);
  if (entries.length === 0) return false;

  const { normalized: normalizedText, map } = buildNormalizedMap(text);

  // Anchored, the collected text IS the sentence. Otherwise it is the page,
  // and the sentence has to be located in it first.
  let searchStart = 0;
  let searchEnd = normalizedText.length;
  if (!useAnchor) {
    const normalizedSentence = normalize(sentenceText);
    if (!normalizedSentence) return false;
    const found = normalizedText.toLowerCase().indexOf(normalizedSentence.toLowerCase());
    if (found === -1) return false;
    searchStart = found;
    searchEnd = found + normalizedSentence.length;
  }

  // Bracket the localWordIndex-th whitespace-separated word in the range.
  let wordCount = -1;
  let wordStart = -1;
  let wordEnd = -1;
  let inWord = false;
  for (let i = searchStart; i <= searchEnd; i++) {
    const atEnd = i === searchEnd;
    const ch = atEnd ? ' ' : normalizedText[i];
    const isSpace = /\s/.test(ch);
    if (!isSpace && !inWord) {
      inWord = true;
      wordCount += 1;
      if (wordCount === localWordIndex) wordStart = i;
    } else if (isSpace && inWord) {
      inWord = false;
      if (wordCount === localWordIndex) {
        wordEnd = i;
        break;
      }
    }
  }
  if (wordStart === -1 || wordEnd === -1) return false;

  return wrapRange(doc, entries, map, wordStart, wordEnd, className);
}

/**
 * Remove all highlight spans, restoring original text nodes.
 */
export function clearHighlights(doc: Document, className: string = 'readalong-hl'): void {
  const highlights = doc.querySelectorAll(`.${className}`);
  for (const span of highlights) {
    const parent = span.parentNode;
    if (!parent) continue;

    // Replace span with its child nodes
    while (span.firstChild) {
      parent.insertBefore(span.firstChild, span);
    }
    parent.removeChild(span);

    // Merge adjacent text nodes
    parent.normalize();
  }
}

/**
 * Extract plain text from a DOM Range, adding whitespace between block elements.
 * Unlike Range.toString(), this inserts newlines at block boundaries so that
 * sentence splitting works correctly (e.g. "end of paragraph." + "Start of next"
 * won't be merged into "paragraph.Start").
 */
export function getRangeText(range: Range): string {
  const ancestor = range.commonAncestorContainer;
  const doc = ancestor.ownerDocument;
  if (!doc) return range.toString();

  const parts: string[] = [];
  const walker = doc.createTreeWalker(ancestor, NodeFilter.SHOW_ALL);
  let pastEnd = false;

  let node: Node | null = walker.currentNode;
  while (node && !pastEnd) {
    if (!range.intersectsNode(node)) {
      node = walker.nextNode();
      continue;
    }

    if (node.nodeType === Node.ELEMENT_NODE) {
      if (BLOCK_TAGS.has((node as Element).tagName)) {
        parts.push('\n\n');
      }
    } else if (node.nodeType === Node.TEXT_NODE) {
      const text = node.nodeValue;
      if (text) {
        const isStart = node === range.startContainer;
        const isEnd = node === range.endContainer;
        const start = isStart ? range.startOffset : 0;
        const end = isEnd ? range.endOffset : text.length;
        parts.push(text.slice(start, end));
        if (isEnd) pastEnd = true;
      }
    }

    node = walker.nextNode();
  }

  return parts
    .join('')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

/**
 * Extract plain text from the document body (fallback when no visible range is available).
 * Note: In paginated EPUB readers, prefer using getRangeText() with the paginator's
 * visible Range instead — this function returns the entire chapter text.
 */
export function getVisibleText(doc: Document): string {
  const body = doc.body;
  if (!body) return '';

  const parts: string[] = [];

  const walker = doc.createTreeWalker(body, NodeFilter.SHOW_ALL);
  let node: Node | null = walker.currentNode;

  while (node) {
    if (node.nodeType === Node.ELEMENT_NODE) {
      const el = node as Element;
      // Skip hidden elements
      const style = doc.defaultView?.getComputedStyle(el);
      if (style && (style.display === 'none' || style.visibility === 'hidden')) {
        // Skip this subtree
        let next = walker.nextSibling();
        if (!next) next = walker.parentNode();
        node = next;
        continue;
      }
      if (BLOCK_TAGS.has(el.tagName)) {
        parts.push('\n\n');
      }
    } else if (node.nodeType === Node.TEXT_NODE) {
      const text = node.nodeValue;
      if (text) parts.push(text);
    }
    node = walker.nextNode();
  }

  return parts
    .join('')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

/**
 * Given a click position in the document, extract surrounding text that can be
 * matched against known TTS sentences. Uses caretPositionFromPoint to find the
 * text node at the click, then grabs a window of surrounding text.
 *
 * Returns null when the caret APIs return nothing — e.g. in foliate's paginated
 * mode, the iframe element spans the *whole chapter* so click coordinates are
 * in full-chapter (not visible-viewport) space, and some browsers refuse caret
 * lookups outside the visual viewport. Callers should fall back to
 * [getTextAroundClick] using the click target.
 */
export function getTextAtPoint(doc: Document, x: number, y: number): string | null {
  // caretPositionFromPoint is the standard API; caretRangeFromPoint is the legacy fallback
  let container: Node | null = null;
  let offset = 0;

  // eslint-disable-next-line @typescript-eslint/no-explicit-any -- caretPositionFromPoint not yet in all TS DOM libs
  const d = doc as any;
  if (typeof d.caretPositionFromPoint === 'function') {
    const pos = d.caretPositionFromPoint(x, y);
    if (pos) {
      container = pos.offsetNode;
      offset = pos.offset;
    }
  } else if (typeof d.caretRangeFromPoint === 'function') {
    const range = d.caretRangeFromPoint(x, y);
    if (range) {
      container = range.startContainer;
      offset = range.startOffset;
    }
  }

  if (!container || container.nodeType !== Node.TEXT_NODE) return null;

  const nodeText = (container as Text).nodeValue ?? '';
  if (!nodeText.trim()) return null;

  // Expand to ~200 chars around the click point for matching context
  const contextRadius = 100;
  const start = Math.max(0, offset - contextRadius);
  const end = Math.min(nodeText.length, offset + contextRadius);
  return nodeText.slice(start, end);
}

/**
 * Block-level fallback for [getTextAtPoint]: returns the textContent of the
 * nearest block-level ancestor of the click target. Less precise than
 * caret-based but always works (no coordinate translation needed) and gives
 * skipToSentence enough context to pick the right paragraph.
 */
export function getTextAroundClick(target: EventTarget | null): string | null {
  if (!(target instanceof Element)) return null;
  const body = target.ownerDocument?.body;
  let node: Element | null = target;
  while (node && node !== body) {
    if (BLOCK_TAGS.has(node.tagName)) {
      const text = node.textContent?.trim();
      if (text) return text.slice(0, 400);
    }
    node = node.parentElement;
  }
  const text = target.textContent?.trim();
  return text ? text.slice(0, 400) : null;
}

/**
 * Scroll the first highlight element into view if not already visible.
 */
export function scrollToHighlight(doc: Document, className: string = 'readalong-hl'): void {
  const el = doc.querySelector(`.${className}`);
  if (!el) return;

  el.scrollIntoView({ behavior: 'smooth', block: 'center' });
}
