import { describe, expect, it } from 'vitest';
import {
  browserRegion,
  libraryStatus,
  releaseAvailability,
  releaseRegionNote,
  streamingAvailability,
  type LibraryStatus,
  type StreamingAvailability
} from './releaseAvailability';
import type { ReleaseDates } from '$lib/types';

const today = '2026-10-21';

function rows(
  over: Partial<{
    releaseDate: string | null;
    releaseDates: ReleaseDates | null;
    year: number | null;
    streaming: StreamingAvailability;
    library: LibraryStatus;
  }> = {}
) {
  return Object.fromEntries(
    releaseAvailability({
      releaseDate: null,
      releaseDates: null,
      year: 1999,
      today,
      streaming: 'hidden',
      library: 'not_in_library',
      ...over
    }).map((r) => [r.label, r.value])
  );
}

describe('releaseAvailability', () => {
  it('reads the release as a date, an upcoming one as coming, and falls back to the year', () => {
    expect(rows({ releaseDate: '1999-03-31' }).Release).toBe('Mar 31, 1999');
    expect(rows({ releaseDate: '2026-12-04' }).Release).toBe('Coming Dec 4, 2026');
    expect(rows().Release).toBe('1999');
    expect(rows({ year: null }).Release).toBe('—');
  });

  it('emphasises only an upcoming release', () => {
    const [upcoming] = releaseAvailability({
      releaseDate: '2026-12-04',
      releaseDates: null,
      year: 2026,
      today,
      streaming: 'hidden',
      library: 'wanted'
    });
    expect(upcoming.emphasised).toBe(true);
  });

  it.each<[StreamingAvailability, string | undefined]>([
    ['hidden', undefined],
    ['checking', 'Checking…'],
    ['instant', 'Ready to stream'],
    ['available', 'Available'],
    ['none', 'Not yet']
  ])('says streaming is %s', (streaming, label) => {
    expect(rows({ streaming }).Streaming).toBe(label);
  });

  it.each<[LibraryStatus, string]>([
    ['wanted', 'Wanted'],
    ['in_library', 'In library'],
    ['not_in_library', 'Not in library']
  ])('names the library status %s', (library, label) => {
    expect(rows({ library }).Library).toBe(label);
  });
});

const dates = (over: Partial<ReleaseDates> = {}): ReleaseDates => ({
  region: 'AU',
  premiere: null,
  theatrical_limited: null,
  theatrical: null,
  digital: null,
  physical: null,
  tv: null,
  ...over
});

describe('releaseAvailability with release types', () => {
  it.each<[string, ReleaseDates, Record<string, string | undefined>]>([
    [
      'shows cinema, digital and physical',
      dates({
        premiere: '2024-01-02',
        theatrical: '2024-02-01',
        digital: '2024-04-10',
        physical: '2024-06-01'
      }),
      {
        Release: undefined,
        Cinema: 'Feb 1, 2024',
        Digital: 'Apr 10, 2024',
        Physical: 'Jun 1, 2024'
      }
    ],
    [
      'falls back to a limited run for cinema',
      dates({ premiere: '2024-01-02', theatrical_limited: '2024-01-20' }),
      { Cinema: 'Jan 20, 2024', Digital: undefined, Physical: undefined }
    ],
    [
      'falls back to the premiere for cinema',
      dates({ premiere: '2024-01-02', tv: '2024-09-09' }),
      { Cinema: 'Jan 2, 2024' }
    ],
    [
      'says when a release type is coming',
      dates({ theatrical: '2026-09-01', digital: '2026-12-04' }),
      { Cinema: 'Sep 1, 2026', Digital: 'Coming Dec 4, 2026', Physical: undefined }
    ],
    [
      'omits cinema when only home release is known',
      dates({ digital: '2024-04-10' }),
      { Cinema: undefined, Digital: 'Apr 10, 2024' }
    ],
    [
      'keeps the single release row when no type is dated',
      dates({ tv: '2024-09-09' }),
      { Release: 'Mar 31, 1999', Cinema: undefined }
    ]
  ])('%s', (_name, releaseDates, expected) => {
    const actual = rows({ releaseDate: '1999-03-31', releaseDates });
    expect(Object.fromEntries(Object.keys(expected).map((k) => [k, actual[k]]))).toEqual(expected);
  });

  it('keeps streaming and library after the release types, emphasising only upcoming ones', () => {
    const result = releaseAvailability({
      releaseDate: '2024-02-01',
      releaseDates: dates({ theatrical: '2024-02-01', digital: '2026-12-04' }),
      year: 2024,
      today,
      streaming: 'none',
      library: 'wanted'
    });
    expect(result.map((r) => [r.label, r.emphasised])).toEqual([
      ['Cinema', false],
      ['Digital', true],
      ['Streaming', false],
      ['Library', true]
    ]);
  });
});

describe('releaseRegionNote', () => {
  it.each<[string, ReleaseDates | null, string | null, string | null]>([
    ['is quiet without release types', null, 'AU', null],
    ['is quiet when the dates are local', dates({ digital: '2024-04-10' }), 'AU', null],
    ['ignores region case', dates({ digital: '2024-04-10' }), 'au', null],
    [
      'names a foreign region',
      dates({ region: 'US', digital: '2024-04-10' }),
      'AU',
      'Dates for United States'
    ],
    [
      'names the region when the local one is unknown',
      dates({ digital: '2024-04-10' }),
      null,
      'Dates for Australia'
    ],
    ['is quiet when nothing is dated', dates({ region: 'US' }), 'AU', null]
  ])('%s', (_name, releaseDates, userRegion, note) => {
    expect(releaseRegionNote(releaseDates, userRegion)).toBe(note);
  });
});

describe('browserRegion', () => {
  it.each<[string[], string | null]>([
    [['en-AU', 'en'], 'AU'],
    [['en', 'fr-CA'], 'CA'],
    [['en'], null],
    [['not a tag'], null],
    [[], null]
  ])('%j is %s', (languages, region) => {
    expect(browserRegion(languages)).toBe(region);
  });
});

describe('streamingAvailability', () => {
  const opt = (cached: boolean, availability?: 'ready' | 'uncached') => ({ cached, availability });

  it.each<[string, boolean, boolean, ReturnType<typeof opt>[] | null, StreamingAvailability]>([
    ['hidden without streaming', false, true, null, 'hidden'],
    ['checking while the search runs', true, true, null, 'checking'],
    ['hidden before any search', true, false, null, 'hidden'],
    ['none when the search found nothing', true, false, [], 'none'],
    ['instant when a release is cached', true, false, [opt(true), opt(false)], 'instant'],
    ['instant when a release is ready', true, false, [opt(false, 'ready')], 'instant'],
    ['available when nothing is warm', true, false, [opt(false, 'uncached')], 'available']
  ])('%s', (_, canStream, searching, found, expected) => {
    expect(streamingAvailability(canStream, searching, found)).toBe(expected);
  });
});

describe('libraryStatus', () => {
  it.each<[string | null | undefined, boolean | null | undefined, LibraryStatus]>([
    ['wanted', false, 'wanted'],
    ['searching', false, 'wanted'],
    ['none', true, 'in_library'],
    [null, null, 'not_in_library']
  ])('%s / in library %s is %s', (want, inLibrary, expected) => {
    expect(libraryStatus(want, inLibrary)).toBe(expected);
  });
});
