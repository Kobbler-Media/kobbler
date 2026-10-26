import { getSearchSuggestions } from '$lib/api/client';
import type { SearchSuggestion } from '$lib/types';

const DEBOUNCE_MS = 150;

const KIND: Record<SearchSuggestion['media_type'], string> = {
  movie: 'Movie',
  tv: 'TV',
  book: 'Book',
  author: 'Author',
  livetv: 'Live TV',
  iptv: 'IPTV'
};

export function suggestionLabel(item: SearchSuggestion): string {
  const kind = KIND[item.media_type];
  return item.source === 'library' ? `${kind} · In library` : kind;
}

export type SuggestionFilter = 'movies' | 'tv' | 'books';

const MEDIA: Record<SuggestionFilter, string> = {
  movies: 'movie',
  tv: 'tv',
  books: 'book'
};

export function useSearchSuggestions(opts: { onPick: (title: string) => void }) {
  let items = $state<SearchSuggestion[]>([]);
  let highlighted = $state(-1);
  let open = $state(false);
  let timer: ReturnType<typeof setTimeout> | null = null;
  let generation = 0;

  function close() {
    generation++;
    if (timer) clearTimeout(timer);
    timer = null;
    open = false;
    highlighted = -1;
  }

  function input(query: string, filters: SuggestionFilter[]) {
    close();
    const media = filters.map((f) => MEDIA[f]).join(',');
    const q = query.trim();
    if (!media || !q) {
      items = [];
      return;
    }
    const current = generation;
    timer = setTimeout(async () => {
      try {
        const found = await getSearchSuggestions(q, media);
        if (current !== generation) return;
        items = found;
        open = found.length > 0;
      } catch {
        if (current === generation) items = [];
      }
    }, DEBOUNCE_MS);
  }

  function pick(title: string) {
    close();
    opts.onPick(title);
  }

  function keydown(e: Pick<KeyboardEvent, 'key' | 'preventDefault'>): boolean {
    if (!open) return false;
    switch (e.key) {
      case 'ArrowDown':
        highlighted = (highlighted + 1) % items.length;
        break;
      case 'ArrowUp':
        highlighted = highlighted <= 0 ? items.length - 1 : highlighted - 1;
        break;
      case 'Enter':
        if (highlighted < 0) return false;
        pick(items[highlighted].title);
        break;
      case 'Escape':
        close();
        break;
      default:
        return false;
    }
    e.preventDefault();
    return true;
  }

  return {
    get items() {
      return items;
    },
    get highlighted() {
      return highlighted;
    },
    get open() {
      return open;
    },
    input,
    keydown,
    pick,
    close
  };
}
