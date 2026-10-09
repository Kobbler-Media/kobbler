/**
 * Unified SSE event stream service.
 * Replaces the WebSocket service with a single EventSource connection
 * that receives library, job, and ingest queue events.
 */

let es: EventSource | null = null;
let connected = $state(false);
let showBanner = $state(false);
let gaveUp = $state(false);
let apiBaseUrl = '';
let lastEventTime = 0;
// True once the stream has connected at least once — distinguishes the
// initial connect from (silent) auto-reconnects in es.onopen.
let everConnected = false;

let bannerTimer: ReturnType<typeof setTimeout> | null = null;
let visibilityListenerActive = false;
let lastIsAuthed = false;

// Consecutive error tracking. EventSource retries on its own every ~3s, so
// this many failures means roughly 30s of outage — past that we stop leaning
// on its fast retry and switch to our own slow backoff.
let consecutiveErrors = 0;
const MAX_ERRORS_BEFORE_GIVEUP = 10;

// Background reconnect delays, used once EventSource's own retry has failed
// MAX_ERRORS_BEFORE_GIVEUP times. The last entry repeats forever, so the
// stream reconnects by itself after an outage that outlasts the fast retry
// (proxy restart, server reboot) without the user reloading the page.
const RECONNECT_BACKOFFS_MS = [5_000, 15_000, 30_000, 60_000];
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
let backoffAttempt = 0;

// Stale data threshold (60 seconds)
const STALE_THRESHOLD_MS = 60_000;

// Callback registry — supports multiple listeners per event type (not reactive, internal only)
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Callback = (data: any) => void;
// eslint-disable-next-line svelte/prefer-svelte-reactivity -- internal registry, not reactive state
const callbacks = new Map<string, Set<Callback>>();
let onReconnectCallback: (() => void) | null = null;

// Reconnect callback registry — allows pages to register handlers for visibility-based reconnection
// eslint-disable-next-line svelte/prefer-svelte-reactivity -- internal registry, not reactive state
const reconnectCallbacks = new Set<() => void>();

// All SSE event types we listen for
const EVENT_TYPES = [
  'library_updated',
  'author_updated',
  'series_updated',
  'movie_updated',
  'tv_show_updated',
  'proxy_log_created',
  'kobload_download_snapshot',
  'kobload_download_created',
  'kobload_download_updated',
  'wanted_status_updated',
  'job',
  'jobs_resync',
  'ingest_queue',
  'notification_created',
  'stream_warm_ready',
  'skip_segments_ready',
  'watch_progress',
  'activity_log_updated',
  'livetv_lease_changed',
  'livetv_recording_updated',
  'livetv_guide_synced',
  'livetv_scan_progress'
] as const;

export type EventType = (typeof EVENT_TYPES)[number];

export function getStreamState() {
  return {
    get connected() {
      return connected;
    },
    get showBanner() {
      return showBanner;
    },
    get gaveUp() {
      return gaveUp;
    }
  };
}

export function setApiBase(base: string) {
  apiBaseUrl = base;
}

/**
 * Register a callback for an event type. Returns the callback for use with off().
 * Multiple callbacks can be registered for the same event type.
 */
export function on(eventType: EventType, callback: Callback): Callback {
  let set = callbacks.get(eventType);
  if (!set) {
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- internal registry, not reactive state
    set = new Set();
    callbacks.set(eventType, set);
  }
  set.add(callback);
  return callback;
}

/**
 * Register a debounced callback for an event type.
 * Coalesces rapid events into a single handler call after `ms` milliseconds of quiet.
 * During sustained activity, fires at most every `maxWait` ms to prevent indefinite deferral.
 * Returns the internal callback for use with off().
 */
export function onDebounced(
  eventType: EventType,
  callback: Callback,
  ms = 500,
  maxWait = 3000
): Callback {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let lastFired = 0;
  const debounced: Callback = (data) => {
    clearTimeout(timer);
    const now = Date.now();
    if (now - lastFired >= maxWait) {
      lastFired = now;
      callback(data);
    } else {
      timer = setTimeout(() => {
        lastFired = Date.now();
        callback(data);
      }, ms);
    }
  };
  return on(eventType, debounced);
}

/**
 * Unregister a specific callback for an event type.
 */
export function off(eventType: EventType, callback: Callback) {
  const set = callbacks.get(eventType);
  if (set) {
    set.delete(callback);
    if (set.size === 0) callbacks.delete(eventType);
  }
}

export function setOnReconnect(callback: (() => void) | null) {
  onReconnectCallback = callback;
}

/**
 * Register a callback that fires when SSE reconnects after visibility change.
 * Useful for pages that need to refresh data when the tab becomes visible again.
 * Returns the callback for use with offReconnect().
 */
export function onReconnect(callback: () => void): () => void {
  reconnectCallbacks.add(callback);
  return callback;
}

/**
 * Unregister a reconnect callback.
 */
export function offReconnect(callback: () => void) {
  reconnectCallbacks.delete(callback);
}

function clearBannerTimer() {
  if (bannerTimer) {
    clearTimeout(bannerTimer);
    bannerTimer = null;
  }
}

function clearReconnectTimer() {
  if (reconnectTimer) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
}

/**
 * Schedule a background reconnect on an escalating delay, capped at the last
 * RECONNECT_BACKOFFS_MS entry and retried indefinitely. The banner stays up
 * while this runs, so the user still sees that live updates are down — but
 * they no longer have to reload the page to get them back.
 */
