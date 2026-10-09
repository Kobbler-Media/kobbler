// Screen Wake Lock API wrapper
//
// Keeps the screen on while reading or listening. Uses ref counting so
// multiple callers (reader page, listen page) can independently request
// and release without interfering with each other.
//
// The wake lock is automatically released by the browser when the page
// becomes hidden (tab switch, screen off). We re-acquire it when the
// page becomes visible again, as long as refCount > 0.

let sentinel: WakeLockSentinel | null = null;
let refCount = 0;

function isSupported(): boolean {
  return typeof navigator !== 'undefined' && 'wakeLock' in navigator;
}

async function acquire() {
  if (!isSupported() || sentinel) return;
  try {
    sentinel = await navigator.wakeLock.request('screen');
    sentinel.addEventListener('release', () => {
      sentinel = null;
    });
  } catch {
    // Permission denied or other failure — not critical
  }
}

function onVisibilityChange() {
  if (document.visibilityState === 'visible' && refCount > 0) {
    acquire();
  }
}

/** Request the screen to stay on. Call releaseWakeLock() when done. */
export async function requestWakeLock() {
  refCount++;
  if (refCount === 1) {
    document.addEventListener('visibilitychange', onVisibilityChange);
    await acquire();
  }
}

/** Release one wake lock request. Screen may turn off when all callers release. */
export async function releaseWakeLock() {
  refCount = Math.max(0, refCount - 1);
  if (refCount === 0) {
    document.removeEventListener('visibilitychange', onVisibilityChange);
    if (sentinel) {
      try {
        await sentinel.release();
      } catch {
        // Already released
      }
      sentinel = null;
    }
  }
}
