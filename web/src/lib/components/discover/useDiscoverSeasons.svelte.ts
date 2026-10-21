import { SvelteMap, SvelteSet } from 'svelte/reactivity';
import { getDiscoverSeason, type DiscoverEpisode } from '$lib/api/client';

export function useDiscoverSeasons(provider: string, providerId: string) {
  const episodes = new SvelteMap<number, DiscoverEpisode[]>();
  const loading = new SvelteSet<number>();
  const failed = new SvelteSet<number>();

  async function load(season: number) {
    if (episodes.has(season) || loading.has(season)) return;
    loading.add(season);
    try {
      const detail = await getDiscoverSeason(provider, providerId, season);
      episodes.set(season, detail.episodes);
    } catch {
      failed.add(season);
    } finally {
      loading.delete(season);
    }
  }

  return {
    episodes: episodes as ReadonlyMap<number, DiscoverEpisode[]>,
    isLoading: (season: number) => loading.has(season),
    hasFailed: (season: number) => failed.has(season),
    load
  };
}
