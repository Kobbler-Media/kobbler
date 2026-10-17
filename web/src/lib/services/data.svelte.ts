/**
 * Data service - auth-related helpers that bridge the API with stores.
 * Load functions for library data live in context/app.svelte.ts.
 */

import {
  setAuthed,
  setRole,
  setEmail,
  setAvatarUrl,
  setHasPin,
  setIsKids,
  setTimezone,
  setCastReceiverAppId,
  clearAuth
} from '$lib/stores/auth.svelte';
import { setDevices } from '$lib/stores/devices.svelte';
import { closeStream } from '$lib/services/eventstream.svelte';
import * as api from '$lib/api/client';

const apiBase = import.meta.env.VITE_API_BASE_URL || '';

function browserLabel(): string {
  if (typeof navigator === 'undefined') return 'Browser';
  const ua = navigator.userAgent;
  const browser = /Firefox\//.test(ua)
    ? 'Firefox'
    : /Edg\//.test(ua)
      ? 'Edge'
      : /OPR\//.test(ua)
        ? 'Opera'
        : /Chrome\//.test(ua)
          ? 'Chrome'
          : /Safari\//.test(ua)
            ? 'Safari'
            : 'Browser';
  const os = /Windows/.test(ua)
    ? 'Windows'
    : /Android/.test(ua)
      ? 'Android'
      : /iPhone|iPad/.test(ua)
        ? 'iOS'
        : /Mac OS X/.test(ua)
          ? 'macOS'
          : /Linux/.test(ua)
            ? 'Linux'
            : '';
  return os ? `${browser} on ${os}` : browser;
}

/**
 * Lightweight auth probe — checks if the session is valid without loading data into stores.
 * Uses the /me endpoint to verify auth and get user role.
 */
export async function checkAuth(): Promise<boolean> {
  try {
    const resp = await fetch(`${apiBase}/api/v1/admin/me`, {
      credentials: 'include'
    });
    if (resp.ok) {
      const data = await resp.json();
      setAuthed(true);
      setRole(data.role || 'viewer');
      setEmail(data.email || '');
      setAvatarUrl(data.avatar_url || '');
      setHasPin(Boolean(data.has_pin));
      setIsKids(Boolean(data.is_kids));
      setTimezone(data.timezone || '');
      setCastReceiverAppId(data.cast_receiver_app_id || '');
      // Sessions have a fixed expiry and nothing else renews them, so a browser that
      // gets used inside the window should never be logged out. Best-effort.
      api.refreshSession(browserLabel(), 'web').catch(() => {});
      return true;
    }
    if (resp.status === 401) {
      closeStream();
      setAuthed(false);
      setRole('');
    }
    return false;
  } catch {
    return false;
  }
}

/**
 * Log out the current user — revoke session and clear client state.
 */
export async function logout(): Promise<void> {
  try {
    await fetch(`${apiBase}/api/v1/admin/logout`, {
      method: 'POST',
      credentials: 'include'
    });
  } catch {
    // Best-effort — clear client state regardless
  }
  closeStream();
  clearAuth();
}

/**
 * Load devices (from library endpoint - for book sync)
 */
export async function loadLibraryDevices(): Promise<void> {
  try {
    const devices = await api.getDevices();
    setDevices(devices);
  } catch {
    // Silently fail - devices are supplementary
  }
}
