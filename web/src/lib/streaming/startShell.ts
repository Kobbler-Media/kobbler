export interface StartView {
  debrid: boolean;
  live: boolean;
  sessionReady: boolean;
  streamStarted: boolean;
  startHeld: boolean;
  failed: boolean;
  casting: boolean;
}

// Only debrid has a resolve and a download to wait through; a library file opens on
// the ordinary spinner, as kobapp does.
export function showsStartShell(v: StartView): boolean {
  if (!v.debrid) return false;
  if (!v.sessionReady) return true;
  return !v.streamStarted && !v.startHeld && !v.failed && !v.casting && !v.live;
}