function scheduleReconnect() {
  clearReconnectTimer();
  const delay = RECONNECT_BACKOFFS_MS[Math.min(backoffAttempt, RECONNECT_BACKOFFS_MS.length - 1)];
  backoffAttempt++;
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    if (!lastIsAuthed) return;
    // Deliberately leave consecutiveErrors past the threshold: if this attempt
    // fails, onerror re-schedules straight away at the next delay instead of
    // burning another ~30s of EventSource's 3s retries first. That keeps a long
    // outage to one attempt per interval. onopen resets the counters on success.
    connectStream(lastIsAuthed);
  }, delay);
}

function handleVisibilityChange() {
  if (document.visibilityState !== 'visible' || !lastIsAuthed) return;

  if (!es || es.readyState === EventSource.CLOSED) {
    // Connection died while hidden — reconnect now rather than waiting out
    // whatever backoff delay is pending. es.onopen fires the reconnect
    // re-sync once the new connection opens.
    consecutiveErrors = 0;
    backoffAttempt = 0;
    gaveUp = false;
    connectStream(true);
  } else if (Date.now() - lastEventTime > STALE_THRESHOLD_MS) {
    // Connected but stale — force a reconnect to catch up. es.onopen fires
    // the reconnect re-sync once the new connection opens.
    closeStream();
    connectStream(lastIsAuthed);
  }
}

function setupVisibilityListener() {
  if (visibilityListenerActive || typeof document === 'undefined') return;
  document.addEventListener('visibilitychange', handleVisibilityChange);
  visibilityListenerActive = true;
}

function removeVisibilityListener() {
  if (!visibilityListenerActive || typeof document === 'undefined') return;
  document.removeEventListener('visibilitychange', handleVisibilityChange);
  visibilityListenerActive = false;
}

export function ensureStream(isAuthed: boolean) {
  if (!isAuthed || gaveUp) return;
  if (es && es.readyState !== EventSource.CLOSED) return;
  connectStream(isAuthed);
}

export function connectStream(isAuthed: boolean) {
  if (!isAuthed) return;

  lastIsAuthed = isAuthed;
  setupVisibilityListener();
  // We're connecting now — drop any pending backoff attempt.
  clearReconnectTimer();

  if (es) {
    es.close();
    es = null;
  }

  connected = false;
  // Keep the banner up across a backoff reconnect — clearing it on every
  // attempt would flicker it off and back on through a long outage. The
  // explicit entry points (retryStream, visibility change) clear gaveUp first,
  // so a user-driven retry still dismisses it immediately.
  if (!gaveUp) showBanner = false;
  clearBannerTimer();

  const url = `${apiBaseUrl}/api/v1/admin/events`;
  es = new EventSource(url, { withCredentials: true });

  es.onopen = () => {
    connected = true;
    showBanner = false;
    gaveUp = false;
    consecutiveErrors = 0;
    backoffAttempt = 0;
    clearReconnectTimer();
    clearBannerTimer();
    lastEventTime = Date.now();
    // Reconnect re-sync: covers silent auto-reconnects (the browser's
    // EventSource retries after a drop without any further error callback)
    // as well as tab-visibility reconnects. Delayed so the SSE stream
    // delivers fresh events first (event handlers already debounce their
    // API calls). Not fired on the initial connect — startup loads data.
    if (everConnected) {
      setTimeout(() => {
        onReconnectCallback?.();
        for (const cb of reconnectCallbacks) {
          try {
            cb();
          } catch {
            // Ignore handler errors
          }
        }
      }, 500);
    }
    everConnected = true;
  };

  es.onerror = () => {
    connected = false;
    consecutiveErrors++;

    if (consecutiveErrors >= MAX_ERRORS_BEFORE_GIVEUP) {
      // EventSource's fast retry isn't getting through. Drop it and hand over
      // to our slow backoff, which keeps trying until the server is back.
      gaveUp = true;
      showBanner = true;
      clearBannerTimer();
      if (es) {
        es.close();
        es = null;
      }
      scheduleReconnect();
      return;
    }

    // Show banner after 4s of disconnection
    if (!bannerTimer) {
      bannerTimer = setTimeout(() => {
        if (!connected) showBanner = true;
      }, 4000);
    }
    // EventSource auto-reconnects — no manual backoff needed
  };

  // Register listeners for each event type
  for (const eventName of EVENT_TYPES) {
    es.addEventListener(eventName, (e: MessageEvent) => {
      // Track event time for stale detection
      lastEventTime = Date.now();

      // Clear banner on successful message
      if (showBanner) showBanner = false;
      clearBannerTimer();
      consecutiveErrors = 0;

      const handlers = callbacks.get(eventName);
      if (handlers) {
        let parsed: unknown;
        try {
          parsed = JSON.parse(e.data);
        } catch {
          return;
        }
        for (const handler of handlers) {
          try {
            handler(parsed);
          } catch (err) {
            console.error(`[SSE] Error in ${eventName} handler:`, err);
          }
        }
      }
    });
  }
}

export function closeStream() {
  connected = false;
  showBanner = false;
  gaveUp = false;
  consecutiveErrors = 0;
  backoffAttempt = 0;
  clearBannerTimer();
  clearReconnectTimer();

  if (es) {
    es.close();
    es = null;
  }
}

/**
 * Reset error state and retry connection.
 * Call when user explicitly clicks retry.
 */
export function retryStream(isAuthed: boolean) {
  gaveUp = false;
  consecutiveErrors = 0;
  backoffAttempt = 0;
  connectStream(isAuthed);
}

/**
 * Clean up all resources. Call on component unmount.
 */
export function cleanup() {
  closeStream();
  removeVisibilityListener();
  lastIsAuthed = false;
  everConnected = false;
  callbacks.clear();
  reconnectCallbacks.clear();
  onReconnectCallback = null;
}
