/**
 * Who this player is, for cross-device handoff.
 *
 * The id only has to tell this page's own position saves apart from another
 * device's, for as long as this page is open — so it is minted per page load and
 * never stored. A player also ignores saves older than when it opened, which is
 * what keeps a reload from mistaking its previous self for another device.
 */

function mintId(): string {
  // randomUUID needs a secure context, and Kobbler is routinely served over
  // plain http on a LAN.
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    try {
      return crypto.randomUUID();
    } catch {
      // fall through
    }
  }
  return `web-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

const clientId = mintId();

function browserLabel(): string {
  if (typeof navigator === 'undefined') return 'Browser';
  const ua = navigator.userAgent;
  const browser = /Edg\//.test(ua)
    ? 'Edge'
    : /Firefox\//.test(ua)
      ? 'Firefox'
      : /Chrome\//.test(ua)
        ? 'Chrome'
        : /Safari\//.test(ua)
          ? 'Safari'
          : 'Browser';
  const os = /Android/.test(ua)
    ? 'Android'
    : /iPhone|iPad|iPod/.test(ua)
      ? 'iOS'
      : /Mac OS X/.test(ua)
        ? 'macOS'
        : /Windows/.test(ua)
          ? 'Windows'
          : /Linux/.test(ua)
            ? 'Linux'
            : null;
  return os ? `${browser} on ${os}` : browser;
}

export function playbackClientId(): string {
  return clientId;
}

/** Sent with every position save; the server relays it to the user's other players. */
export function playbackClient(): { client_id: string; client_label: string } {
  return { client_id: clientId, client_label: browserLabel() };
}
