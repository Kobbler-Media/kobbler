import { beforeEach, describe, expect, it, vi } from 'vitest';

const api = vi.hoisted(() => ({ getSearchSuggestions: vi.fn() }));

vi.mock('#lib/api/client.js', async (importOriginal) => ({
  ...(await importOriginal<typeof import('#lib/api/client.js')>()),
  ...api
}));

const { useSearchSuggestions, suggestionLabel } = await import('./useSearchSuggestions.svelte');

const parasite = { title: 'Parasite', media_type: 'movie', source: 'tmdb' };
const paradise = { title: 'Paradise', media_type: 'tv', source: 'tmdb' };

function key(name: string) {
  return { key: name, preventDefault: vi.fn() };
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.useFakeTimers();
});

describe('useSearchSuggestions', () => {
  it('asks once per pause in typing, for the media the filter allows', async () => {
    api.getSearchSuggestions.mockResolvedValue([parasite, paradise]);
    const s = useSearchSuggestions({ onPick: vi.fn() });
    s.input('p', ['movies', 'tv', 'books']);
    s.input('pa', ['movies', 'tv', 'books']);
    s.input('par', ['movies']);
    await vi.advanceTimersByTimeAsync(200);
    expect(api.getSearchSuggestions).toHaveBeenCalledTimes(1);
    expect(api.getSearchSuggestions).toHaveBeenCalledWith('par', 'movie');
    expect(s.items).toEqual([parasite, paradise]);
    expect(s.open).toBe(true);
  });

  it.each([
    [['movies', 'tv', 'books'], 'movie,tv,book'],
    [['books'], 'book'],
    [['tv'], 'tv']
  ] as const)('asks for every type searched: %j', async (filters, media) => {
    api.getSearchSuggestions.mockResolvedValue([parasite]);
    const s = useSearchSuggestions({ onPick: vi.fn() });
    s.input('pa', [...filters]);
    await vi.advanceTimersByTimeAsync(200);
    expect(api.getSearchSuggestions).toHaveBeenCalledWith('pa', media);
  });

  it('never asks with nothing to search or blank input, nor keeps a slower stale answer', async () => {
    let resolveFirst: (v: unknown) => void = () => {};
    api.getSearchSuggestions
      .mockReturnValueOnce(new Promise((r) => (resolveFirst = r)))
      .mockResolvedValueOnce([paradise]);
    const s = useSearchSuggestions({ onPick: vi.fn() });
    s.input('pa', []);
    s.input('   ', ['movies', 'tv']);
    await vi.advanceTimersByTimeAsync(200);
    expect(api.getSearchSuggestions).not.toHaveBeenCalled();

    s.input('pa', ['movies', 'tv']);
    await vi.advanceTimersByTimeAsync(200);
    s.input('parad', ['tv']);
    await vi.advanceTimersByTimeAsync(200);
    resolveFirst([parasite]);
    await vi.advanceTimersByTimeAsync(0);
    expect(api.getSearchSuggestions).toHaveBeenLastCalledWith('parad', 'tv');
    expect(s.items).toEqual([paradise]);
  });

  it('moves through the list with arrow keys and picks with Enter', async () => {
    api.getSearchSuggestions.mockResolvedValue([parasite, paradise]);
    const onPick = vi.fn();
    const s = useSearchSuggestions({ onPick });
    s.input('pa', ['movies', 'tv']);
    await vi.advanceTimersByTimeAsync(200);

    expect(s.keydown(key('Enter'))).toBe(false);
    expect(s.keydown(key('ArrowDown'))).toBe(true);
    expect(s.keydown(key('ArrowDown'))).toBe(true);
    expect(s.keydown(key('ArrowDown'))).toBe(true);
    expect(s.highlighted).toBe(0);
    expect(s.keydown(key('ArrowUp'))).toBe(true);
    expect(s.highlighted).toBe(1);
    expect(s.keydown(key('Enter'))).toBe(true);
    expect(onPick).toHaveBeenCalledWith('Paradise');
    expect(s.open).toBe(false);
  });

  it('closes on Escape and drops a reply that lands after closing', async () => {
    api.getSearchSuggestions.mockResolvedValue([parasite]);
    const s = useSearchSuggestions({ onPick: vi.fn() });
    s.input('pa', ['movies', 'tv']);
    await vi.advanceTimersByTimeAsync(200);
    expect(s.keydown(key('Escape'))).toBe(true);
    expect(s.open).toBe(false);

    s.input('par', ['movies', 'tv']);
    s.close();
    await vi.advanceTimersByTimeAsync(200);
    expect(s.open).toBe(false);
  });

  it('stays quiet when the request fails', async () => {
    api.getSearchSuggestions.mockRejectedValue(new Error('down'));
    const s = useSearchSuggestions({ onPick: vi.fn() });
    s.input('pa', ['movies', 'tv']);
    await vi.advanceTimersByTimeAsync(200);
    expect(s.items).toEqual([]);
    expect(s.open).toBe(false);
  });

  it.each([
    [{ title: 'Dune', media_type: 'movie', source: 'tmdb' }, 'Movie'],
    [{ title: 'Bluey', media_type: 'tv', source: 'library' }, 'TV · In library'],
    [{ title: 'Dune', media_type: 'book', source: 'hardcover' }, 'Book'],
    [{ title: 'Frank Herbert', media_type: 'author', source: 'library' }, 'Author · In library']
  ] as const)('labels %j', (item, label) => {
    expect(suggestionLabel(item)).toBe(label);
  });
});
