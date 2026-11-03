<script lang="ts">
  import { onMount, onDestroy, untrack } from 'svelte';
  import {
    highlightSentence as highlightText,
    highlightWordByIndex,
    clearHighlights,
    getVisibleText,
    getRangeText,
    getTextAtPoint,
    getTextAroundClick
  } from '#lib/utils/text-highlight.js';
  import { installKoboTextOverlay } from '#lib/utils/kepub-text-overlay.js';
  import type { FoliateBook } from 'foliate-js/view.js';

  interface Props {
    fileUrl: string;
    initialCfi?: string | null;
    initialTheme?: 'light' | 'dark' | 'sepia';
    onRelocate?: (detail: { progress: number; cfi: string; tocItem?: { label: string } }) => void;
    onLoaded?: (detail: { toc: TocItem[]; metadata: BookMetadata }) => void;
    onSentenceTap?: (contextText: string) => void;
  }

  interface TocItem {
    label: string;
    href: string;
    subitems?: TocItem[];
  }

  interface BookMetadata {
    title?: string;
    author?: string;
    language?: string;
  }

  let {
    fileUrl,
    initialCfi = null,
    initialTheme = 'light',
    onRelocate,
    onLoaded,
    onSentenceTap
  }: Props = $props();

  let containerEl: HTMLDivElement;
  let view: HTMLElement & {
    open: (book: unknown) => Promise<void>;
    init: (opts: { lastLocation?: string }) => Promise<void>;
    goTo: (target: string) => Promise<void>;
    goToFraction: (frac: number) => Promise<void>;
    close: () => void;
    renderer: {
      next: () => Promise<void>;
      prev: () => Promise<void>;
      setAttribute: (name: string, value: string) => void;
      setStyles?: (styles: string | [string, string]) => void;
      /** Flip to the page holding an element or Range. Emits a relocate. */
      scrollToAnchor?: (anchor: Node | Range, select?: boolean) => Promise<void>;
      size?: number;
      start?: number;
      scrolled?: boolean;
    };
    /** Recomputed by foliate on every relocate; `range` is the visible range. */
    lastLocation?: { range?: Range | null; cfi?: string };
    /** True when foliate chose the fixed-layout renderer over the paginator. */
    isFixedLayout?: boolean;
    book: {
      toc?: TocItem[];
      metadata?: BookMetadata;
      /** Spine items. Read ahead of the rendered page for fixed layout. */
      sections?: Array<{ linear?: string; createDocument?: () => Promise<Document> }>;
    };
  };

  let theme = $state<'light' | 'dark' | 'sepia'>(untrack(() => initialTheme));
  let fontSize = $state(100);
  let iframeDoc = $state<Document | null>(null);
  let visibleRange = $state<Range | null>(null);

  // Swipe gesture tracking
  const SWIPE_THRESHOLD = 50; // min px to count as swipe
  const SWIPE_MAX_Y = 100; // max vertical drift
  // Tap-zone center dead-zone: ignore taps in the middle 20% to avoid
  // accidental page turns when reaching for something else.
  const TAP_DEAD_ZONE_LOW = 0.4;
  const TAP_DEAD_ZONE_HIGH = 0.6;
  let touchStartX = 0;
  let touchStartY = 0;
  let touchTracking = false;
  // Set true when a touchend resolved to a page turn, so the ensuing
  // synthetic click doesn't also turn a page (double-advance).
  let swipeHandled = false;

  function handleTouchStart(e: TouchEvent) {
    if (e.touches.length !== 1) return;
    touchStartX = e.touches[0].clientX;
    touchStartY = e.touches[0].clientY;
    touchTracking = true;
    swipeHandled = false;
  }

  function handleTouchEnd(e: TouchEvent) {
    if (!touchTracking || !view?.renderer) return;
    touchTracking = false;
    const touch = e.changedTouches[0];
    if (!touch) return;
    const dx = touch.clientX - touchStartX;
    const dy = Math.abs(touch.clientY - touchStartY);
    if (dy > SWIPE_MAX_Y || Math.abs(dx) < SWIPE_THRESHOLD) return;
    swipeHandled = true;
    if (dx < 0) {
      view.renderer.next();
    } else {
      view.renderer.prev();
    }
  }

  // hl: TTS read-along highlight color, tuned per theme so it's actually visible.
  // The previous single 0.12-alpha blue was nearly invisible on every theme.
  const THEME_STYLES: Record<string, { bg: string; fg: string; hl: string }> = {
    light: { bg: '#ffffff', fg: '#1a1a1a', hl: 'rgba(255, 217, 61, 0.55)' },
    dark: { bg: '#000000', fg: '#cccccc', hl: 'rgba(98, 158, 230, 0.45)' },
    sepia: { bg: '#f4ecd8', fg: '#5b4636', hl: 'rgba(193, 122, 50, 0.35)' }
  };

  function applyTheme() {
    if (!view?.renderer) return;
    const s = THEME_STYLES[theme];
    view.renderer.setAttribute('flow', 'paginated');

    // Style the foliate-view element background to match the theme.
    // Also set --_margin/--_gap to 0 so the paginator grid has no visible
    // margin frame. These CSS custom properties inherit through the closed
    // shadow DOM boundary into the paginator's #top element.
    const el = view as HTMLElement;
    el.style.background = s.bg;
    el.style.setProperty('--_margin', '0px');
    el.style.setProperty('--_gap', '0%');

    // setStyles injects CSS into the book iframe. Set --theme-bg-color so
    // the paginator's #replaceBackground reads it and colors its background
    // columns to match our theme instead of the epub's original background.
    view.renderer.setStyles?.(`
      html {
        --theme-bg-color: ${s.bg};
      }
      html, body {
        color: ${s.fg} !important;
        background: ${s.bg} !important;
      }
      body {
        zoom: ${fontSize / 100} !important;
        line-height: 1.6 !important;
        max-width: 700px !important;
        margin: 0 auto !important;
      }
      a { color: inherit !important; }
      .readalong-hl {
        background: ${s.hl};
        border-radius: 2px;
        box-shadow: 0 0 0 1px ${s.hl};
        transition: background 0.15s ease;
      }
      /* Current word highlight from the forced aligner. Nests inside
         .readalong-hl so both fire together; we use weight + underline
         instead of background to avoid visual stacking. */
      .readalong-word {
        font-weight: 700;
        text-decoration: underline;
        text-decoration-thickness: 2px;
        text-underline-offset: 2px;
      }
    `);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (!view?.renderer) return;
    switch (e.key) {
      case 'ArrowRight':
      case ' ':
        e.preventDefault();
        view.renderer.next();
        break;
      case 'ArrowLeft':
        e.preventDefault();
        view.renderer.prev();
        break;
    }
  }

  // In foliate's paginated mode the iframe element spans the full chapter width
  // (all columns laid out horizontally) and a parent container scrolls to show
  // one column at a time. So `clientX` inside the iframe is in full-chapter
  // coordinates, not visible-page coordinates. Use the renderer's page size and
  // scroll offset to map back to the visible page.
  function clickToPageRatio(clientX: number, fallbackWidth: number): number {
    const r = view?.renderer;
    const size = r?.size;
    const start = r?.start;
    if (!r?.scrolled && typeof size === 'number' && size > 0 && typeof start === 'number') {
      return (clientX - start) / size;
    }
    return clientX / fallbackWidth;
  }

  function handleTapZone(e: MouseEvent) {
    if (!view?.renderer) return;
    // Synthetic click after a swipe — already handled by touchend.
    if (swipeHandled) {
      swipeHandled = false;
      return;
    }
    // Outer container clicks (margins around the foliate-view shadow DOM) are
    // already in visible-viewport coordinates, so a plain ratio is correct.
    const rect = containerEl.getBoundingClientRect();
    const x = (e.clientX - rect.left) / rect.width;
    if (x >= TAP_DEAD_ZONE_LOW && x <= TAP_DEAD_ZONE_HIGH) return;
    if (x < TAP_DEAD_ZONE_LOW) {
      view.renderer.prev();
    } else {
      view.renderer.next();
    }
  }

  export function goTo(target: string) {
    view?.goTo(target);
  }

  export function nextPage() {
    view?.renderer?.next();
  }

  export function prevPage() {
    view?.renderer?.prev();
  }

  export function setFontSize(size: number) {
    fontSize = size;
    applyTheme();
  }

  export function setReaderTheme(t: 'light' | 'dark' | 'sepia') {
    theme = t;
    applyTheme();
  }

  export function getIframeDoc(): Document | null {
    return iframeDoc;
  }

  /**
   * Where in the chapter the last highlighted sentence was found. Read-along
   * runs forward, so the next search starts here rather than at the top of the
   * chapter — see `highlightSentence` in text-highlight.ts. Reset whenever a
   * new document loads.
   */
  let sentenceSearchCursor = 0;
  /**
   * The sections making up the spread on screen, from foliate's `load` events.
   *
   * A fixed-layout book on a wide screen renders TWO sections at once, each in
   * its own iframe, so "the current document" is only half of what the reader
   * can see — and it is whichever half loaded last. Read-along used to take
   * that document as the page and silently skip the other one. Collected here
   * instead: `relocate` closes the set, and the next `load` starts a new one,
   * which holds whichever order the renderer emits them in.
   */
  let spreadSections: number[] = [];
  let spreadClosed = true;
  /** The documents of those sections, in the order they were loaded. */
  let spreadDocs: Document[] = [];
  /**
   * The document the sentence highlight is currently living in. On a two-up
   * spread that is not necessarily `iframeDoc`, which is whichever half loaded
   * last — half the sentences being read are in the other one.
   */
  let highlightDoc: Document | null = null;
  /** Sentence to re-highlight once the page being turned to has loaded. */
  let pendingSentence: string | null = null;
  /** Consecutive page turns made looking for a sentence, reset on a hit. */
  let sentenceMisses = 0;
  /**
   * How far to chase a sentence that isn't on the current page before giving
   * up. Two pages covers the real case (the span ran past the page); more
   * would let a sentence that is simply unmatchable walk the reader through
   * the book while the audio plays on.
   */
  const MAX_SENTENCE_MISSES = 2;

  export function highlightSentence(sentence: string): void {
    if (!iframeDoc) return;
    // Every document on screen is a candidate. In paginated mode that is one
    // and this is the same search it always was; on a fixed-layout spread it
    // is both pages, and which of them foliate loaded last means nothing.
    const candidates = spreadDocs.length > 0 ? spreadDocs : [iframeDoc];
    for (const doc of candidates) {
      const from = doc === highlightDoc ? sentenceSearchCursor : 0;
      let at = highlightText(doc, sentence, 'readalong-hl', from);
      // Not ahead of the cursor: either the reader jumped backwards or the
      // sentence sits before it. Start over rather than lose the highlight.
      if (at < 0 && from > 0) at = highlightText(doc, sentence, 'readalong-hl', 0);
      if (at < 0) continue;
      // The losing candidates each cleared their own highlight on the way
      // through, so only this one is lit.
      if (doc !== highlightDoc) clearOtherHighlights(doc);
      highlightDoc = doc;
      sentenceSearchCursor = at;
      sentenceMisses = 0;
      revealHighlight();
      return;
    }
    // Nowhere on this page. In a fixed-layout book each page is a separate
    // document, so a span that runs past the page simply isn't here yet —
    // turn to the next one and finish the highlight when it loads.
    if (view?.isFixedLayout && sentenceMisses < MAX_SENTENCE_MISSES) {
      sentenceMisses += 1;
      pendingSentence = sentence;
      nextPage();
    }
  }

  /**
   * Bring the highlighted text onto the screen.
   *
   * Read-along synthesises a span that runs past the end of the visible page,
   * so the page has to follow the voice rather than the other way round. Only
   * moves when the highlight is genuinely off-page: `scrollToAnchor` emits a
   * relocate every time it is called, and calling it on every highlight would
   * fire one per sentence for text already on screen.
   */
  /** Drop any highlight left in the OTHER documents of this spread. */
  function clearOtherHighlights(keep: Document): void {
    for (const doc of spreadDocs) {
      if (doc === keep) continue;
      clearHighlights(doc);
      clearHighlights(doc, 'readalong-word');
    }
  }

  function revealHighlight(): void {
    const doc = highlightDoc ?? iframeDoc;
    if (!doc || !view) return;
    const el = doc.querySelector('.readalong-hl');
    if (!el) return;
    const visible = view.lastLocation?.range;
    if (visible instanceof Range && visible.intersectsNode(el)) return;
    void view.renderer.scrollToAnchor?.(el);
  }

  /**
   * Highlight the `localWordIndex`-th word inside `sentenceText`.
   * `sentenceText` is the same string the most recent `highlightSentence`
   * was called with, so the FA word index lands inside the already-highlighted
   * sentence range (no fuzzy text matching).
   */
  export function highlightWord(localWordIndex: number, sentenceText: string): void {
    const doc = highlightDoc ?? iframeDoc;
    if (!doc) return;
    highlightWordByIndex(doc, sentenceText, localWordIndex);
  }

  export function clearHighlight(): void {
    for (const doc of new Set([...spreadDocs, iframeDoc])) {
      if (!doc) continue;
      clearHighlights(doc);
      clearHighlights(doc, 'readalong-word');
    }
    highlightDoc = null;
  }

  /**
   * Text of the page currently on screen — the read-along's input.
   *
   * Prefers foliate's LIVE `lastLocation.range` over the copy cached at the
   * last relocate, so a range we somehow missed an event for can't leave us
   * reading a stale page.
   *
   * The whole-document fallback is only valid for fixed layout, where the
   * document IS the page. In PAGINATED mode the iframe holds the entire
   * chapter, most of it scrolled out of view, so falling back to it there
   * hands the read-along the whole chapter from its beginning — the reader
   * then narrates text that is not on screen, which is exactly what it used
   * to do. Better to report no text and let the caller move on.
   */
  export function getPageText(): string {
    const live = view?.lastLocation?.range;
    const range = live instanceof Range ? live : visibleRange;
    if (range) {
      return getRangeText(range);
    }
    if (iframeDoc && view?.isFixedLayout) {
      return getVisibleText(iframeDoc);
    }
    return '';
  }

  /**
   * Text to read aloud starting at the current page, running on to the end of
   * the chapter.
   *
   * Read-along used to request one page at a time, which cost more than a
   * request per page: the engine's long-form mode carries the previous chunk's
   * generated audio forward as the reference for the next one, so voice, tone
   * and pacing hold across chunk seams — but only WITHIN one request. A request
   * per page reset that at every turn, which is what made the narrator sound
   * like a different reader after each page.
   *
   * In paginated mode the iframe already holds the whole chapter, so the span
   * costs nothing extra to extract: take everything from the start of the
   * visible page to the end of the document. The caller decides how much of it
   * to synthesise at once — audio for a whole chapter would sit in memory as
   * decoded AudioBuffers, and the stream arrives faster than it plays.
   *
   * Fixed-layout books get the page and nothing more: there, one document IS
   * one page.
   */
  export async function getSpanText(maxWords: number): Promise<string> {
    if (view?.isFixedLayout) return fixedLayoutSpanText(maxWords);

    const doc = iframeDoc;
    const live = view?.lastLocation?.range;
    const range = live instanceof Range ? live : visibleRange;
    const body = doc?.body;
    if (!doc || !range || !body) return getPageText();

    const toEnd = doc.createRange();
    toEnd.setStart(range.startContainer, range.startOffset);
    toEnd.setEnd(body, body.childNodes.length);
    return getRangeText(toEnd);
  }

  /**
   * The same span for a fixed-layout book, where one document is one page.
   *
   * Nothing here can be taken from the rendered iframe: the pages ahead are
   * separate documents that have not been rendered. They are parsed straight
   * out of the book instead — cheap, and it is the same call the layout sniff
   * already makes. Non-linear sections are skipped, the way a reader skips
   * them.
   */
  async function fixedLayoutSpanText(maxWords: number): Promise<string> {
    const sections = view?.book?.sections ?? [];
    // Start at the FIRST section on screen, not the last one loaded: on a
    // two-up spread the other page is already in front of the reader. With no
    // load seen there is no way to know where "here" is, and starting at
    // section 0 would read the book from the beginning — take the rendered
    // page and nothing more.
    if (spreadSections.length === 0) return getPageText();
    const start = Math.min(...spreadSections);

    const parts: string[] = [];
    let words = 0;
    for (let i = start; i < sections.length && words < maxWords; i++) {
      const section = sections[i];
      if (section.linear === 'no' || !section.createDocument) continue;
      try {
        const doc = await section.createDocument();
        const text = getVisibleText(doc);
        if (!text) continue;
        parts.push(text);
        words += text.split(/\s+/).filter(Boolean).length;
      } catch {
        break;
      }
    }
    return parts.join('\n\n');
  }

  /**
   * Whether a book is fixed-layout without declaring it.
   *
   * The test is the same one foliate's fixed-layout renderer itself relies on:
   * a viewport meta carrying a numeric width AND height. That pair is what an
   * FXL document states and what a reflowable one does not — the reflowable
   * spelling is `width=device-width`, which is not a number — so a false
   * positive needs a reflowable book to declare a pixel page size it doesn't
   * have. Failing the sniff just leaves the book reflowable, as today.
   */
  async function looksFixedLayout(book: FoliateBook): Promise<boolean> {
    const section = book.sections?.find((s) => s.linear !== 'no') ?? book.sections?.[0];
    if (!section?.createDocument) return false;
    try {
      const doc = await section.createDocument();
      const content = doc.querySelector('meta[name="viewport"]')?.getAttribute('content');
      if (!content) return false;
      const dims = Object.fromEntries(
        content
          .split(/[,;\s]+/)
          .filter(Boolean)
          .map((pair) => pair.split('=').map((x) => x.trim()))
      );
      return Number(dims.width) > 0 && Number(dims.height) > 0;
    } catch {
      // A book we can't pre-read is one we render the way we always did.
      return false;
    }
  }

  onMount(async () => {
    const { makeBook, View } = await import('foliate-js/view.js');

    // Register custom element if not already registered
    if (!customElements.get('foliate-view')) {
      customElements.define('foliate-view', View);
    }

    view = document.createElement('foliate-view') as typeof view;
    // eslint-disable-next-line svelte/no-dom-manipulating -- foliate-js requires imperative DOM control
    containerEl.append(view);

    // Style the view to fill container
    (view as HTMLElement).style.cssText = 'width: 100%; height: 100%;';

    // Load the book
    const res = await fetch(fileUrl, { credentials: 'include' });
    if (!res.ok) throw new Error(`Failed to fetch book: ${res.status}`);
    const blob = await res.blob();
    const file = new File([blob], 'book.epub', { type: 'application/epub+zip' });
    const book = await makeBook(file);

    // KEPUB gen-3 books (KF8 full-page-background pages) carry their text in a
    // `hidden` layer below the artwork — invisible and below the fold. Patch
    // it into a page overlay before any section document is rendered.
    installKoboTextOverlay(book);

    // Some producers ship a page-image book — text absolutely positioned over a
    // full-page background — without ever writing `rendition:layout` into the
    // OPF. foliate then reflows 1424px pages into a ~600px column and the book
    // arrives as loose lines scattered down the screen. Declaring the layout
    // ourselves is what routes it to the fixed-layout renderer instead.
    if (await looksFixedLayout(book)) {
      book.rendition = { ...(book.rendition ?? {}), layout: 'pre-paginated' };
    }

    // Attach keyboard handlers to each iframe document as it loads,
    // since the iframe steals focus and svelte:window won't receive keys.
    view.addEventListener('load', ((e: CustomEvent) => {
      const doc = e.detail?.doc as Document | undefined;
      iframeDoc = doc ?? null;
      sentenceSearchCursor = 0;
      if (typeof e.detail?.index === 'number') {
        if (spreadClosed) {
          spreadSections = [];
          spreadDocs = [];
          spreadClosed = false;
        }
        spreadSections.push(e.detail.index);
        if (doc) spreadDocs.push(doc);
      }
      // A sentence the read-along could not find because it lives on a later
      // page of a fixed-layout book — this is the page it was waiting for.
      if (pendingSentence && doc) {
        const sentence = pendingSentence;
        pendingSentence = null;
        highlightSentence(sentence);
      }
      if (doc) {
        doc.addEventListener('keydown', handleKeydown);
        doc.addEventListener('touchstart', handleTouchStart, { passive: true });
        doc.addEventListener('touchend', handleTouchEnd);
        doc.addEventListener('click', (ce: MouseEvent) => {
          // Skip if user is selecting text
          const sel = doc.getSelection();
          if (sel && sel.toString().length > 0) return;

          // Synthetic click after a swipe — already handled by touchend.
          if (swipeHandled) {
            swipeHandled = false;
            return;
          }

          // Click-to-turn lives in the edge zones (left/right), regardless of
          // TTS. The previous behaviour swallowed every text tap as a sentence
          // skip in TTS mode, leaving no way to turn pages by tapping.
          // In paginated mode the iframe spans the entire chapter, so map the
          // click into visible-page coordinates via the foliate renderer.
          const x = clickToPageRatio(ce.clientX, doc.documentElement.clientWidth);
          if (x < TAP_DEAD_ZONE_LOW) {
            view?.renderer?.prev();
            return;
          }
          if (x > TAP_DEAD_ZONE_HIGH) {
            view?.renderer?.next();
            return;
          }

          // Center zone: in TTS mode, skip to the tapped sentence. Caret-based
          // lookup gives sub-sentence precision when it works; we fall back to
          // the click target's block-level ancestor when foliate's wide iframe
          // puts the click outside the visual viewport (browsers vary on
          // whether caretPositionFromPoint accepts that).
          if (onSentenceTap) {
            const text =
              getTextAtPoint(doc, ce.clientX, ce.clientY) ?? getTextAroundClick(ce.target);
            if (text) onSentenceTap(text);
          }
        });
      }
    }) as (e: Event) => void);

    view.addEventListener('relocate', ((e: CustomEvent) => {
      const detail = e.detail;
      // Store the visible range from foliate's paginator for getPageText()
      visibleRange = detail?.range instanceof Range ? detail.range : null;
      spreadClosed = true;
      if (onRelocate && detail) {
        const fraction = detail.fraction ?? 0;
        const cfi = detail.cfi ?? '';
        const tocItem = detail.tocItem;
        onRelocate({
          progress: Math.round(fraction * 10000) / 100,
          cfi,
          tocItem: tocItem ? { label: tocItem.label ?? '' } : undefined
        });
      }
    }) as (e: Event) => void);

    await view.open(book);
    applyTheme();

    if (onLoaded && view.book) {
      onLoaded({
        toc: view.book.toc ?? [],
        metadata: view.book.metadata ?? {}
      });
    }

    await view.init({ lastLocation: initialCfi ?? undefined });
  });

  onDestroy(() => {
    // close() tears down foliate's paginator — including its ResizeObservers —
    // which are otherwise left running when the node is detached and spam
    // "ResizeObserver loop completed with undelivered notifications" forever.
    view?.close?.();
    if (view && containerEl?.contains(view)) {
      // eslint-disable-next-line svelte/no-dom-manipulating -- cleanup imperative DOM
      containerEl.removeChild(view);
    }
  });
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="epub-reader"
  class:theme-dark={theme === 'dark'}
  class:theme-sepia={theme === 'sepia'}
  bind:this={containerEl}
  role="application"
  tabindex="-1"
  style="background: {THEME_STYLES[theme].bg}"
  onclick={handleTapZone}
  onkeydown={handleKeydown}
  ontouchstart={handleTouchStart}
  ontouchend={handleTouchEnd}
></div>

<style>
  .epub-reader {
    width: 100%;
    height: 100%;
    position: relative;
    overflow: hidden;
    transition: background 0.2s ease;
  }
</style>
