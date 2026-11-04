/**
 * Google Cast Sender SDK integration.
 *
 * Lazily loads the Cast SDK and provides session management.
 * Works with both Chromecast dongles and Google TV devices.
 */

import { receiverReported } from './capabilities';
import { CAST_NAMESPACE, type CastMessage, type InitSessionPayload } from './messages';

let sdkLoaded = false;
let sdkLoading = false;
let castContext: cast.framework.CastContext | null = null;

// Callbacks for state changes
type StateListener = (connected: boolean, deviceName: string | null, resumed: boolean) => void;
type MessageListener = (msg: CastMessage) => void;

const stateListeners: StateListener[] = [];
const messageListeners: MessageListener[] = [];

/**
 * Load the Cast sender SDK and initialize the framework.
 * Safe to call multiple times — only loads once.
 */
export async function initCastSdk(receiverAppId: string): Promise<boolean> {
  if (sdkLoaded) return true;
  if (sdkLoading) {
    // Wait for existing load
    return new Promise((resolve) => {
      const check = setInterval(() => {
        if (sdkLoaded) {
          clearInterval(check);
          resolve(true);
        }
      }, 100);
      // Timeout after 10s
      setTimeout(() => {
        clearInterval(check);
        resolve(false);
      }, 10000);
    });
  }

  sdkLoading = true;

  try {
    // Set the callback BEFORE loading the script — the SDK calls it once ready
    const apiReady = new Promise<void>((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error('Cast SDK timeout')), 10000);

      // If already available (script was cached / loaded by another path)
      if (window.chrome?.cast && window.cast?.framework) {
        clearTimeout(timeout);
        resolve();
        return;
      }

      (
        window as Window & { __onGCastApiAvailable?: (available: boolean) => void }
      ).__onGCastApiAvailable = (isAvailable: boolean) => {
        clearTimeout(timeout);
        if (isAvailable) resolve();
        else reject(new Error('Cast API not available'));
      };
    });

    await loadScript('https://www.gstatic.com/cv/js/sender/v1/cast_sender.js?loadCastFramework=1');
    await apiReady;

    // Initialize Cast context
    castContext = cast.framework.CastContext.getInstance();
    castContext.setOptions({
      receiverApplicationId: receiverAppId,
      autoJoinPolicy: chrome.cast.AutoJoinPolicy.ORIGIN_SCOPED
    });

    // Listen for session state changes
    castContext.addEventListener(
      cast.framework.CastContextEventType.SESSION_STATE_CHANGED,
      (event: cast.framework.SessionStateEventData) => {
        const connected =
          event.sessionState === cast.framework.SessionState.SESSION_STARTED ||
          event.sessionState === cast.framework.SessionState.SESSION_RESUMED;
        const resumed = event.sessionState === cast.framework.SessionState.SESSION_RESUMED;

        const session = castContext?.getCurrentSession();
        const deviceName = connected ? (session?.getCastDevice()?.friendlyName ?? null) : null;

        for (const listener of stateListeners) {
          listener(connected, deviceName, resumed);
        }

        receiverReported(null);
        // Set up message listener on new sessions
        if (connected && session) {
          session.addMessageListener(CAST_NAMESPACE, (_ns: string, msgStr: string) => {
            try {
              const msg: CastMessage = JSON.parse(msgStr);
              if (msg.type === 'CAPABILITIES') receiverReported(msg.payload);
              for (const listener of messageListeners) {
                listener(msg);
              }
            } catch {
              // Ignore parse errors
            }
          });
          sendMessage({ type: 'CAPABILITIES_REQUEST' });
        }
      }
    );

    sdkLoaded = true;
    sdkLoading = false;
    return true;
  } catch (err) {
    console.error('[Cast] SDK load failed:', err);
    sdkLoading = false;
    return false;
  }
}

/**
 * Request a Cast session (shows the device picker).
 */
export async function requestSession(): Promise<boolean> {
  if (!castContext) return false;
  try {
    await castContext.requestSession();
    return true;
  } catch {
    return false;
  }
}

/**
 * End the current Cast session.
 */
export function endSession(): void {
  castContext?.endCurrentSession(true);
}

/**
 * Send an INIT_SESSION message to the receiver and load media.
 */
export function loadMedia(payload: InitSessionPayload, positionSeconds: number): void {
  const session = castContext?.getCurrentSession();
  if (!session) return;

  // Determine media URL and content type based on playback method
  const contentUrl = payload.playlistUrl || payload.directUrl;
  if (!contentUrl) return;

  const contentType = payload.method === 'direct_play' ? 'video/mp4' : 'application/x-mpegURL';
  const mediaInfo = new chrome.cast.media.MediaInfo(contentUrl, contentType);

  // Set metadata
  const metadata = new chrome.cast.media.GenericMediaMetadata();
  metadata.title = payload.metadata.title;
  if (payload.metadata.subtitle) {
    metadata.subtitle = payload.metadata.subtitle;
  }
  if (payload.metadata.posterUrl) {
    metadata.images = [new chrome.cast.Image(payload.metadata.posterUrl)];
  }
  mediaInfo.metadata = metadata;

  // Attach Kobbler session data as customData for the receiver
  mediaInfo.customData = { kobblerSession: payload };

  const request = new chrome.cast.media.LoadRequest(mediaInfo);
  request.currentTime = Math.max(0, positionSeconds - payload.startOffset);

  session.loadMedia(request).then(
    () => console.log('[Cast] Media loaded'),
    (err: chrome.cast.Error) => console.error('[Cast] Load failed:', err)
  );

  // Also send via custom namespace for redundancy
  sendMessage({ type: 'INIT_SESSION', payload });
}

/**
 * Send a message to the receiver via custom namespace.
 */
export function sendMessage(msg: CastMessage): void {
  const session = castContext?.getCurrentSession();
  if (!session) return;

  session.sendMessage(CAST_NAMESPACE, msg).catch((err: Error) => {
    console.error('[Cast] Send failed:', err);
  });
}

/**
 * Check if a Cast session is currently active.
 */
export function isConnected(): boolean {
  const session = castContext?.getCurrentSession();
  return session?.getSessionState() === cast.framework.SessionState.SESSION_STARTED;
}

/**
 * Get the current Cast device name.
 */
export function getDeviceName(): string | null {
  return castContext?.getCurrentSession()?.getCastDevice()?.friendlyName ?? null;
}

/**
 * Register a listener for Cast connection state changes.
 */
export function onStateChange(listener: StateListener): () => void {
  stateListeners.push(listener);
  return () => {
    const idx = stateListeners.indexOf(listener);
    if (idx >= 0) stateListeners.splice(idx, 1);
  };
}

/**
 * Register a listener for messages from the receiver.
 */
export function onMessage(listener: MessageListener): () => void {
  messageListeners.push(listener);
  return () => {
    const idx = messageListeners.indexOf(listener);
    if (idx >= 0) messageListeners.splice(idx, 1);
  };
}

/**
 * Check if the Cast SDK is loaded and available.
 */
export function isSdkReady(): boolean {
  return sdkLoaded;
}

// ── Helpers ────────────────────────────────────────────────────

function loadScript(src: string): Promise<void> {
  return new Promise((resolve, reject) => {
    // Don't double-load
    if (document.querySelector(`script[src="${src}"]`)) {
      resolve();
      return;
    }
    const script = document.createElement('script');
    script.src = src;
    script.async = true;
    script.onload = () => resolve();
    script.onerror = () => reject(new Error(`Failed to load ${src}`));
    document.head.appendChild(script);
  });
}
