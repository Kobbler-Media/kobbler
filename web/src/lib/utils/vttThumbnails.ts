/**
 * Parse a WebVTT thumbnails file into an array of thumbnail entries.
 *
 * Each entry maps a time range to a region within a sprite sheet.
 * Format: "sprite_001.jpg#xywh=0,0,320,180"
 */
export interface ThumbnailEntry {
  start: number;
  end: number;
  url: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * Parse WebVTT thumbnail file content into structured entries.
 */
export function parseVttThumbnails(vttContent: string, baseUrl: string): ThumbnailEntry[] {
  const entries: ThumbnailEntry[] = [];
  const lines = vttContent.split('\n');
  let i = 0;

  // Skip WEBVTT header
  while (i < lines.length && !lines[i].includes('-->')) {
    i++;
  }

  while (i < lines.length) {
    const line = lines[i].trim();
    if (line.includes('-->')) {
      const [startStr, endStr] = line.split('-->').map((s) => s.trim());
      const start = parseVttTime(startStr);
      const end = parseVttTime(endStr);

      i++;
      if (i < lines.length) {
        const urlLine = lines[i].trim();
        if (urlLine) {
          const parsed = parseSpriteRef(urlLine, baseUrl);
          if (parsed) {
            entries.push({ start, end, ...parsed });
          }
        }
      }
    }
    i++;
  }

  return entries;
}

/**
 * Find the thumbnail entry for a given time using binary search.
 */
export function findThumbnailAtTime(
  entries: ThumbnailEntry[],
  time: number
): ThumbnailEntry | null {
  if (entries.length === 0) return null;

  let lo = 0;
  let hi = entries.length - 1;

  while (lo <= hi) {
    const mid = (lo + hi) >>> 1;
    const entry = entries[mid];
    if (time < entry.start) {
      hi = mid - 1;
    } else if (time >= entry.end) {
      lo = mid + 1;
    } else {
      return entry;
    }
  }

  return null;
}

function parseVttTime(str: string): number {
  const parts = str.split(':');
  if (parts.length === 3) {
    const [h, m, s] = parts;
    return parseInt(h) * 3600 + parseInt(m) * 60 + parseFloat(s);
  } else if (parts.length === 2) {
    const [m, s] = parts;
    return parseInt(m) * 60 + parseFloat(s);
  }
  return parseFloat(str) || 0;
}

function parseSpriteRef(
  ref: string,
  baseUrl: string
): { url: string; x: number; y: number; w: number; h: number } | null {
  const [filename, fragment] = ref.split('#');
  if (!fragment?.startsWith('xywh=')) return null;

  const [x, y, w, h] = fragment.replace('xywh=', '').split(',').map(Number);

  // Build full URL relative to VTT base — thumbnails.vtt → thumbnails/{filename}
  const base = baseUrl.replace(/\.vtt$/, '/');
  const url = `${base}${filename}`;

  return { url, x, y, w, h };
}
