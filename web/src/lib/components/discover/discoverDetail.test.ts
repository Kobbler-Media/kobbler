import { describe, expect, it } from 'vitest';
import type { DiscoverEpisode } from '$lib/api/client';
import { episodeCode, parseDiscoverItem, streamTarget } from './discoverDetail';

const season = (season_number: number, episode_count?: number) => ({
  season_number,
  episode_count
});

const ep = (episode_number: number): DiscoverEpisode => ({
  episode_number,
  title: null,
  air_date: null,
  description: null,
  still_url: null,
  runtime: null
});

describe('streamTarget', () => {
  it.each<[string, ReturnType<typeof season>[], Map<number, DiscoverEpisode[]>, unknown]>([
    [
      'starts at S01E01',
      [season(0, 3), season(1, 10), season(2, 8)],
      new Map(),
      { season: 1, episode: 1 }
    ],
    [
      'skips seasons with no episodes',
      [season(1, 0), season(2, 8)],
      new Map(),
      { season: 2, episode: 1 }
    ],
    [
      'uses the first episode the season actually lists',
      [season(1, 10)],
      new Map([[1, [ep(3), ep(2)]]]),
      { season: 1, episode: 2 }
    ],
    [
      'falls back to specials when nothing else exists',
      [season(0, 4)],
      new Map(),
      { season: 0, episode: 1 }
    ],
    ['has nothing to stream without seasons', [], new Map(), null]
  ])('%s', (_name, seasons, episodes, target) => {
    expect(streamTarget(seasons, episodes)).toEqual(target);
  });

  it.each([
    [2, { season: 2, episode: 1 }],
    [9, { season: 1, episode: 1 }]
  ])('starts at season %i when the link asked for it and it exists', (preferred, target) => {
    expect(streamTarget([season(1, 3), season(2, 3)], new Map(), preferred)).toEqual(target);
  });
});

describe('episodeCode', () => {
  it.each([
    [1, 1, 'S01E01'],
    [12, 103, 'S12E103']
  ])('S%iE%i', (s, e, code) => {
    expect(episodeCode(s, e)).toBe(code);
  });
});

describe('parseDiscoverItem', () => {
  it('prefers the loaded detail and falls back to the card preview', () => {
    const item = parseDiscoverItem(
      {
        media_type: 'tv',
        provider: 'tmdb',
        details: { title: 'Show', overview: 'About', seasons: [season(1, 2)], logo_url: 'logo' }
      },
      { media_type: 'tv', provider: 'tmdb', provider_id: '1', title: 'Old', poster_url: 'p' }
    );
    expect(item).toMatchObject({
      title: 'Show',
      description: 'About',
      posterUrl: 'p',
      logoUrl: 'logo',
      seasons: [season(1, 2)]
    });
  });

  it('reads a movie release date for the availability strip', () => {
    const details = { title: 'Film', release_date: '2026-12-04' };
    expect(
      parseDiscoverItem({ media_type: 'movie', provider: 'tmdb', details }, null).releaseDate
    ).toBe('2026-12-04');
    expect(parseDiscoverItem(null, null).releaseDate).toBeNull();
  });

  it.each<[string, unknown, unknown]>([
    [
      'reads per-type release dates',
      { region: 'AU', theatrical: '2024-02-01', digital: null, premiere: 5 },
      {
        region: 'AU',
        premiere: null,
        theatrical_limited: null,
        theatrical: '2024-02-01',
        digital: null,
        physical: null,
        tv: null
      }
    ],
    ['ignores a missing object', undefined, null],
    ['ignores an object without a region', { theatrical: '2024-02-01' }, null]
  ])('%s', (_name, release_dates, expected) => {
    const details = { title: 'Film', release_dates };
    expect(
      parseDiscoverItem({ media_type: 'movie', provider: 'tmdb', details }, null).releaseDates
    ).toEqual(expected);
  });
});
