import type { DiscoverDetailResponse, DiscoverEpisode, StreamResolveParams } from '$lib/api/client';
import type { DiscoverPreview } from '$lib/stores/discover-preview.svelte';
import type { ReleaseDates } from '$lib/types';

export interface DiscoverSeason {
  season_number: number;
  episode_count?: number;
  air_date?: string;
  title?: string;
}

export interface DiscoverCastMember {
  name: string;
  character?: string;
  profile_url?: string;
}

export interface DiscoverItem {
  title: string;
  year: number | null;
  description: string | null;
  tagline: string | null;
  posterUrl: string | null;
  backdropUrl: string | null;
  logoUrl: string | null;
  genres: string[];
  rating: number | null;
  ratingCount: number | null;
  runtime: number | null;
  releaseDate: string | null;
  releaseDates: ReleaseDates | null;
  status: string | null;
  network: string | null;
  contentRating: string | null;
  identifiers: Record<string, string>;
  cast: DiscoverCastMember[];
  seasons: DiscoverSeason[];
  authors: string[];
  publisher: string | null;
  pageCount: number | null;
  isbn: string | null;
}

export interface EpisodeRef {
  season: number;
  episode: number;
}

export function parseDiscoverItem(
  detail: DiscoverDetailResponse | null,
  preview: DiscoverPreview | null
): DiscoverItem {
  const d = detail?.details ?? {};
  const str = (k: string) => (typeof d[k] === 'string' ? (d[k] as string) : null);
  const num = (k: string) => (typeof d[k] === 'number' ? (d[k] as number) : null);
  const list = <T>(k: string) => (Array.isArray(d[k]) ? (d[k] as T[]) : null);
  return {
    title: str('title') ?? preview?.title ?? '',
    year: num('year') ?? preview?.year ?? null,
    description: str('description') ?? str('overview') ?? preview?.description ?? null,
    tagline: str('tagline'),
    posterUrl: str('poster_url') ?? preview?.poster_url ?? null,
    backdropUrl: str('backdrop_url') ?? preview?.backdrop_url ?? null,
    logoUrl: str('logo_url'),
    genres: list<string>('genres') ?? preview?.genres ?? [],
    rating: num('rating') ?? preview?.rating ?? null,
    ratingCount: num('rating_count') ?? preview?.rating_count ?? null,
    runtime: num('runtime'),
    releaseDate: str('release_date'),
    releaseDates: parseReleaseDates(d.release_dates),
    status: str('status'),
    network: str('network'),
    contentRating: str('content_rating'),
    identifiers:
      (d.identifiers as Record<string, string> | undefined) ?? preview?.identifiers ?? {},
    cast: list<DiscoverCastMember>('cast') ?? [],
    seasons: list<DiscoverSeason>('seasons') ?? [],
    authors: list<string>('authors') ?? [],
    publisher: str('publisher'),
    pageCount: num('page_count'),
    isbn: str('isbn')
  };
}

const RELEASE_TYPES = [
  'premiere',
  'theatrical_limited',
  'theatrical',
  'digital',
  'physical',
  'tv'
] as const;

function parseReleaseDates(value: unknown): ReleaseDates | null {
  if (!value || typeof value !== 'object') return null;
  const o = value as Record<string, unknown>;
  if (typeof o.region !== 'string') return null;
  const date = (k: string) => (typeof o[k] === 'string' ? (o[k] as string) : null);
  return {
    region: o.region,
    ...(Object.fromEntries(RELEASE_TYPES.map((k) => [k, date(k)])) as Omit<ReleaseDates, 'region'>)
  };
}

const seasonOrder = (n: number) => (n === 0 ? Number.MAX_SAFE_INTEGER : n);

export function listedSeasons(seasons: DiscoverSeason[]): DiscoverSeason[] {
  return [...seasons].sort((a, b) => seasonOrder(a.season_number) - seasonOrder(b.season_number));
}

/** First episode of the asked-for or lowest real season; specials only when nothing else exists. */
export function streamTarget(
  seasons: DiscoverSeason[],
  episodes: ReadonlyMap<number, DiscoverEpisode[]>,
  preferredSeason: number | null = null
): EpisodeRef | null {
  const sorted = listedSeasons(seasons);
  const season =
    sorted.find((s) => s.season_number === preferredSeason) ??
    sorted.find((s) => s.season_number > 0 && s.episode_count !== 0) ??
    sorted[0];
  if (!season) return null;
  const loaded = episodes.get(season.season_number);
  const first = loaded?.length ? Math.min(...loaded.map((e) => e.episode_number)) : 1;
  return { season: season.season_number, episode: first };
}

export function episodeCode(season: number, episode: number): string {
  return `S${String(season).padStart(2, '0')}E${String(episode).padStart(2, '0')}`;
}

export function autoCreateFor(
  item: DiscoverItem,
  provider: string,
  providerId: string,
  episode: EpisodeRef | null
): NonNullable<StreamResolveParams['auto_create']> {
  return {
    title: item.title,
    year: item.year ?? undefined,
    identifiers: item.identifiers,
    description: item.description ?? undefined,
    poster_url: item.posterUrl ?? undefined,
    backdrop_url: item.backdropUrl ?? undefined,
    genres: item.genres.length > 0 ? item.genres : undefined,
    rating: item.rating ?? undefined,
    rating_count: item.ratingCount ?? undefined,
    runtime: item.runtime ?? undefined,
    network: item.network ?? undefined,
    ...(episode
      ? {
          provider,
          provider_id: providerId,
          season_number: episode.season,
          episode_number: episode.episode
        }
      : {})
  };
}
