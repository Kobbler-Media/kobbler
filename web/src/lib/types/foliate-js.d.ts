declare module 'foliate-js/view.js' {
  /** One spine item. Only the parts we actually read are modelled. */
  export interface FoliateSection {
    /** `'no'` for non-linear sections (notes, ads) — skipped when sampling. */
    linear?: string;
    /** Parse this section's XHTML without rendering it. */
    createDocument?: () => Promise<Document>;
  }

  export interface FoliateBook {
    sections?: FoliateSection[];
    /**
     * Loader transform hook: fires a `data` event for every resource the book
     * loads, and listeners may replace `detail.data` before it becomes the
     * rendered blob URL. See `kepub-text-overlay.ts`.
     */
    transformTarget?: EventTarget;
    /**
     * EPUB `rendition:*` metadata. `layout: 'pre-paginated'` is what selects
     * the fixed-layout renderer over the reflowable paginator — it is writable
     * because producers routinely ship fixed-layout books that never declare it.
     */
    rendition?: { layout?: string; viewport?: string };
    toc?: Array<{
      label: string;
      href: string;
      subitems?: Array<{ label: string; href: string }>;
    }>;
    metadata?: { title?: string; author?: string; language?: string };
  }

  export function makeBook(file: File | string): Promise<FoliateBook>;
  export class View extends HTMLElement {
    open(book: unknown): Promise<void>;
    init(opts?: { lastLocation?: string }): Promise<void>;
    goTo(target: string): Promise<void>;
    goToFraction(frac: number): Promise<void>;
    renderer: {
      next(): Promise<void>;
      prev(): Promise<void>;
      setAttribute(name: string, value: string): void;
      setStyles?(styles: string | [string, string]): void;
    };
    book: FoliateBook;
  }
}
