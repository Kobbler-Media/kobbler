/**
 * KEPUB text-layer fix (koblibs kepub generation 3).
 *
 * A fixed-layout KF8 kids' book rewrites each full-page-background page as an
 * SVG artwork plus a `hidden` text layer stacked in normal flow AFTER it. In
 * the 600x914 fixed-layout viewport the layer starts at y=914 — below the
 * fold — and `hidden` makes it `display: none` via the UA stylesheet, so
 * foliate renders the artwork and never the text (which is real DOM, present
 * but invisible).
 *
 * The text must overlay the artwork exactly: the page's text blocks are
 * absolutely positioned in %/px against the `.fs` 600x914 box, which lines up
 * 1:1 with the SVG's `viewBox="0 0 600 914"`. So the layer is unhidden and
 * pinned to the page:
 *
 *     position: absolute; top: 0; left: 0; width: 100%; height: 100%;
 *
 * Nothing above the layer is positioned, so its containing block is the
 * initial containing block — exactly the 600x914 page in foliate's
 * fixed-layout renderer, which sizes the iframe to the viewport and scales
 * the whole rendered page with a CSS transform. Overlay and artwork therefore
 * align 1:1 and scale together under zoom, with no drift.
 *
 * Applied through foliate's own transform hook: the EPUB loader dispatches a
 * `data` event on `book.transformTarget` for every resource it loads, and a
 * listener may replace `detail.data` before the content becomes the iframe's
 * blob URL (foliate's own paginator uses the same hook to rewrite CSS). The
 * patch runs once per section document. `section.createDocument()` — used for
 * TTS text extraction and search — reads the original zip entry and is
 * untouched, so the `hidden` layer keeps its meaning there.
 */

/** Class name koblibs gives the hidden text layer of a rewritten KF8 page. */
const KOBO_TEXT_LAYER = 'kobo-page-text';

/** Overlay positioning. `display` is left alone: a div is block by default. */
const OVERLAY_STYLE = 'position:absolute;top:0;left:0;width:100%;height:100%;';

/**
 * Patch one serialized section document. Documents without the marker — the
 * common case — are returned untouched.
 */
export function patchKoboTextLayer(html: string): string {
  if (!html.includes(KOBO_TEXT_LAYER)) return html;
  const doc = new DOMParser().parseFromString(html, 'application/xhtml+xml');
  if (doc.querySelector('parsererror')) return html;
  const layers = doc.querySelectorAll(`.${KOBO_TEXT_LAYER}`);
  if (layers.length === 0) return html;
  for (const layer of layers) {
    const existing = layer.getAttribute('style');
    if (existing?.includes(OVERLAY_STYLE)) continue; // already patched
    layer.removeAttribute('hidden');
    layer.setAttribute('style', existing ? `${existing};${OVERLAY_STYLE}` : OVERLAY_STYLE);
  }
  return new XMLSerializer().serializeToString(doc);
}

/**
 * Register the patch on a foliate book's transform target. Call after
 * `makeBook()`, before `view.open(book)`, so every section document is
 * patched before it is rendered.
 */
export function installKoboTextOverlay(book: { transformTarget?: EventTarget }): void {
  const target = book.transformTarget;
  if (!target) return;
  target.addEventListener('data', (event: Event) => {
    const detail = (event as CustomEvent).detail as { data: unknown; type: string } | undefined;
    if (!detail) return;
    detail.data = Promise.resolve(detail.data).then((data) => {
      if (typeof data !== 'string') return data;
      if (detail.type !== 'application/xhtml+xml' && detail.type !== 'text/html') return data;
      return patchKoboTextLayer(data);
    });
  });
}
