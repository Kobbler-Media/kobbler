import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { DiscoverItem } from './discoverDetail';

const client = vi.hoisted(() => ({
  addDiscoverToWatchlist: vi.fn(),
  createMovie: vi.fn(),
  createTvShow: vi.fn(),
  createBook: vi.fn(),
  setTvShowMonitored: vi.fn(),
  wantDiscoverEpisode: vi.fn(),
  wantMovie: vi.fn()
}));
vi.mock('#lib/api/client.js', () => client);

const { watchlistDiscoverItem } = await import('./discoverWant');

const item: DiscoverItem = {
  title: 'Some Day',
  year: 2026,
  description: null,
  tagline: null,
  posterUrl: 'https://img/poster.jpg',
  backdropUrl: null,
  logoUrl: null,
  genres: [],
  rating: null,
  ratingCount: null,
  runtime: 45,
  releaseDate: null,
  releaseDates: null,
  status: null,
  network: 'HBO',
  contentRating: null,
  identifiers: { tmdb_id: '77' },
  cast: [],
  seasons: [],
  authors: [],
  publisher: null,
  pageCount: null,
  isbn: null
};

describe('watchlistDiscoverItem', () => {
  beforeEach(() => vi.clearAllMocks());

  it.each([
    ['tv', 'show', '/tv/lib-1'],
    ['movie', 'movie', '/movies/lib-1']
  ])('puts a %s on the list and lands on its library page', async (mediaType, stored, route) => {
    client.addDiscoverToWatchlist.mockResolvedValue({ media_type: stored, media_id: 'lib-1' });

    await expect(watchlistDiscoverItem(mediaType, item, 'tmdb', '77')).resolves.toBe(route);

    expect(client.addDiscoverToWatchlist).toHaveBeenCalledWith(
      expect.objectContaining({
        media_type: mediaType,
        provider: 'tmdb',
        provider_id: '77',
        title: 'Some Day',
        identifiers: { tmdb_id: '77' }
      })
    );
    expect(client.setTvShowMonitored).not.toHaveBeenCalled();
    expect(client.wantMovie).not.toHaveBeenCalled();
  });
});
