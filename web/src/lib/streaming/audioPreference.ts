import type { PlaybackAudioTrack, PlaybackMethod } from '#lib/types.js';

export interface StartingAudio {
  active: number | null;
  switchTo: number | null;
}

// Without a preference from the server, its default flag already is the
// preferred-language pick.
function preferredIndex(tracks: PlaybackAudioTrack[], preferred: number | null | undefined) {
  if (preferred != null && tracks.some((t) => t.stream_index === preferred)) return preferred;
  return tracks.find((t) => t.is_default)?.stream_index ?? null;
}

function browserIndex(tracks: PlaybackAudioTrack[]) {
  return (tracks.find((t) => t.is_default) ?? tracks[0])?.stream_index ?? null;
}

// A browser cannot pick the audio track of a file it plays directly, so a
// preferred track other than the file's default needs the server to remux.
export function startingAudio(
  method: PlaybackMethod,
  tracks: PlaybackAudioTrack[],
  preferred: number | null | undefined
): StartingAudio {
  const want = preferredIndex(tracks, preferred);
  if (method !== 'direct_play') return { active: want, switchTo: null };
  const active = browserIndex(tracks);
  return { active, switchTo: want !== null && want !== active ? want : null };
}
