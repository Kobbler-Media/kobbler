import { tick } from 'svelte';

/**
 * Scroll helpers for the paginated grid pages (library, movies, tv, sports).
 *
 * The app layout scrolls `main.content`, not the window, so SvelteKit's own scroll
 * handling never touches it: after a `goto()` the container keeps whatever scrollTop
 * it had. Every list that swaps its window under the reader has to manage that itself.
 */

/** Cards across the grid pages tag themselves with one of these id attributes. */
const ITEM_ID_ATTRS = [
  'data-book-id',
  'data-item-id',
  'data-author-id',
  'data-series-id',
  'data-person-id'
] as const;

const ITEM_SELECTOR = ITEM_ID_ATTRS.map((attr) => `[${attr}]`).join(', ');

export interface ScrollAnchor {
  /** Entity id of the anchored item. */
  id: string;
  /** Viewport-relative top the item had when it was captured. */
  offset: number;
}

/** The scrollable element the grid lives in (`main.content`, or the document as fallback). */
export function getScrollContainer(): HTMLElement {
  const main = document.querySelector('main.content');
  if (main instanceof HTMLElement) {
    const style = getComputedStyle(main);
    if (style.overflowY === 'auto' || style.overflowY === 'scroll') {
      return main;
    }
  }
  return document.documentElement;
}

function findItem(id: string): Element | null {
  const selector = ITEM_ID_ATTRS.map((attr) => `[${attr}="${CSS.escape(id)}"]`).join(', ');
  return document.querySelector(selector);
}

/** Record the first (partially) visible grid item so the view can be pinned across a reload. */
export function captureScrollAnchor(): ScrollAnchor | null {
  for (const item of document.querySelectorAll(ITEM_SELECTOR)) {
    const rect = item.getBoundingClientRect();
    if (rect.top < window.innerHeight && rect.bottom > 0) {
      for (const attr of ITEM_ID_ATTRS) {
        const id = item.getAttribute(attr);
        if (id) return { id, offset: rect.top };
      }
      return null;
    }
  }
  return null;
}

/** Wait for Svelte's DOM update plus a completed layout pass. */
async function afterLayout(): Promise<void> {
  await tick();
  await new Promise<void>((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  });
}

/** Put the anchored item back where it was. No-op if it is gone from the new window. */
export async function restoreScrollAnchor(anchor: ScrollAnchor | null): Promise<void> {
  if (!anchor) return;
  await afterLayout();
  const item = findItem(anchor.id);
  if (!item) return;
  const diff = item.getBoundingClientRect().top - anchor.offset;
  if (Math.abs(diff) > 1) {
    getScrollContainer().scrollTop += diff;
  }
}

/** Scroll back to the top of the list — the landing spot for a fresh `around_before=0` window. */
export async function scrollContainerToTop(): Promise<void> {
  await afterLayout();
  getScrollContainer().scrollTop = 0;
}

/** Bring one item to the top of the scroll container; falls back to the top of the list. */
export async function scrollItemToTop(id: string): Promise<void> {
  await afterLayout();
  const container = getScrollContainer();
  const item = findItem(id);
  if (!item) {
    container.scrollTop = 0;
    return;
  }
  const containerTop =
    container === document.documentElement ? 0 : container.getBoundingClientRect().top;
  container.scrollTop += item.getBoundingClientRect().top - containerTop;
}
