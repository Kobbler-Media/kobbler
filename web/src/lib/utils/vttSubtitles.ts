/**
 * Parse a WebVTT subtitle file into cue entries for use with the VTTCue API.
 *
 * Handles standard VTT: multi-line cues, HH:MM:SS.mmm and MM:SS.mmm timestamps,
 * positioning metadata after timestamps (stripped). Preserves VTT formatting tags
 * (<b>, <i>, <u>) since VTTCue renders them natively.
 */
export interface VttCue {
  start: number;
  end: number;
  text: string;
}

export function parseVtt(content: string): VttCue[] {
  const cues: VttCue[] = [];
  const blocks = content.split(/\n\n+/);

  for (const block of blocks) {
    const lines = block.trim().split('\n');

    // Find the timestamp line
    let timeLine = -1;
    for (let i = 0; i < lines.length; i++) {
      if (lines[i].includes('-->')) {
        timeLine = i;
        break;
      }
    }
    if (timeLine === -1) continue;

    const [startStr, rest] = lines[timeLine].split('-->');
    if (!rest) continue;

    // End timestamp is the first token; everything after is positioning metadata
    const endStr = rest.trim().split(/\s+/)[0];
    const start = parseTimestamp(startStr.trim());
    const end = parseTimestamp(endStr);

    const text = lines
      .slice(timeLine + 1)
      .filter((l) => l.trim())
      .join('\n');
    if (!text) continue;

    cues.push({ start, end, text });
  }

  return cues;
}

function parseTimestamp(str: string): number {
  const parts = str.split(':');
  if (parts.length === 3) {
    return parseInt(parts[0]) * 3600 + parseInt(parts[1]) * 60 + parseFloat(parts[2]);
  }
  if (parts.length === 2) {
    return parseInt(parts[0]) * 60 + parseFloat(parts[1]);
  }
  return parseFloat(str) || 0;
}
