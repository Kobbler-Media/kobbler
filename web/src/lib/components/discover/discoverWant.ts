import {
  createBook,
  createMovie,
  createTvShow,
  setTvShowMonitored,
  wantDiscoverEpisode,
  wantMovie
} from '#lib/api/client.js';
import { autoCreateFor, type DiscoverItem, type EpisodeRef } from './discoverDetail';

/** Add the title to the library as wanted; resolves to the library page to land on. */
export async function wantDiscoverItem(mediaType: string, item: DiscoverItem): Promise<string> {
  switch (mediaType) {
    case 'movie': {
      const { id } = await createMovie({
        title: item.title,
        year: item.year,
        description: item.description,
        poster_url: item.posterUrl,
        backdrop_url: item.backdropUrl,
        identifiers: item.identifiers,
        genres: item.genres,
        rating: item.rating,
        rating_count: item.ratingCount,
        runtime: item.runtime,
        content_rating: null
      });
      await wantMovie(id);
      return `/movies/${id}`;
    }
    case 'tv': {
      const { id } = await createTvShow({
        title: item.title,
        year: item.year,
        description: item.description,
        poster_url: item.posterUrl,
        identifiers: item.identifiers,
        genres: item.genres,
        network: item.network
      });
      await setTvShowMonitored(id, true, 'all');
      return `/tv/${id}`;
    }
    default: {
      const { id } = await createBook({
        title: item.title,
        authors: item.authors.map((name) => ({ name })),
        description: item.description,
        cover_url: item.posterUrl,
        identifiers: item.identifiers,
        rating: item.rating,
        rating_count: item.ratingCount,
        want: true
      });
      return `/library/books/${id}`;
    }
  }
}

export async function wantDiscoverEpisodeRef(
  item: DiscoverItem,
  provider: string,
  providerId: string,
  ref: EpisodeRef
): Promise<string> {
  const { title, ...rest } = autoCreateFor(item, provider, providerId, null);
  const result = await wantDiscoverEpisode({
    ...rest,
    title,
    provider,
    provider_id: providerId,
    season_number: ref.season,
    episode_number: ref.episode
  });
  return `/tv/${result.show_id}`;
}
