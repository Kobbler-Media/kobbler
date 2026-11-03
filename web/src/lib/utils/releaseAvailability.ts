import type { StreamOption } from '#lib/api/client.js';
import type { ReleaseDates } from '#lib/types.js';
import { isActivelyWanted } from './want-status';

export type StreamingAvailability = 'hidden' | 'checking' | 'instant' | 'available' | 'none';
export type LibraryStatus = 'wanted' | 'in_library' | 'not_in_library';

export interface AvailabilityRow {
  label: string;
  value: string;
  emphasised: boolean;
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

const STREAMING_LABELS: Record<Exclude<StreamingAvailability, 'hidden'>, [string, boolean]> = {
  checking: ['Checking…', false],
  instant: ['Ready to stream', true],
  available: ['Available', true],
  none: ['Not yet', false]
};

const LIBRARY_LABELS: Record<LibraryStatus, [string, boolean]> = {
  wanted: ['Wanted', true],
  in_library: ['In library', true],
  not_in_library: ['Not in library', false]
};

export function todayIso(now = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}

function dateLabel(iso: string): string | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})/.exec(iso);
  if (!m) return null;
  return `${MONTHS[Number(m[2]) - 1]} ${Number(m[3])}, ${m[1]}`;
}

function datedRow(label: string, iso: string | null, today: string): AvailabilityRow | null {
  const date = iso ? dateLabel(iso) : null;
  if (!date) return null;
  const upcoming = iso!.slice(0, 10) > today;
  return { label, value: upcoming ? `Coming ${date}` : date, emphasised: upcoming };
}

function releaseRow(releaseDate: string | null, year: number | null, today: string) {
  return (
    datedRow('Release', releaseDate, today) ?? {
      label: 'Release',
      value: year ? String(year) : '—',
      emphasised: false
    }
  );
}

function typedReleaseRows(dates: ReleaseDates, today: string): AvailabilityRow[] {
  return [
    datedRow('Cinema', dates.theatrical ?? dates.theatrical_limited ?? dates.premiere, today),
    datedRow('Digital', dates.digital, today),
    datedRow('Physical', dates.physical, today)
  ].filter((r) => r !== null);
}

export function releaseAvailability(input: {
  releaseDate: string | null;
  releaseDates: ReleaseDates | null;
  year: number | null;
  today: string;
  streaming: StreamingAvailability;
  library: LibraryStatus;
}): AvailabilityRow[] {
  const typed = input.releaseDates ? typedReleaseRows(input.releaseDates, input.today) : [];
  const rows = typed.length > 0 ? typed : [releaseRow(input.releaseDate, input.year, input.today)];
  if (input.streaming !== 'hidden') {
    const [value, emphasised] = STREAMING_LABELS[input.streaming];
    rows.push({ label: 'Streaming', value, emphasised });
  }
  const [value, emphasised] = LIBRARY_LABELS[input.library];
  rows.push({ label: 'Library', value, emphasised });
  return rows;
}

function regionName(code: string): string {
  try {
    return new Intl.DisplayNames(['en'], { type: 'region' }).of(code) ?? code;
  } catch {
    return code;
  }
}

export function releaseRegionNote(
  dates: ReleaseDates | null,
  userRegion: string | null
): string | null {
  if (!dates || typedReleaseRows(dates, '').length === 0) return null;
  const region = dates.region.toUpperCase();
  if (userRegion && userRegion.toUpperCase() === region) return null;
  return `Dates for ${regionName(region)}`;
}

export function browserRegion(languages: readonly string[]): string | null {
  for (const tag of languages) {
    try {
      const region = new Intl.Locale(tag).region;
      if (region) return region;
    } catch {
      continue;
    }
  }
  return null;
}

export function streamingAvailability(
  canStream: boolean,
  searching: boolean,
  found: Pick<StreamOption, 'cached' | 'availability'>[] | null
): StreamingAvailability {
  if (!canStream) return 'hidden';
  if (found === null) return searching ? 'checking' : 'hidden';
  if (found.some((o) => o.cached || o.availability === 'ready')) return 'instant';
  return found.length > 0 ? 'available' : 'none';
}

export function libraryStatus(
  wantStatus: string | null | undefined,
  inLibrary: boolean | null | undefined
): LibraryStatus {
  if (inLibrary) return 'in_library';
  return isActivelyWanted(wantStatus) ? 'wanted' : 'not_in_library';
}
